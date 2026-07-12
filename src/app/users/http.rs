use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use super::db::{last_ip, set_note};
use crate::app::api;
use crate::app::behavior;
use crate::app::contract::AppEvent;
use crate::app::db;
use crate::app::state::App;

pub async fn browse_user(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    app.client.browse_user(&username).await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct FoldersQuery {
    #[serde(default)]
    filter: String,
    #[serde(default = "default_folder_limit")]
    limit: usize,
}

fn default_folder_limit() -> usize {
    500
}

pub async fn user_folders(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    let data = app.projection.read();
    let Some(browse) = data.users.browse(&username) else {
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
        .map(|folder| json!({ "directory": folder.directory, "file_count": folder.files.len() }))
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
pub struct TreeQuery {
    #[serde(default)]
    dir: String,
}

pub async fn user_tree(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<TreeQuery>,
) -> impl IntoResponse {
    let data = app.projection.read();
    let Some(browse) = data.users.browse(&username) else {
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
        let Some(rest) = folder.directory.strip_prefix(&prefix) else {
            continue;
        };
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
pub struct FilesQuery {
    dir: String,
}

pub async fn user_files(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    let data = app.projection.read();
    let Some(browse) = data.users.browse(&username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match browse
        .folders
        .iter()
        .chain(browse.private_folders.iter())
        .find(|folder| folder.directory == query.dir)
    {
        Some(folder) => {
            Json(json!({ "directory": folder.directory, "files": folder.files })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn request_user_info(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> StatusCode {
    app.client.request_user_info(&username).await;
    app.client.request_user_stats(&username).await;
    app.client.request_user_interests(&username).await;
    let mut data = app.projection.write();
    let info = data.users.ensure_info(&username);
    data.broadcast(AppEvent::UserInfo { info });
    StatusCode::ACCEPTED
}

pub async fn get_user_info(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> impl IntoResponse {
    let data = app.projection.read();
    match data.users.info(&username) {
        Some(info) => Json(serde_json::to_value(info).unwrap()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
pub struct UserBody {
    username: String,
}

pub async fn list_buddies(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    let mut buddies: Vec<_> = data.users.buddies().collect();
    buddies.sort_by(|a, b| a.username.cmp(&b.username));
    Json(json!({ "buddies": buddies }))
}

pub async fn add_buddy(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    if let Err(error) = db::add_to_list(&app.db, "buddy", &body.username).await {
        return api::db_failed(error);
    }
    app.client.add_buddy(&body.username).await;
    {
        let mut data = app.projection.write();
        let buddy = data.users.insert_buddy(&body.username);
        data.broadcast(AppEvent::Buddy { buddy });
    }
    behavior::buddy_added(&app, &body.username).await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct NoteBody {
    note: String,
}

pub async fn set_buddy_note(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<NoteBody>,
) -> StatusCode {
    if !app.projection.read().users.is_buddy(&username) {
        return StatusCode::NOT_FOUND;
    }
    if let Err(error) = set_note(&app.db, &username, &body.note).await {
        return api::db_failed(error);
    }
    let mut data = app.projection.write();
    let Some(buddy) = data.users.set_note(&username, body.note) else {
        return StatusCode::NOT_FOUND;
    };
    data.broadcast(AppEvent::Buddy { buddy });
    StatusCode::ACCEPTED
}

pub async fn remove_buddy(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    if let Err(error) = db::remove_from_list(&app.db, "buddy", &username).await {
        return api::db_failed(error);
    }
    if let Err(error) = set_note(&app.db, &username, "").await {
        return api::db_failed(error);
    }
    app.client.remove_buddy(&username).await;
    let mut data = app.projection.write();
    data.users.remove_buddy(&username);
    data.broadcast(AppEvent::BuddyRemoved { username });
    StatusCode::ACCEPTED
}

pub async fn list_banned(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "users": data.users.banned() }))
}

pub async fn ban_user(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    if let Err(error) = db::add_to_list(&app.db, "banned", &body.username).await {
        return api::db_failed(error);
    }
    app.client.ban_user(&body.username).await;
    let mut data = app.projection.write();
    let users = data.users.ban(body.username);
    data.broadcast(AppEvent::Banned { users });
    StatusCode::ACCEPTED
}

pub async fn unban_user(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    if let Err(error) = db::remove_from_list(&app.db, "banned", &username).await {
        return api::db_failed(error);
    }
    app.client.unban_user(&username).await;
    let mut data = app.projection.write();
    let users = data.users.unban(&username);
    data.broadcast(AppEvent::Banned { users });
    StatusCode::ACCEPTED
}

pub async fn list_ignored(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "users": data.users.ignored() }))
}

pub async fn ignore_user(State(app): State<Arc<App>>, Json(body): Json<UserBody>) -> StatusCode {
    if let Err(error) = db::add_to_list(&app.db, "ignored", &body.username).await {
        return api::db_failed(error);
    }
    app.client.ignore_user(&body.username).await;
    let mut data = app.projection.write();
    let users = data.users.ignore(body.username);
    data.broadcast(AppEvent::Ignored { users });
    StatusCode::ACCEPTED
}

pub async fn unignore_user(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> StatusCode {
    if let Err(error) = db::remove_from_list(&app.db, "ignored", &username).await {
        return api::db_failed(error);
    }
    app.client.unignore_user(&username).await;
    let mut data = app.projection.write();
    let users = data.users.unignore(&username);
    data.broadcast(AppEvent::Ignored { users });
    StatusCode::ACCEPTED
}

pub async fn ban_user_ip(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    let Some(ip) = last_ip(&app.db, &username).await else {
        return StatusCode::NOT_FOUND;
    };
    if let Err(error) = db::add_to_list(&app.db, "ip_ban", &ip).await {
        return api::db_failed(error);
    }
    push_ip_bans(&app).await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct IpBanBody {
    pattern: String,
}

fn canonical_ip_pattern(pattern: &str) -> Option<String> {
    let parts: Vec<&str> = pattern.trim().split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut canonical = Vec::with_capacity(4);
    for part in parts {
        if part == "*" {
            canonical.push("*".to_owned());
        } else {
            canonical.push(part.parse::<u8>().ok()?.to_string());
        }
    }
    Some(canonical.join("."))
}

async fn push_ip_bans(app: &App) {
    let patterns = db::load_list(&app.db, "ip_ban").await;
    app.client.set_ip_bans(patterns).await;
}

pub async fn list_ip_bans(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(json!({ "patterns": db::load_list(&app.db, "ip_ban").await }))
}

pub async fn add_ip_ban(State(app): State<Arc<App>>, Json(body): Json<IpBanBody>) -> StatusCode {
    let Some(pattern) = canonical_ip_pattern(&body.pattern) else {
        return StatusCode::BAD_REQUEST;
    };
    if let Err(error) = db::add_to_list(&app.db, "ip_ban", &pattern).await {
        return api::db_failed(error);
    }
    push_ip_bans(&app).await;
    StatusCode::ACCEPTED
}

pub async fn remove_ip_ban(State(app): State<Arc<App>>, Json(body): Json<IpBanBody>) -> StatusCode {
    if let Err(error) = db::remove_from_list(&app.db, "ip_ban", &body.pattern).await {
        return api::db_failed(error);
    }
    push_ip_bans(&app).await;
    StatusCode::ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::canonical_ip_pattern;

    #[test]
    fn ip_patterns_canonicalize() {
        assert_eq!(
            canonical_ip_pattern("10.0.*.*").as_deref(),
            Some("10.0.*.*")
        );
        assert_eq!(
            canonical_ip_pattern(" 192.168.001.5 ").as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(canonical_ip_pattern("300.1.1.1"), None);
        assert_eq!(canonical_ip_pattern("1.2.3"), None);
        assert_eq!(canonical_ip_pattern("1.2.3.4.5"), None);
    }
}
