use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sqlx::MySqlPool;
use tokio::sync::broadcast;

use newkitine::client::Client;
use newkitine::types::{FileAttributes, FolderContents, SimilarUser, UserStats};

use super::settings::Settings;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

pub struct App {
    pub client: Client,
    pub db: MySqlPool,
    pub data: RwLock<AppData>,
    pub events: broadcast::Sender<String>,
    pub settings: RwLock<Settings>,
    pub locked_settings: Vec<&'static str>,
    pub gluetun_enabled: bool,
}

impl App {
    pub fn broadcast(&self, event: serde_json::Value) {
        let _ = self.events.send(event.to_string());
    }

    pub fn effective_settings(&self) -> Settings {
        let mut settings = self.settings.read().unwrap().clone();
        settings.apply_env();
        if self.gluetun_enabled {
            settings.listen_port = self.data.read().unwrap().status.listen_port;
        }
        settings
    }
}

#[derive(Default)]
pub struct AppData {
    pub status: Status,
    pub downloads: HashMap<(String, String), TransferView>,
    pub uploads: HashMap<(String, String), TransferView>,
    pub searches: Vec<SearchView>,
    pub browses: HashMap<String, BrowseView>,
    pub user_infos: HashMap<String, UserInfoView>,
    pub private_chats: HashMap<String, Vec<ChatMessage>>,
    pub rooms: RoomsView,
    pub buddies: HashMap<String, BuddyView>,
    pub banned: Vec<String>,
    pub ignored: Vec<String>,
    pub wishlist: Vec<String>,
    pub interests: InterestsView,
}

#[derive(Default, Clone, Serialize)]
pub struct Status {
    pub connected: bool,
    pub logged_in: bool,
    pub username: String,
    pub server: String,
    pub banner: String,
    pub listen_port: u16,
    pub shared_folders: u32,
    pub shared_files: u32,
    pub privileges_secs: u32,
}

#[derive(Clone, Serialize)]
pub struct TransferView {
    pub username: String,
    pub virtual_path: String,
    pub size: u64,
    pub bytes_done: u64,
    pub status: String,
    pub file_path: Option<String>,
    pub queue_place: u32,
    pub attributes: FileAttributes,
    pub updated_at: i64,
}

#[derive(Serialize)]
pub struct SearchView {
    pub token: u32,
    pub query: String,
    pub results: Vec<SearchResponseView>,
}

#[derive(Clone, Serialize)]
pub struct SearchResponseView {
    pub username: String,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
    pub files: Vec<SearchFileView>,
}

#[derive(Clone, Serialize)]
pub struct SearchFileView {
    pub name: String,
    pub size: u64,
    pub attributes: FileAttributes,
}

#[derive(Serialize)]
pub struct BrowseView {
    pub username: String,
    pub folders: Vec<FolderContents>,
    pub private_folders: Vec<FolderContents>,
    pub received_at: i64,
}

#[derive(Default, Clone, Serialize)]
pub struct UserInfoView {
    pub username: String,
    pub received: bool,
    pub description: String,
    pub picture_base64: Option<String>,
    pub upload_slots: u32,
    pub queue_size: u32,
    pub slots_available: bool,
    pub stats: Option<UserStats>,
    pub interests_liked: Vec<String>,
    pub interests_hated: Vec<String>,
}

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

#[derive(Default, Clone, Serialize)]
pub struct BuddyView {
    pub username: String,
    pub status: String,
    pub privileged: bool,
    pub stats: UserStats,
    pub note: String,
}

#[derive(Default, Serialize)]
pub struct InterestsView {
    pub liked: Vec<String>,
    pub hated: Vec<String>,
    pub recommendations: Vec<(String, i32)>,
    pub unrecommendations: Vec<(String, i32)>,
    pub recommendations_for: Option<String>,
    pub recommendations_global: bool,
    pub similar_users: Vec<SimilarUser>,
    pub similar_users_for: Option<String>,
}
