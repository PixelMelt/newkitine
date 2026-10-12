use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::db::set_note;
use super::{Block, browse, open_info, publish_buddy};
use crate::app::api;
use crate::app::behavior;
use crate::app::contract::AppEvent;
use crate::app::db;
use crate::app::peer_history::last_ip;
use crate::app::state::App;

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/users/{username}/browse", post(browse_user))
        .route("/api/users/{username}/folders", get(user_folders))
        .route("/api/users/{username}/tree", get(user_tree))
        .route("/api/users/{username}/files", get(user_files))
        .route(
            "/api/users/{username}/info",
            get(get_user_info).post(request_user_info),
        )
        .route("/api/users/{username}/ban_ip", post(ban_user_ip))
        .route("/api/buddies", get(list_buddies).post(add_buddy))
        .route("/api/buddies/{username}", delete(remove_buddy))
        .route("/api/buddies/{username}/note", post(set_buddy_note))
        .route(
            "/api/banned",
            get(list_banned).post(|app, Json(body): Json<UserBody>| {
                set_blocked(app, Block::Banned, body.username, true)
            }),
        )
        .route(
            "/api/banned/{username}",
            delete(|app, Path(username)| set_blocked(app, Block::Banned, username, false)),
        )
        .route(
            "/api/ignored",
            get(list_ignored).post(|app, Json(body): Json<UserBody>| {
                set_blocked(app, Block::Ignored, body.username, true)
            }),
        )
        .route(
            "/api/ignored/{username}",
            delete(|app, Path(username)| set_blocked(app, Block::Ignored, username, false)),
        )
        .route(
            "/api/ip_bans",
            get(list_ip_bans).post(|app, body| set_ip_ban(app, body, true)),
        )
        .route(
            "/api/ip_bans/remove",
            post(|app, body| set_ip_ban(app, body, false)),
        )
}

async fn browse_user(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    app.client.browse_user(&username).await;
    Ok(StatusCode::ACCEPTED)
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

const FOLDER_LIMIT_MAX: usize = 5000;

async fn user_folders(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    if query.limit > FOLDER_LIMIT_MAX {
        return api::limit_rejected(query.limit as u32, FOLDER_LIMIT_MAX as u32);
    }
    let Some(browse) = browse(&app, &username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let filter = query.filter.to_lowercase();
    let matching = browse
        .all()
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
struct TreeQuery {
    #[serde(default)]
    dir: String,
}

async fn user_tree(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<TreeQuery>,
) -> impl IntoResponse {
    let Some(browse) = browse(&app, &username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (folders, private_folders) = (&browse.folders, &browse.private_folders);
    let prefix = if query.dir.is_empty() {
        String::new()
    } else {
        format!("{}\\", query.dir)
    };
    let mut children: std::collections::BTreeMap<&str, (usize, bool, bool)> =
        std::collections::BTreeMap::new();
    for (folder, private) in folders
        .iter()
        .map(|folder| (folder, false))
        .chain(private_folders.iter().map(|folder| (folder, true)))
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
        let (mut files, mut size) = (0u64, 0u64);
        for folder in browse.all() {
            files += folder.files.len() as u64;
            size = folder
                .files
                .iter()
                .fold(size, |total, file| total.saturating_add(file.size));
        }
        response["summary"] = json!({
            "folders": folders.len() + private_folders.len(),
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
    let Some(browse) = browse(&app, &username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match browse.all().find(|folder| folder.directory == query.dir) {
        Some(folder) => {
            Json(json!({ "directory": folder.directory, "files": folder.files })).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn request_user_info(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    open_info(&app, &username, |_| {});
    app.client.request_user_info(&username).await;
    app.client.request_user_stats(&username).await;
    app.client.request_user_interests(&username).await;
    Ok(StatusCode::ACCEPTED)
}

async fn get_user_info(
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
struct UserBody {
    username: String,
}

async fn list_buddies(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    let mut buddies: Vec<_> = data.users.buddies().collect();
    buddies.sort_by(|a, b| a.username.cmp(&b.username));
    Json(json!({ "buddies": buddies }))
}

async fn add_buddy(
    State(app): State<Arc<App>>,
    Json(body): Json<UserBody>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    db::add_to_list(&app.db, "buddy", &body.username)
        .await
        .map_err(api::db_failed)?;
    app.client.add_buddy(&body.username).await;
    {
        let mut data = app.projection.write();
        let buddy = data.users.insert_buddy(&body.username);
        data.broadcast(AppEvent::Buddy { buddy });
    }
    behavior::buddy_added(&app, &body.username).await;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct NoteBody {
    note: String,
}

const NOTE_LIMIT: usize = 16 * 1024;

async fn set_buddy_note(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<NoteBody>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    if !app.projection.read().users.is_buddy(&username) {
        return Err(StatusCode::NOT_FOUND);
    }
    if body.note.len() > NOTE_LIMIT {
        return Err(StatusCode::BAD_REQUEST);
    }
    set_note(&app.db, &username, &body.note)
        .await
        .map_err(api::db_failed)?;
    publish_buddy(&app, &username, |buddy| buddy.note = body.note);
    Ok(StatusCode::ACCEPTED)
}

async fn remove_buddy(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    let mut tx = app.db.begin().await.map_err(api::db_failed)?;
    db::remove_from_list(&mut *tx, "buddy", &username)
        .await
        .map_err(api::db_failed)?;
    set_note(&mut *tx, &username, "")
        .await
        .map_err(api::db_failed)?;
    tx.commit().await.map_err(api::db_failed)?;
    app.client.remove_buddy(&username).await;
    let mut data = app.projection.write();
    data.users.remove_buddy(&username);
    data.broadcast(AppEvent::BuddyRemoved { username });
    Ok(StatusCode::ACCEPTED)
}

async fn list_banned(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "users": data.users.banned() }))
}

async fn list_ignored(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "users": data.users.ignored() }))
}

async fn set_blocked(
    State(app): State<Arc<App>>,
    block: Block,
    username: String,
    blocked: bool,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    let list = match block {
        Block::Banned => "banned",
        Block::Ignored => "ignored",
    };
    if blocked {
        db::add_to_list(&app.db, list, &username).await
    } else {
        db::remove_from_list(&app.db, list, &username).await
    }
    .map_err(api::db_failed)?;
    match (block, blocked) {
        (Block::Banned, true) => app.client.ban_user(&username).await,
        (Block::Banned, false) => app.client.unban_user(&username).await,
        (Block::Ignored, true) => app.client.ignore_user(&username).await,
        (Block::Ignored, false) => app.client.unignore_user(&username).await,
    }
    let mut data = app.projection.write();
    let users = data.users.set_blocked(block, &username, blocked);
    data.broadcast(match block {
        Block::Banned => AppEvent::Banned { users },
        Block::Ignored => AppEvent::Ignored { users },
    });
    Ok(StatusCode::ACCEPTED)
}

async fn ban_user_ip(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    let ip = last_ip(&app.db, &username)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    db::add_to_list(&app.db, "ip_ban", &ip)
        .await
        .map_err(api::db_failed)?;
    push_ip_bans(&app).await;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct IpBanBody {
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

async fn list_ip_bans(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(json!({ "patterns": db::load_list(&app.db, "ip_ban").await }))
}

async fn set_ip_ban(
    State(app): State<Arc<App>>,
    Json(body): Json<IpBanBody>,
    banned: bool,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    let pattern = canonical_ip_pattern(&body.pattern).ok_or(StatusCode::BAD_REQUEST)?;
    if banned {
        db::add_to_list(&app.db, "ip_ban", &pattern).await
    } else {
        db::remove_from_list(&app.db, "ip_ban", &pattern).await
    }
    .map_err(api::db_failed)?;
    push_ip_bans(&app).await;
    Ok(StatusCode::ACCEPTED)
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
