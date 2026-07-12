use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;
use sqlx::Row;

use crate::types::{ChatMessage, RoomEntry, RoomView, RoomsView};

use super::api;
use super::contract::AppEvent;
use super::db;
use super::state::{App, now};

const MAX_ROOM_MESSAGES: usize = 200;

#[derive(Default)]
pub struct Chat {
    private_chats: HashMap<String, Vec<ChatMessage>>,
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

pub async fn insert_chat(
    pool: &MySqlPool,
    kind: &str,
    target: &str,
    message: &ChatMessage,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO chat_messages (kind, target, sender, message, timestamp) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(kind)
    .bind(target)
    .bind(&message.sender)
    .bind(&message.message)
    .bind(message.timestamp)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_chat(pool: &MySqlPool, kind: &str, target: &str, limit: u32) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = sqlx::query(
        "SELECT sender, message, timestamp FROM chat_messages
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
        sender: row.get(0),
        message: row.get(1),
        timestamp: row.get(2),
    })
    .collect();
    messages.reverse();
    messages
}

fn record_private_message(app: &App, username: &str, message: ChatMessage) {
    let mut data = app.projection.write();
    data.chat
        .private_chats
        .entry(username.to_owned())
        .or_default()
        .push(message.clone());
    if !data.chat.partners.iter().any(|user| user == username) {
        data.chat.partners.push(username.to_owned());
    }
    data.broadcast(AppEvent::PrivateMessage {
        username: username.to_owned(),
        message,
    });
}

pub async fn private_message_received(
    app: &App,
    username: String,
    message: String,
    timestamp: u32,
) {
    let message = ChatMessage {
        sender: username.clone(),
        message,
        timestamp: timestamp as i64,
    };
    insert_chat(&app.db, "private", &username, &message)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    db::add_to_list(&app.db, "chat", &username)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    record_private_message(app, &username, message);
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
    let message = ChatMessage {
        sender: username,
        message,
        timestamp: now(),
    };
    insert_chat(&app.db, "room", &room, &message)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    let mut data = app.projection.write();
    let Some(view) = data.chat.rooms.joined.get_mut(&room) else {
        tracing::debug!(room, "room left while persisting its message, dropping");
        return;
    };
    view.messages.push(message.clone());
    if view.messages.len() > MAX_ROOM_MESSAGES {
        view.messages.remove(0);
    }
    data.broadcast(AppEvent::RoomMessage { room, message });
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
    db::add_to_list(&app.db, "room", &room)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    let mut data = app.projection.write();
    let mut usernames = users;
    usernames.sort();
    let view = RoomView {
        users: usernames,
        messages: Vec::new(),
    };
    data.broadcast(AppEvent::RoomJoined {
        room: room.clone(),
        users: view.users.clone(),
    });
    data.chat.rooms.joined.insert(room, view);
}

pub async fn room_left(app: &App, room: String) {
    db::remove_from_list(&app.db, "room", &room)
        .await
        .unwrap_or_else(|error| db::fatal(error));
    let mut data = app.projection.write();
    data.chat.rooms.joined.remove(&room);
    data.broadcast(AppEvent::RoomLeft { room });
}

pub fn room_user_joined(app: &App, room: String, username: String) {
    let mut data = app.projection.write();
    let Some(view) = data.chat.rooms.joined.get_mut(&room) else {
        tracing::debug!(
            room,
            username,
            "user joined a room we have not joined, dropping"
        );
        return;
    };
    if !view.users.contains(&username) {
        view.users.push(username.clone());
        view.users.sort();
    }
    data.broadcast(AppEvent::RoomUserJoined { room, username });
}

pub fn room_user_left(app: &App, room: String, username: String) {
    let mut data = app.projection.write();
    let Some(view) = data.chat.rooms.joined.get_mut(&room) else {
        tracing::debug!(
            room,
            username,
            "user left a room we have not joined, dropping"
        );
        return;
    };
    view.users.retain(|user| user != &username);
    data.broadcast(AppEvent::RoomUserLeft { room, username });
}

pub fn server_disconnected(app: &App) {
    app.projection.write().chat.rooms.joined.clear();
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    100
}

pub async fn list_chats(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "chats": data.chat.partners() }))
}

pub async fn open_chat(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    if let Err(error) = db::add_to_list(&app.db, "chat", &username).await {
        return api::db_failed(error);
    }
    let mut data = app.projection.write();
    if !data.chat.partners.contains(&username) {
        data.chat.partners.push(username.clone());
    }
    data.broadcast(AppEvent::ChatOpened { username });
    StatusCode::ACCEPTED
}

pub async fn close_chat(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    if let Err(error) = db::remove_from_list(&app.db, "chat", &username).await {
        return api::db_failed(error);
    }
    let mut data = app.projection.write();
    data.chat.partners.retain(|user| user != &username);
    data.broadcast(AppEvent::ChatClosed { username });
    StatusCode::ACCEPTED
}

pub async fn chat_history(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let messages = load_chat(&app.db, "private", &username, query.limit).await;
    Json(json!({ "username": username, "messages": messages }))
}

#[derive(Deserialize)]
pub struct MessageBody {
    message: String,
}

pub async fn send_private_message(
    State(app): State<Arc<App>>,
    Path(username): Path<String>,
    Json(body): Json<MessageBody>,
) -> StatusCode {
    let sender = app.projection.read().session.status().username.clone();
    let message = ChatMessage {
        sender,
        message: body.message,
        timestamp: now(),
    };
    if let Err(error) = insert_chat(&app.db, "private", &username, &message).await {
        return api::db_failed(error);
    }
    if let Err(error) = db::add_to_list(&app.db, "chat", &username).await {
        return api::db_failed(error);
    }
    app.client
        .send_private_message(&username, &message.message)
        .await;
    record_private_message(&app, &username, message);
    StatusCode::ACCEPTED
}

pub async fn rooms(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "rooms": data.chat.rooms() }))
}

#[derive(Deserialize)]
pub struct RoomBody {
    room: String,
}

pub async fn join_room(State(app): State<Arc<App>>, Json(body): Json<RoomBody>) -> StatusCode {
    app.client.join_room(&body.room).await;
    StatusCode::ACCEPTED
}

pub async fn leave_room(State(app): State<Arc<App>>, Json(body): Json<RoomBody>) -> StatusCode {
    app.client.leave_room(&body.room).await;
    StatusCode::ACCEPTED
}

pub async fn room_history(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
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
    Json(json!({ "room": room, "messages": messages, "users": users }))
}

pub async fn say_room(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Json(body): Json<MessageBody>,
) -> StatusCode {
    app.client.say_room(&room, &body.message).await;
    StatusCode::ACCEPTED
}
