use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

use newkitine::types::{FileAttributes, SearchScope};

use super::db;
use super::events::buddy_view;
use super::settings::Settings;
use super::state::{App, ChatMessage, SearchView, TransferView, UserInfoView, now};

pub fn router(app: Arc<App>, web_root: &str) -> Router {
    let static_files = ServeDir::new(web_root)
        .fallback(ServeFile::new(format!("{web_root}/index.html")));
    Router::new()
        .route("/api/status", get(status))
        .route("/api/connect", post(connect))
        .route("/api/disconnect", post(disconnect))
        .route("/api/reconnect", post(reconnect))
        .route("/api/port", post(set_port))
        .route("/api/searches", get(list_searches).post(start_search))
        .route("/api/searches/{token}", get(search_results).delete(remove_search))
        .route("/api/downloads", get(list_downloads).post(enqueue_download))
        .route("/api/downloads/folder", post(enqueue_folder_download))
        .route("/api/downloads/abort", post(abort_download))
        .route("/api/downloads/retry", post(retry_download))
        .route("/api/downloads/clear", post(clear_downloads))
        .route("/api/downloads/clear_all", post(clear_all_downloads))
        .route("/api/uploads", get(list_uploads))
        .route("/api/uploads/abort", post(abort_upload))
        .route("/api/uploads/clear", post(clear_uploads))
        .route("/api/uploads/clear_all", post(clear_all_uploads))
        .route("/api/users/{username}/browse", post(browse_user))
        .route("/api/users/{username}/folders", get(user_folders))
        .route("/api/users/{username}/tree", get(user_tree))
        .route("/api/users/{username}/files", get(user_files))
        .route("/api/users/{username}/info", get(get_user_info).post(request_user_info))
        .route("/api/buddies", get(list_buddies).post(add_buddy))
        .route("/api/buddies/{username}", delete(remove_buddy))
        .route("/api/buddies/{username}/note", post(set_buddy_note))
        .route("/api/banned", get(list_banned).post(ban_user))
        .route("/api/banned/{username}", delete(unban_user))
        .route("/api/ignored", get(list_ignored).post(ignore_user))
        .route("/api/ignored/{username}", delete(unignore_user))
        .route("/api/wishlist", get(list_wishlist).post(add_wish))
        .route("/api/wishlist/remove", post(remove_wish))
        .route("/api/chats", get(list_chats))
        .route(
            "/api/chats/{username}",
            get(chat_history).post(send_private_message).delete(close_chat),
        )
        .route("/api/chats/{username}/open", post(open_chat))
        .route("/api/rooms", get(rooms))
        .route("/api/rooms/join", post(join_room))
        .route("/api/rooms/leave", post(leave_room))
        .route("/api/rooms/{room}/messages", get(room_history).post(say_room))
        .route("/api/interests", get(interests))
        .route("/api/interests/add", post(add_interest))
        .route("/api/interests/remove", post(remove_interest))
        .route("/api/interests/refresh", post(refresh_interests))
        .route("/api/interests/item", post(item_recommendations))
        .route("/api/shares/rescan", post(rescan_shares))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/ws", get(ws_upgrade))
        .fallback_service(static_files)
        .with_state(app)
}

async fn ws_upgrade(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(app, socket))
}

async fn ws_session(app: Arc<App>, mut socket: WebSocket) {
    let mut events = app.events.subscribe();
    let snapshot = {
        let data = app.data.read().unwrap();
        json!({ "type": "status", "status": &data.status }).to_string()
    };
    if socket.send(Message::Text(snapshot.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            message = socket.recv() => match message {
                Some(Ok(_)) => continue,
                _ => return,
            },
        }
    }
}

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "status": &data.status }))
}

async fn connect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.connect();
    StatusCode::ACCEPTED
}

async fn disconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.disconnect();
    StatusCode::ACCEPTED
}

async fn reconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.reconnect();
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct PortBody {
    port: u16,
}

async fn set_port(State(app): State<Arc<App>>, Json(body): Json<PortBody>) -> StatusCode {
    {
        let mut data = app.data.write().unwrap();
        data.status.listen_port = body.port;
        app.broadcast(json!({ "type": "status", "status": &data.status }));
    }
    app.client.set_listen_port(body.port);
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default = "default_search_mode")]
    mode: String,
    room: Option<String>,
    user: Option<String>,
}

fn default_search_mode() -> String {
    "global".into()
}

async fn list_searches(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    let searches: Vec<serde_json::Value> = data
        .searches
        .iter()
        .map(|search| {
            json!({
                "token": search.token,
                "query": search.query,
                "responses": search.results.len(),
            })
        })
        .collect();
    Json(json!({ "searches": searches }))
}

async fn start_search(
    State(app): State<Arc<App>>,
    Json(body): Json<SearchBody>,
) -> impl IntoResponse {
    let scope = match body.mode.as_str() {
        "global" => SearchScope::Global,
        "buddies" => SearchScope::Buddies,
        "rooms" => match body.room {
            Some(room) => SearchScope::Room(room),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        "user" => match body.user {
            Some(user) => SearchScope::User(user),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let token = app.client.search(&body.query, scope);
    let mut data = app.data.write().unwrap();
    data.searches.push(SearchView { token, query: body.query, results: Vec::new() });
    app.broadcast(json!({ "type": "search_added", "search": data.searches.last().unwrap() }));
    Json(json!({ "token": token })).into_response()
}

async fn search_results(
    State(app): State<Arc<App>>,
    Path(token): Path<u32>,
) -> impl IntoResponse {
    let data = app.data.read().unwrap();
    match data.searches.iter().find(|search| search.token == token) {
        Some(search) => Json(serde_json::to_value(search).unwrap()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn remove_search(State(app): State<Arc<App>>, Path(token): Path<u32>) -> StatusCode {
    app.client.cancel_search(token);
    let mut data = app.data.write().unwrap();
    data.searches.retain(|search| search.token != token);
    app.broadcast(json!({ "type": "search_removed", "token": token }));
    StatusCode::NO_CONTENT
}

fn transfer_list(views: &std::collections::HashMap<(String, String), TransferView>) -> Vec<&TransferView> {
    let mut list: Vec<&TransferView> = views.values().collect();
    list.sort_by_key(|view| std::cmp::Reverse(view.updated_at));
    list
}

async fn list_downloads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "transfers": transfer_list(&data.downloads) }))
}

async fn list_uploads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "transfers": transfer_list(&data.uploads) }))
}

#[derive(Deserialize)]
struct DownloadBody {
    username: String,
    virtual_path: String,
    size: u64,
    #[serde(default)]
    attributes: FileAttributes,
}

async fn start_download(
    app: &App,
    username: &str,
    virtual_path: &str,
    size: u64,
    attributes: FileAttributes,
) {
    let view = TransferView {
        username: username.to_owned(),
        virtual_path: virtual_path.to_owned(),
        size,
        bytes_done: 0,
        status: "queued".into(),
        file_path: None,
        queue_place: 0,
        attributes: attributes.clone(),
        updated_at: now(),
    };
    db::upsert_transfer(&app.db, "download", &view).await;
    {
        let mut data = app.data.write().unwrap();
        app.broadcast(json!({ "type": "transfer", "direction": "download", "transfer": &view }));
        data.downloads.insert((view.username.clone(), view.virtual_path.clone()), view);
    }
    app.client.download(username, virtual_path, size, attributes);
}

async fn enqueue_download(
    State(app): State<Arc<App>>,
    Json(body): Json<DownloadBody>,
) -> StatusCode {
    start_download(&app, &body.username, &body.virtual_path, body.size, body.attributes).await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct FolderDownloadBody {
    username: String,
    dir: String,
    #[serde(default)]
    recursive: bool,
}

async fn enqueue_folder_download(
    State(app): State<Arc<App>>,
    Json(body): Json<FolderDownloadBody>,
) -> impl IntoResponse {
    let files: Vec<(String, u64, FileAttributes)> = {
        let data = app.data.read().unwrap();
        let Some(browse) = data.browses.get(&body.username) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let subdir_prefix = format!("{}\\", body.dir);
        browse
            .folders
            .iter()
            .chain(browse.private_folders.iter())
            .filter(|folder| {
                folder.directory == body.dir
                    || (body.recursive && folder.directory.starts_with(&subdir_prefix))
            })
            .flat_map(|folder| {
                folder.files.iter().map(|file| {
                    (
                        format!("{}\\{}", folder.directory, file.name),
                        file.size,
                        file.attributes.clone(),
                    )
                })
            })
            .collect()
    };
    let count = files.len();
    for (virtual_path, size, attributes) in files {
        start_download(&app, &body.username, &virtual_path, size, attributes).await;
    }
    Json(json!({ "enqueued": count })).into_response()
}

#[derive(Deserialize)]
struct TransferRef {
    username: String,
    virtual_path: String,
}

async fn abort_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    app.client.abort_download(&body.username, &body.virtual_path);
    let view = {
        let mut data = app.data.write().unwrap();
        let key = (body.username, body.virtual_path);
        let Some(view) = data.downloads.get_mut(&key) else { return StatusCode::NOT_FOUND };
        view.status = "aborted".into();
        view.updated_at = now();
        view.clone()
    };
    db::upsert_transfer(&app.db, "download", &view).await;
    app.broadcast(json!({ "type": "transfer", "direction": "download", "transfer": &view }));
    StatusCode::ACCEPTED
}

async fn retry_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    let view = {
        let mut data = app.data.write().unwrap();
        let key = (body.username.clone(), body.virtual_path.clone());
        let Some(view) = data.downloads.get_mut(&key) else { return StatusCode::NOT_FOUND };
        view.status = "queued".into();
        view.updated_at = now();
        view.clone()
    };
    db::upsert_transfer(&app.db, "download", &view).await;
    app.broadcast(json!({ "type": "transfer", "direction": "download", "transfer": &view }));
    app.client.download(&body.username, &body.virtual_path, view.size, view.attributes);
    StatusCode::ACCEPTED
}

const CLEARED_STATUSES: &[&str] = &["finished", "aborted", "failed"];

fn is_cleared(status: &str) -> bool {
    status == "finished" || status == "aborted" || status.starts_with("failed")
}

#[derive(Deserialize)]
struct ClearBody {
    statuses: Vec<String>,
}

fn clear_statuses(body: ClearBody) -> Option<Vec<String>> {
    if body.statuses.is_empty()
        || body.statuses.iter().any(|status| !CLEARED_STATUSES.contains(&status.as_str()))
    {
        return None;
    }
    Some(body.statuses)
}

fn status_matches(status: &str, statuses: &[String]) -> bool {
    statuses.iter().any(|cleared| match cleared.as_str() {
        "failed" => status.starts_with("failed"),
        other => status == other,
    })
}

async fn clear_downloads(
    State(app): State<Arc<App>>,
    Json(body): Json<ClearBody>,
) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else { return StatusCode::BAD_REQUEST };
    let refs: Vec<&str> = statuses.iter().map(String::as_str).collect();
    db::clear_transfers(&app.db, "download", &refs).await;
    let mut data = app.data.write().unwrap();
    data.downloads.retain(|_, view| !status_matches(&view.status, &statuses));
    app.broadcast(json!({
        "type": "transfers_cleared",
        "direction": "download",
        "statuses": statuses,
    }));
    StatusCode::ACCEPTED
}

async fn abort_upload(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    app.client.abort_upload(&body.username, &body.virtual_path);
    let view = {
        let mut data = app.data.write().unwrap();
        let key = (body.username, body.virtual_path);
        let Some(view) = data.uploads.get_mut(&key) else { return StatusCode::NOT_FOUND };
        view.status = "aborted".into();
        view.updated_at = now();
        view.clone()
    };
    db::upsert_transfer(&app.db, "upload", &view).await;
    app.broadcast(json!({ "type": "transfer", "direction": "upload", "transfer": &view }));
    StatusCode::ACCEPTED
}

async fn clear_uploads(
    State(app): State<Arc<App>>,
    Json(body): Json<ClearBody>,
) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else { return StatusCode::BAD_REQUEST };
    let refs: Vec<&str> = statuses.iter().map(String::as_str).collect();
    db::clear_transfers(&app.db, "upload", &refs).await;
    let mut data = app.data.write().unwrap();
    data.uploads.retain(|_, view| !status_matches(&view.status, &statuses));
    app.broadcast(json!({
        "type": "transfers_cleared",
        "direction": "upload",
        "statuses": statuses,
    }));
    StatusCode::ACCEPTED
}

async fn clear_all_downloads(State(app): State<Arc<App>>) -> StatusCode {
    db::clear_all_transfers(&app.db, "download").await;
    let mut data = app.data.write().unwrap();
    for view in data.downloads.values() {
        if !is_cleared(&view.status) {
            app.client.abort_download(&view.username, &view.virtual_path);
        }
    }
    data.downloads.clear();
    app.broadcast(json!({ "type": "transfers_cleared", "direction": "download", "scope": "all" }));
    StatusCode::ACCEPTED
}

async fn clear_all_uploads(State(app): State<Arc<App>>) -> StatusCode {
    db::clear_all_transfers(&app.db, "upload").await;
    let mut data = app.data.write().unwrap();
    for view in data.uploads.values() {
        if !is_cleared(&view.status) {
            app.client.abort_upload(&view.username, &view.virtual_path);
        }
    }
    data.uploads.clear();
    app.broadcast(json!({ "type": "transfers_cleared", "direction": "upload", "scope": "all" }));
    StatusCode::ACCEPTED
}

async fn browse_user(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    app.client.browse_user(&username);
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct FoldersQuery {
    #[serde(default)]
    filter: String,
    #[serde(default = "default_folder_limit")]
    limit: usize,
}

fn default_folder_limit() -> usize {
    500
}

async fn user_folders(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    let data = app.data.read().unwrap();
    let Some(browse) = data.browses.get(&username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let filter = query.filter.to_lowercase();
    let matching = browse
        .folders
        .iter()
        .chain(browse.private_folders.iter())
        .filter(|folder| filter.is_empty() || folder.directory.to_lowercase().contains(&filter));
    let total = matching.clone().count();
    let folders: Vec<serde_json::Value> = matching
        .take(query.limit)
        .map(|folder| {
            json!({ "directory": folder.directory, "file_count": folder.files.len() })
        })
        .collect();
    Json(json!({
        "username": username,
        "folders": folders,
        "total": total,
        "received_at": browse.received_at,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct TreeQuery {
    #[serde(default)]
    dir: String,
}

async fn user_tree(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<TreeQuery>,
) -> impl IntoResponse {
    let data = app.data.read().unwrap();
    let Some(browse) = data.browses.get(&username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let prefix = if query.dir.is_empty() {
        String::new()
    } else {
        format!("{}\\", query.dir)
    };
    let mut children: std::collections::BTreeMap<&str, (usize, bool, bool)> =
        std::collections::BTreeMap::new();
    for (folder, private) in browse
        .folders
        .iter()
        .map(|folder| (folder, false))
        .chain(browse.private_folders.iter().map(|folder| (folder, true)))
    {
        let Some(rest) = folder.directory.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        let (name, deeper) = match rest.split_once('\\') {
            Some((name, _)) => (name, true),
            None => (rest, false),
        };
        let entry = children.entry(name).or_insert((0, false, private));
        if deeper {
            entry.1 = true;
        } else {
            entry.0 = folder.files.len();
            entry.2 = private;
        }
    }
    let children: Vec<serde_json::Value> = children
        .into_iter()
        .map(|(name, (file_count, has_children, private))| {
            json!({
                "name": name,
                "path": format!("{prefix}{name}"),
                "file_count": file_count,
                "has_children": has_children,
                "private": private,
            })
        })
        .collect();
    let mut response = json!({ "dir": query.dir, "children": children });
    if query.dir.is_empty() {
        let all = browse.folders.iter().chain(browse.private_folders.iter());
        let (mut files, mut size) = (0u64, 0u64);
        for folder in all {
            files += folder.files.len() as u64;
            size += folder.files.iter().map(|file| file.size).sum::<u64>();
        }
        response["summary"] = json!({
            "folders": browse.folders.len() + browse.private_folders.len(),
            "files": files,
            "size": size,
        });
    }
    Json(response).into_response()
}

#[derive(Deserialize)]
struct FilesQuery {
    dir: String,
}

async fn user_files(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    let data = app.data.read().unwrap();
    let Some(browse) = data.browses.get(&username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match browse
        .folders
        .iter()
        .chain(browse.private_folders.iter())
        .find(|folder| folder.directory == query.dir)
    {
        Some(folder) => Json(json!({ "directory": folder.directory, "files": folder.files }))
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn request_user_info(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> StatusCode {
    app.client.request_user_info(&username);
    app.client.server_request(newkitine::protocol::ServerRequest::GetUserStats {
        user: username.clone(),
    });
    app.client.server_request(newkitine::protocol::ServerRequest::UserInterests {
        user: username.clone(),
    });
    let mut data = app.data.write().unwrap();
    let info = data
        .user_infos
        .entry(username.clone())
        .or_insert_with(|| UserInfoView { username, ..Default::default() });
    app.broadcast(json!({ "type": "user_info", "info": info.clone() }));
    StatusCode::ACCEPTED
}

async fn get_user_info(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> impl IntoResponse {
    let data = app.data.read().unwrap();
    match data.user_infos.get(&username) {
        Some(info) => Json(serde_json::to_value(info).unwrap()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct UserBody {
    username: String,
}

async fn list_buddies(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    let mut buddies: Vec<_> = data.buddies.values().collect();
    buddies.sort_by(|a, b| a.username.cmp(&b.username));
    Json(json!({ "buddies": buddies }))
}

async fn add_buddy(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    app.client.add_buddy(&body.username);
    db::add_to_list(&app.db, "buddy", &body.username).await;
    let mut data = app.data.write().unwrap();
    let buddy = data
        .buddies
        .entry(body.username.clone())
        .or_insert_with(|| buddy_view(&body.username));
    app.broadcast(json!({ "type": "buddy", "buddy": buddy.clone() }));
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct NoteBody {
    note: String,
}

async fn set_buddy_note(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<NoteBody>,
) -> StatusCode {
    db::set_note(&app.db, &username, &body.note).await;
    let mut data = app.data.write().unwrap();
    let Some(buddy) = data.buddies.get_mut(&username) else { return StatusCode::NOT_FOUND };
    buddy.note = body.note;
    app.broadcast(json!({ "type": "buddy", "buddy": buddy.clone() }));
    StatusCode::ACCEPTED
}

async fn remove_buddy(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> StatusCode {
    app.client.remove_buddy(&username);
    db::remove_from_list(&app.db, "buddy", &username).await;
    app.data.write().unwrap().buddies.remove(&username);
    app.broadcast(json!({ "type": "buddy_removed", "username": username }));
    StatusCode::ACCEPTED
}

async fn list_banned(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "users": &data.banned }))
}

async fn ban_user(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    app.client.ban_user(&body.username);
    db::add_to_list(&app.db, "banned", &body.username).await;
    let mut data = app.data.write().unwrap();
    if !data.banned.contains(&body.username) {
        data.banned.push(body.username);
        data.banned.sort();
    }
    app.broadcast(json!({ "type": "banned", "users": &data.banned }));
    StatusCode::ACCEPTED
}

async fn unban_user(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    app.client.unban_user(&username);
    db::remove_from_list(&app.db, "banned", &username).await;
    let mut data = app.data.write().unwrap();
    data.banned.retain(|user| user != &username);
    app.broadcast(json!({ "type": "banned", "users": &data.banned }));
    StatusCode::ACCEPTED
}

async fn list_ignored(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "users": &data.ignored }))
}

async fn ignore_user(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    app.client.ignore_user(&body.username);
    db::add_to_list(&app.db, "ignored", &body.username).await;
    let mut data = app.data.write().unwrap();
    if !data.ignored.contains(&body.username) {
        data.ignored.push(body.username);
        data.ignored.sort();
    }
    app.broadcast(json!({ "type": "ignored", "users": &data.ignored }));
    StatusCode::ACCEPTED
}

async fn unignore_user(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    app.client.unignore_user(&username);
    db::remove_from_list(&app.db, "ignored", &username).await;
    let mut data = app.data.write().unwrap();
    data.ignored.retain(|user| user != &username);
    app.broadcast(json!({ "type": "ignored", "users": &data.ignored }));
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct WishBody {
    term: String,
}

async fn list_wishlist(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "wishlist": &data.wishlist }))
}

async fn add_wish(State(app): State<Arc<App>>, Json(body): Json<WishBody>) -> StatusCode {
    app.client.add_wish(&body.term);
    db::add_wish(&app.db, &body.term).await;
    let mut data = app.data.write().unwrap();
    if !data.wishlist.contains(&body.term) {
        data.wishlist.push(body.term);
        data.wishlist.sort();
    }
    app.broadcast(json!({ "type": "wishlist", "wishlist": &data.wishlist }));
    StatusCode::ACCEPTED
}

async fn remove_wish(State(app): State<Arc<App>>, Json(body): Json<WishBody>) -> StatusCode {
    app.client.remove_wish(&body.term);
    db::remove_wish(&app.db, &body.term).await;
    let mut data = app.data.write().unwrap();
    data.wishlist.retain(|term| term != &body.term);
    app.broadcast(json!({ "type": "wishlist", "wishlist": &data.wishlist }));
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    100
}

async fn list_chats(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let partners = db::load_list(&app.db, "chat").await;
    Json(json!({ "chats": partners }))
}

async fn open_chat(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    db::add_to_list(&app.db, "chat", &username).await;
    app.broadcast(json!({ "type": "chat_opened", "username": username }));
    StatusCode::ACCEPTED
}

async fn close_chat(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    db::remove_from_list(&app.db, "chat", &username).await;
    app.broadcast(json!({ "type": "chat_closed", "username": username }));
    StatusCode::ACCEPTED
}

async fn chat_history(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let messages = db::load_chat(&app.db, "private", &username, query.limit).await;
    Json(json!({ "username": username, "messages": messages }))
}

#[derive(Deserialize)]
struct MessageBody {
    message: String,
}

async fn send_private_message(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<MessageBody>,
) -> StatusCode {
    app.client.send_private_message(&username, &body.message);
    let sender = app.data.read().unwrap().status.username.clone();
    let message = ChatMessage { sender, message: body.message, timestamp: now() };
    db::insert_chat(&app.db, "private", &username, &message).await;
    db::add_to_list(&app.db, "chat", &username).await;
    app.data
        .write()
        .unwrap()
        .private_chats
        .entry(username.clone())
        .or_default()
        .push(message.clone());
    app.broadcast(json!({
        "type": "private_message",
        "username": username,
        "message": message,
    }));
    StatusCode::ACCEPTED
}

async fn rooms(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "rooms": &data.rooms }))
}

#[derive(Deserialize)]
struct RoomBody {
    room: String,
}

async fn join_room(State(app): State<Arc<App>>, Json(body): Json<RoomBody>) -> StatusCode {
    app.client.join_room(&body.room);
    StatusCode::ACCEPTED
}

async fn leave_room(State(app): State<Arc<App>>, Json(body): Json<RoomBody>) -> StatusCode {
    app.client.leave_room(&body.room);
    StatusCode::ACCEPTED
}

async fn room_history(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let messages = db::load_chat(&app.db, "room", &room, query.limit).await;
    let users = {
        let data = app.data.read().unwrap();
        data.rooms.joined.get(&room).map(|view| view.users.clone()).unwrap_or_default()
    };
    Json(json!({ "room": room, "messages": messages, "users": users }))
}

async fn say_room(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Json(body): Json<MessageBody>,
) -> StatusCode {
    app.client.say_room(&room, &body.message);
    StatusCode::ACCEPTED
}

async fn interests(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "interests": &data.interests }))
}

#[derive(Deserialize)]
struct InterestBody {
    kind: String,
    thing: String,
}

async fn add_interest(
    State(app): State<Arc<App>>,
    Json(body): Json<InterestBody>,
) -> StatusCode {
    match body.kind.as_str() {
        "liked" => app.client.add_liked_interest(&body.thing),
        "hated" => app.client.add_hated_interest(&body.thing),
        _ => return StatusCode::BAD_REQUEST,
    }
    db::add_interest(&app.db, &body.kind, &body.thing).await;
    let mut data = app.data.write().unwrap();
    let list = if body.kind == "liked" {
        &mut data.interests.liked
    } else {
        &mut data.interests.hated
    };
    if !list.contains(&body.thing) {
        list.push(body.thing);
        list.sort();
    }
    app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
    StatusCode::ACCEPTED
}

async fn remove_interest(
    State(app): State<Arc<App>>,
    Json(body): Json<InterestBody>,
) -> StatusCode {
    match body.kind.as_str() {
        "liked" => app.client.remove_liked_interest(&body.thing),
        "hated" => app.client.remove_hated_interest(&body.thing),
        _ => return StatusCode::BAD_REQUEST,
    }
    db::remove_interest(&app.db, &body.kind, &body.thing).await;
    let mut data = app.data.write().unwrap();
    let list = if body.kind == "liked" {
        &mut data.interests.liked
    } else {
        &mut data.interests.hated
    };
    list.retain(|thing| thing != &body.thing);
    app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
    StatusCode::ACCEPTED
}

async fn refresh_interests(State(app): State<Arc<App>>) -> StatusCode {
    let no_interests = {
        let data = app.data.read().unwrap();
        data.interests.liked.is_empty() && data.interests.hated.is_empty()
    };
    if no_interests {
        app.client
            .server_request(newkitine::protocol::ServerRequest::GlobalRecommendations);
    } else {
        app.client.request_recommendations();
    }
    app.client.server_request(newkitine::protocol::ServerRequest::SimilarUsers);
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct ItemBody {
    thing: String,
}

async fn item_recommendations(
    State(app): State<Arc<App>>,
    Json(body): Json<ItemBody>,
) -> StatusCode {
    app.client.server_request(newkitine::protocol::ServerRequest::ItemRecommendations {
        thing: body.thing.clone(),
    });
    app.client
        .server_request(newkitine::protocol::ServerRequest::ItemSimilarUsers { thing: body.thing });
    StatusCode::ACCEPTED
}

async fn rescan_shares(State(app): State<Arc<App>>) -> StatusCode {
    app.client.rescan_shares();
    StatusCode::ACCEPTED
}

async fn get_settings(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(json!({
        "settings": app.effective_settings(),
        "locked": app.locked_settings,
        "gluetun": app.gluetun_enabled,
    }))
}

fn locked_change(locked: &[&'static str], old: &Settings, new: &Settings) -> Option<&'static str> {
    locked
        .iter()
        .find(|key| match **key {
            "server" => old.server != new.server,
            "username" => old.username != new.username,
            "password" => old.password != new.password,
            "listen_port" => old.listen_port != new.listen_port,
            "download_dir" => old.download_dir != new.download_dir,
            key => unreachable!("unknown locked settings key {key}"),
        })
        .copied()
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Json(body): Json<Settings>,
) -> impl IntoResponse {
    let old = app.effective_settings();
    if let Some(key) = locked_change(&app.locked_settings, &old, &body) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{key} is set by the environment") })),
        )
            .into_response();
    }
    if app.gluetun_enabled && body.listen_port != old.listen_port {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "listen_port is managed by gluetun" })),
        )
            .into_response();
    }
    let login_changed = body.server != old.server
        || body.username != old.username
        || body.password != old.password;
    if login_changed {
        let server = match body.resolve_server() {
            Ok(server) => server,
            Err(error) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": error })))
                    .into_response();
            }
        };
        app.client.set_login(server, &body.username, &body.password);
    }
    if body.listen_port != old.listen_port {
        app.client.set_listen_port(body.listen_port);
    }
    if body.description != old.description {
        app.client.set_description(&body.description);
    }
    if body.shares != old.shares {
        app.client.set_shared_folders(body.shares.clone());
    }
    if body.upload_slots != old.upload_slots || body.queue_file_limit != old.queue_file_limit {
        app.client.set_upload_slots(body.upload_slots, body.queue_file_limit);
    }
    if body.download_dir != old.download_dir || body.incomplete_dir() != old.incomplete_dir() {
        app.client.set_download_dirs(body.download_dir.clone(), body.incomplete_dir());
    }
    if body.upload_limit_kbps != old.upload_limit_kbps
        || body.download_limit_kbps != old.download_limit_kbps
    {
        app.client.set_transfer_limits(body.upload_limit_kbps, body.download_limit_kbps);
    }
    if body.auto_reconnect != old.auto_reconnect {
        app.client.set_auto_reconnect(body.auto_reconnect);
    }

    let stored = {
        let mut settings = app.settings.write().unwrap();
        let mut stored = body.clone();
        for key in &app.locked_settings {
            match *key {
                "server" => stored.server = settings.server.clone(),
                "username" => stored.username = settings.username.clone(),
                "password" => stored.password = settings.password.clone(),
                "listen_port" => stored.listen_port = settings.listen_port,
                "download_dir" => stored.download_dir = settings.download_dir.clone(),
                key => unreachable!("unknown locked settings key {key}"),
            }
        }
        if app.gluetun_enabled {
            stored.listen_port = settings.listen_port;
        }
        *settings = stored.clone();
        stored
    };
    db::save_settings(&app.db, &serde_json::to_string(&stored).unwrap()).await;

    {
        let mut data = app.data.write().unwrap();
        data.status.server = body.server.clone();
        data.status.listen_port = body.listen_port;
        if !data.status.logged_in {
            data.status.username = body.username.clone();
        }
        app.broadcast(json!({ "type": "status", "status": &data.status }));
    }
    app.broadcast(json!({
        "type": "settings",
        "settings": &body,
        "locked": app.locked_settings,
        "gluetun": app.gluetun_enabled,
    }));
    StatusCode::ACCEPTED.into_response()
}
