use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;
use sqlx::Row;

use super::api;
use super::behavior;
use super::contract::AppEvent;
use super::db;
use super::projection::{ProjectionWriter, set_member};
use super::state::{App, now};
use crate::types::USERNAME_LIMIT;

#[derive(Clone, serde::Serialize)]
pub struct ChatMessage {
    pub id: u64,
    pub sender: String,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Default, serde::Serialize)]
pub struct RoomsView {
    pub available: Vec<RoomEntry>,
    pub joined: HashMap<String, RoomView>,
}

#[derive(Clone, serde::Serialize)]
pub struct RoomEntry {
    pub name: String,
    pub users: u32,
}

#[derive(Default, Clone, serde::Serialize)]
pub struct RoomView {
    pub users: Vec<String>,
}

#[derive(Default)]
pub struct Chat {
    partners: Vec<String>,
    rooms: RoomsView,
}

impl Chat {
    pub fn load(partners: Vec<String>) -> Self {
        Self {
            partners,
            ..Default::default()
        }
    }

    pub fn partners(&self) -> &[String] {
        &self.partners
    }

    pub fn rooms(&self) -> &RoomsView {
        &self.rooms
    }
}

const CHAT_MESSAGE_LIMIT: usize = 16 * 1024;

async fn insert_chat(
    executor: impl sqlx::MySqlExecutor<'_>,
    kind: &str,
    target: &str,
    sender: String,
    mut message: String,
    timestamp: i64,
) -> Result<ChatMessage, sqlx::Error> {
    message.truncate(message.floor_char_boundary(CHAT_MESSAGE_LIMIT));
    let result = sqlx::query(
        "INSERT INTO chat_messages (kind, target, sender, message, timestamp) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(kind)
    .bind(target)
    .bind(&sender)
    .bind(&message)
    .bind(timestamp)
    .execute(executor)
    .await?;
    Ok(ChatMessage {
        id: result.last_insert_id(),
        sender,
        message,
        timestamp,
    })
}

fn outgoing_text(message: &str) -> String {
    message.replace('\r', "").replace('\n', " ")
}

async fn load_chat(pool: &MySqlPool, kind: &str, target: &str, limit: u32) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = sqlx::query(
        "SELECT id, sender, message, timestamp FROM chat_messages
         WHERE kind = ? AND target = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(kind)
    .bind(target)
    .bind(limit)
    .fetch_all(pool)
    .await
    .expect("load chat")
    .into_iter()
    .map(|row| ChatMessage {
        id: row.get(0),
        sender: row.get(1),
        message: row.get(2),
        timestamp: row.get(3),
    })
    .collect();
    messages.reverse();
    messages
}

fn record_private_message(app: &App, username: &str, message: ChatMessage) {
    let mut data = app.projection.write();
    if !data.chat.partners.iter().any(|user| user == username) {
        data.chat.partners.push(username.to_owned());
    }
    data.broadcast(AppEvent::PrivateMessage {
        username: username.to_owned(),
        message,
    });
}

async fn store_private_message(
    app: &App,
    partner: &str,
    sender: String,
    text: String,
    timestamp: i64,
) -> Result<ChatMessage, sqlx::Error> {
    let mut tx = app.db.begin().await?;
    let message = insert_chat(&mut *tx, "private", partner, sender, text, timestamp).await?;
    db::add_to_list(&mut *tx, "chat", partner).await?;
    tx.commit().await?;
    Ok(message)
}

pub async fn private_message_received(
    app: &Arc<App>,
    username: String,
    message: String,
    timestamp: u32,
) {
    let _mutation = app.list_mutation.lock().await;
    let sender = username.clone();
    let message = store_private_message(app, &username, sender, message, timestamp as i64)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    if let Some(keys) = app.settings.pushover_private_messages() {
        app.pushover.notify(
            keys,
            &format!("Private message from {username}"),
            &message.message,
        );
    }
    record_private_message(app, &username, message);
    behavior::message_received(app, &username).await;
}

pub async fn room_message_received(app: &App, room: String, username: String, message: String) {
    if !app.projection.read().chat.rooms.joined.contains_key(&room) {
        tracing::debug!(
            room,
            username,
            "message for a room we have not joined, dropping"
        );
        return;
    }
    let message = insert_chat(&app.db, "room", &room, username, message, now())
        .await
        .unwrap_or_else(|error| db::fatal(error));
    app.projection
        .write()
        .broadcast(AppEvent::RoomMessage { room, message });
}

pub fn room_list(app: &App, rooms: Vec<(String, u32)>) {
    let mut data = app.projection.write();
    data.chat.rooms.available = rooms
        .into_iter()
        .map(|(name, users)| RoomEntry { name, users })
        .collect();
    data.chat
        .rooms
        .available
        .sort_by_key(|room| std::cmp::Reverse(room.users));
    let event = AppEvent::RoomList {
        rooms: data.chat.rooms.available.clone(),
    };
    data.broadcast(event);
}

pub async fn room_joined(app: &App, room: String, users: Vec<String>) {
    let _mutation = app.list_mutation.lock().await;
    db::add_to_list(&app.db, "room", &room)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    let mut data = app.projection.write();
    let mut usernames = users;
    usernames.sort();
    let view = RoomView { users: usernames };
    data.broadcast(AppEvent::RoomJoined {
        room: room.clone(),
        users: view.users.clone(),
    });
    data.chat.rooms.joined.insert(room, view);
}

pub async fn room_left(app: &App, room: String) {
    let _mutation = app.list_mutation.lock().await;
    db::remove_from_list(&app.db, "room", &room)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    let mut data = app.projection.write();
    data.chat.rooms.joined.remove(&room);
    data.broadcast(AppEvent::RoomLeft { room });
}

pub fn room_user(app: &App, room: String, username: String, joined: bool) {
    let mut data = app.projection.write();
    let Some(view) = data.chat.rooms.joined.get_mut(&room) else {
        tracing::debug!(
            room,
            username,
            joined,
            "membership change in a room we have not joined, dropping"
        );
        return;
    };
    set_member(&mut view.users, &username, joined);
    data.broadcast(if joined {
        AppEvent::RoomUserJoined { room, username }
    } else {
        AppEvent::RoomUserLeft { room, username }
    });
}

pub fn server_disconnected(data: &mut ProjectionWriter<'_>) {
    let rooms: Vec<String> = data.chat.rooms.joined.keys().cloned().collect();
    for room in rooms {
        data.chat.rooms.joined.remove(&room);
        data.broadcast(AppEvent::RoomLeft { room });
    }
    data.chat.rooms.available.clear();
    data.broadcast(AppEvent::RoomList { rooms: Vec::new() });
}

pub(super) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/chats", get(list_chats))
        .route(
            "/api/chats/{username}",
            get(chat_history)
                .post(send_private_message)
                .delete(close_chat),
        )
        .route("/api/chats/{username}/open", post(open_chat))
        .route("/api/rooms", get(rooms))
        .route("/api/rooms/join", post(join_room))
        .route("/api/rooms/leave", post(leave_room))
        .route(
            "/api/rooms/{room}/messages",
            get(room_history).post(say_room),
        )
}

const HISTORY_LIMIT_MAX: u32 = 1000;

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    100
}

async fn list_chats(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "chats": data.chat.partners() }))
}

async fn open_chat(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    db::add_to_list(&app.db, "chat", &username)
        .await
        .map_err(api::db_failed)?;
    let mut data = app.projection.write();
    if !data.chat.partners.contains(&username) {
        data.chat.partners.push(username.clone());
    }
    data.broadcast(AppEvent::ChatOpened { username });
    Ok(StatusCode::ACCEPTED)
}

async fn close_chat(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    db::remove_from_list(&app.db, "chat", &username)
        .await
        .map_err(api::db_failed)?;
    let mut data = app.projection.write();
    data.chat.partners.retain(|user| user != &username);
    data.broadcast(AppEvent::ChatClosed { username });
    Ok(StatusCode::ACCEPTED)
}

async fn chat_history(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    if query.limit > HISTORY_LIMIT_MAX {
        return api::limit_rejected(query.limit, HISTORY_LIMIT_MAX);
    }
    let messages = load_chat(&app.db, "private", &username, query.limit).await;
    Json(json!({ "username": username, "messages": messages })).into_response()
}

#[derive(Deserialize)]
struct MessageBody {
    message: String,
}

async fn send_private_message(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<MessageBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    if body.message.len() > CHAT_MESSAGE_LIMIT {
        return Err(StatusCode::BAD_REQUEST);
    }
    let _mutation = app.list_mutation.lock().await;
    let sender = app.projection.read().session.username.clone();
    let text = outgoing_text(&body.message);
    let message = store_private_message(&app, &username, sender, text, now())
        .await
        .map_err(api::db_failed)?;
    app.client
        .send_private_message(&username, &message.message)
        .await;
    record_private_message(&app, &username, message);
    Ok(StatusCode::ACCEPTED)
}

async fn rooms(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "rooms": data.chat.rooms() }))
}

#[derive(Deserialize)]
struct RoomBody {
    room: String,
}

async fn join_room(
    State(app): State<Arc<App>>,
    Json(body): Json<RoomBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    if body.room.len() > USERNAME_LIMIT {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.client.join_room(&body.room).await;
    Ok(StatusCode::ACCEPTED)
}

async fn leave_room(
    State(app): State<Arc<App>>,
    Json(body): Json<RoomBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    let _mutation = app.list_mutation.lock().await;
    db::remove_from_list(&app.db, "room", &body.room)
        .await
        .map_err(api::db_failed)?;
    app.client.leave_room(&body.room).await;
    Ok(StatusCode::ACCEPTED)
}

async fn room_history(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    if query.limit > HISTORY_LIMIT_MAX {
        return api::limit_rejected(query.limit, HISTORY_LIMIT_MAX);
    }
    let messages = load_chat(&app.db, "room", &room, query.limit).await;
    let users = {
        let data = app.projection.read();
        data.chat
            .rooms
            .joined
            .get(&room)
            .map(|view| view.users.clone())
            .unwrap_or_default()
    };
    Json(json!({ "room": room, "messages": messages, "users": users })).into_response()
}

async fn say_room(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Json(body): Json<MessageBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    if body.message.len() > CHAT_MESSAGE_LIMIT {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.client
        .say_room(&room, &outgoing_text(&body.message))
        .await;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::outgoing_text;

    #[test]
    fn outgoing_text_drops_carriage_returns_and_flattens_newlines() {
        assert_eq!(outgoing_text("one\r\ntwo\nthree\r"), "one two three");
        assert_eq!(outgoing_text("plain"), "plain");
    }
}
