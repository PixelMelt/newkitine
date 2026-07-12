use std::collections::HashMap;

use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct ChatMessage {
    pub sender: String,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Default, Serialize)]
pub struct RoomsView {
    pub available: Vec<RoomEntry>,
    pub joined: HashMap<String, RoomView>,
}

#[derive(Clone, Serialize)]
pub struct RoomEntry {
    pub name: String,
    pub users: u32,
}

#[derive(Default, Clone, Serialize)]
pub struct RoomView {
    pub users: Vec<String>,
    pub messages: Vec<ChatMessage>,
}
