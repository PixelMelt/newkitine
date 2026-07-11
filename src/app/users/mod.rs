mod db;
mod http;

pub use db::{load_notes, record_user_shares};
pub use http::{
    add_buddy, add_ip_ban, ban_user, ban_user_ip, browse_user, get_user_info, ignore_user,
    list_banned, list_buddies, list_ignored, list_ip_bans, remove_buddy, remove_ip_ban,
    request_user_info, set_buddy_note, unban_user, unignore_user, user_files, user_folders,
    user_tree,
};

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use serde::Serialize;

use crate::types::{FolderContents, UserStats, UserStatus};

use super::behavior;
use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

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

#[derive(Default, Clone, Serialize)]
pub struct BuddyView {
    pub username: String,
    pub status: String,
    pub privileged: bool,
    pub stats: UserStats,
    pub note: String,
}

#[derive(Default)]
pub struct UsersState {
    buddies: HashMap<String, BuddyView>,
    banned: Vec<String>,
    ignored: Vec<String>,
    user_infos: HashMap<String, UserInfoView>,
    browses: HashMap<String, BrowseView>,
}

impl UsersState {
    pub fn load(
        buddies: &[String],
        notes: Vec<(String, String)>,
        banned: Vec<String>,
        ignored: Vec<String>,
    ) -> Self {
        let mut state = Self {
            buddies: buddies
                .iter()
                .map(|username| (username.clone(), buddy_view(username)))
                .collect(),
            banned,
            ignored,
            user_infos: HashMap::new(),
            browses: HashMap::new(),
        };
        for (username, note) in notes {
            if let Some(buddy) = state.buddies.get_mut(&username) {
                buddy.note = note;
            }
        }
        state
    }

    pub fn buddies(&self) -> impl Iterator<Item = &BuddyView> {
        self.buddies.values()
    }

    pub fn is_buddy(&self, username: &str) -> bool {
        self.buddies.contains_key(username)
    }

    pub fn banned(&self) -> &[String] {
        &self.banned
    }

    pub fn ignored(&self) -> &[String] {
        &self.ignored
    }

    pub fn browse(&self, username: &str) -> Option<&BrowseView> {
        self.browses.get(username)
    }

    pub fn browse_timestamps(&self) -> impl Iterator<Item = (&String, i64)> {
        self.browses
            .iter()
            .map(|(username, browse)| (username, browse.received_at))
    }
}

pub fn buddy_view(username: &str) -> BuddyView {
    BuddyView {
        username: username.to_owned(),
        status: "unknown".into(),
        ..Default::default()
    }
}

pub fn status_string(status: Option<UserStatus>) -> &'static str {
    match status {
        Some(UserStatus::Online) => "online",
        Some(UserStatus::Away) => "away",
        Some(UserStatus::Offline) => "offline",
        None => "unknown",
    }
}
pub fn user_status(app: &App, username: String, status: Option<UserStatus>, privileged: bool) {
    let status = status_string(status);
    let mut data = app.data.write().unwrap();
    if let Some(buddy) = data.users.buddies.get_mut(&username) {
        buddy.status = status.into();
        buddy.privileged = privileged;
    }
    app.broadcast(
        &mut data,
        AppEvent::UserStatus {
            username,
            status,
            privileged,
        },
    );
}

pub async fn shared_file_list(
    app: &Arc<App>,
    username: String,
    shares: Vec<FolderContents>,
    private_shares: Vec<FolderContents>,
) {
    let file_count: usize = shares
        .iter()
        .chain(private_shares.iter())
        .map(|folder| folder.files.len())
        .sum();
    behavior::browse_received(app, &username, file_count as u32).await;
    let mut data = app.data.write().unwrap();
    let received_at = now();
    data.users.browses.insert(
        username.clone(),
        BrowseView {
            username: username.clone(),
            folders: shares,
            private_folders: private_shares,
            received_at,
        },
    );
    app.broadcast(
        &mut data,
        AppEvent::BrowseLoaded {
            username,
            received_at,
        },
    );
}

pub struct UserInfoReceived {
    pub username: String,
    pub description: String,
    pub picture: Option<Vec<u8>>,
    pub total_uploads: u32,
    pub queue_size: u32,
    pub slots_available: bool,
}

pub fn user_info_received(app: &App, info: UserInfoReceived) {
    let mut data = app.data.write().unwrap();
    let view = data
        .users
        .user_infos
        .entry(info.username.clone())
        .or_insert_with(|| UserInfoView {
            username: info.username,
            ..Default::default()
        });
    view.received = true;
    view.description = info.description;
    view.picture_base64 = info
        .picture
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
    view.upload_slots = info.total_uploads;
    view.queue_size = info.queue_size;
    view.slots_available = info.slots_available;
    let event = AppEvent::UserInfo { info: view.clone() };
    app.broadcast(&mut data, event);
}

pub async fn watch_user(
    app: &Arc<App>,
    user: String,
    user_exists: bool,
    status: Option<UserStatus>,
    stats: Option<UserStats>,
) {
    if let Some(stats) = &stats {
        record_user_shares(&app.db, &user, stats.files, stats.dirs, now())
            .await
            .unwrap_or_else(|error| fatal(error));
        behavior::stats_received(app, &user, stats.files, stats.dirs).await;
    }
    let mut data = app.data.write().unwrap();
    if let Some(buddy) = data.users.buddies.get_mut(&user) {
        if user_exists {
            buddy.status = status_string(status).into();
        }
        if let Some(stats) = stats {
            buddy.stats = stats;
        }
        let event = AppEvent::Buddy {
            buddy: buddy.clone(),
        };
        app.broadcast(&mut data, event);
    }
}

pub async fn user_stats(app: &Arc<App>, user: String, stats: UserStats) {
    record_user_shares(&app.db, &user, stats.files, stats.dirs, now())
        .await
        .unwrap_or_else(|error| fatal(error));
    behavior::stats_received(app, &user, stats.files, stats.dirs).await;
    let mut data = app.data.write().unwrap();
    if let Some(buddy) = data.users.buddies.get_mut(&user) {
        buddy.stats = stats.clone();
        let event = AppEvent::Buddy {
            buddy: buddy.clone(),
        };
        app.broadcast(&mut data, event);
    }
    if let Some(info) = data.users.user_infos.get_mut(&user) {
        info.stats = Some(stats);
        let event = AppEvent::UserInfo { info: info.clone() };
        app.broadcast(&mut data, event);
    }
}

pub fn user_interests(app: &App, user: String, likes: Vec<String>, hates: Vec<String>) {
    let mut data = app.data.write().unwrap();
    if let Some(info) = data.users.user_infos.get_mut(&user) {
        info.interests_liked = likes;
        info.interests_hated = hates;
        let event = AppEvent::UserInfo { info: info.clone() };
        app.broadcast(&mut data, event);
    }
}
