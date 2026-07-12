#[cfg(test)]
mod check;

use std::collections::HashMap;

use serde::Serialize;

use crate::types::{
    BuddyView, ChatMessage, InterestsView, PublicSettings, RoomEntry, RoomsView,
    SearchResponseView, SearchView, Status, TransferDirection, TransferId, TransferView,
    UserInfoView,
};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    Status {
        status: Status,
    },
    ConnCount {
        count: usize,
    },
    LoginFailed {
        reason: String,
        detail: Option<String>,
    },
    ServerMessage {
        message: String,
    },
    Settings(SettingsPayload),
    Transfer {
        direction: TransferDirection,
        transfer: TransferView,
    },
    TransfersRemoved {
        direction: TransferDirection,
        ids: Vec<TransferId>,
    },
    SearchAdded {
        search: SearchView,
    },
    SearchResults {
        token: u32,
        response: SearchResponseView,
    },
    SearchRemoved {
        token: u32,
    },
    Wishlist {
        wishlist: Vec<String>,
    },
    Interests {
        interests: InterestsView,
    },
    Buddy {
        buddy: BuddyView,
    },
    BuddyRemoved {
        username: String,
    },
    Banned {
        users: Vec<String>,
    },
    Ignored {
        users: Vec<String>,
    },
    UserInfo {
        info: UserInfoView,
    },
    BrowseLoaded {
        username: String,
        received_at: i64,
    },
    PrivateMessage {
        username: String,
        message: ChatMessage,
    },
    ChatOpened {
        username: String,
    },
    ChatClosed {
        username: String,
    },
    RoomMessage {
        room: String,
        message: ChatMessage,
    },
    RoomList {
        rooms: Vec<RoomEntry>,
    },
    RoomJoined {
        room: String,
        users: Vec<String>,
    },
    RoomLeft {
        room: String,
    },
    RoomUserJoined {
        room: String,
        username: String,
    },
    RoomUserLeft {
        room: String,
        username: String,
    },
}

#[derive(Serialize)]
pub struct Envelope<'a> {
    pub rev: u64,
    #[serde(flatten)]
    pub event: &'a AppEvent,
}

#[derive(Serialize)]
pub struct SettingsPayload {
    pub settings: PublicSettings,
    pub locked: Vec<&'static str>,
    pub gluetun: bool,
}

#[derive(Serialize)]
pub struct Snapshot<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub rev: u64,
    pub status: &'a Status,
    pub downloads: Vec<&'a TransferView>,
    pub uploads: Vec<&'a TransferView>,
    pub searches: &'a [SearchView],
    pub buddies: Vec<&'a BuddyView>,
    pub banned: &'a [String],
    pub ignored: &'a [String],
    pub wishlist: &'a [String],
    pub interests: &'a InterestsView,
    pub rooms: &'a RoomsView,
    pub chat_partners: &'a [String],
    pub browses: HashMap<&'a String, i64>,
    pub settings: SettingsPayload,
}
