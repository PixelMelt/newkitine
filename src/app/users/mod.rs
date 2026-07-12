mod db;
pub mod http;

pub use db::{load_notes, record_user_shares};

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;

use crate::types::{
    BrowseView, BuddyView, FolderContents, UserInfoReceived, UserInfoView, UserStats, UserStatus,
};

use super::behavior;
use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

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
            match state.buddies.get_mut(&username) {
                Some(buddy) => buddy.note = note,
                None => tracing::warn!(username, "note for a user that is not a buddy, ignoring"),
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

    fn insert_browse(&mut self, browse: BrowseView) {
        self.browses.insert(browse.username.clone(), browse);
    }

    fn insert_buddy(&mut self, username: &str) -> BuddyView {
        self.buddies
            .entry(username.to_owned())
            .or_insert_with(|| buddy_view(username))
            .clone()
    }

    fn remove_buddy(&mut self, username: &str) {
        self.buddies.remove(username);
    }

    fn set_note(&mut self, username: &str, note: String) -> Option<BuddyView> {
        let buddy = self.buddies.get_mut(username)?;
        buddy.note = note;
        Some(buddy.clone())
    }

    fn set_status(
        &mut self,
        username: &str,
        status: &'static str,
        privileged: bool,
    ) -> Option<BuddyView> {
        let buddy = self.buddies.get_mut(username)?;
        buddy.status = status.into();
        buddy.privileged = privileged;
        Some(buddy.clone())
    }

    fn apply_watch(
        &mut self,
        username: &str,
        status: Option<&'static str>,
        stats: Option<UserStats>,
    ) -> Option<BuddyView> {
        let buddy = self.buddies.get_mut(username)?;
        if let Some(status) = status {
            buddy.status = status.into();
        }
        if let Some(stats) = stats {
            buddy.stats = stats;
        }
        Some(buddy.clone())
    }

    fn set_buddy_stats(&mut self, username: &str, stats: &UserStats) -> Option<BuddyView> {
        let buddy = self.buddies.get_mut(username)?;
        buddy.stats = stats.clone();
        Some(buddy.clone())
    }

    fn set_info_stats(&mut self, username: &str, stats: UserStats) -> Option<UserInfoView> {
        let info = self.user_infos.get_mut(username)?;
        info.stats = Some(stats);
        Some(info.clone())
    }

    fn set_info_interests(
        &mut self,
        username: &str,
        liked: Vec<String>,
        hated: Vec<String>,
    ) -> Option<UserInfoView> {
        let info = self.user_infos.get_mut(username)?;
        info.interests_liked = liked;
        info.interests_hated = hated;
        Some(info.clone())
    }

    fn ensure_info(&mut self, username: &str) -> UserInfoView {
        self.user_infos
            .entry(username.to_owned())
            .or_insert_with(|| UserInfoView {
                username: username.to_owned(),
                ..Default::default()
            })
            .clone()
    }

    fn info(&self, username: &str) -> Option<&UserInfoView> {
        self.user_infos.get(username)
    }

    fn info_received(&mut self, received: UserInfoReceived) -> UserInfoView {
        let info = self
            .user_infos
            .entry(received.username.clone())
            .or_insert_with(|| UserInfoView {
                username: received.username,
                ..Default::default()
            });
        info.received = true;
        info.description = received.description;
        info.picture_base64 = received
            .picture
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        info.upload_slots = received.total_uploads;
        info.queue_size = received.queue_size;
        info.slots_available = received.slots_available;
        info.clone()
    }

    fn ban(&mut self, username: String) -> Vec<String> {
        if !self.banned.contains(&username) {
            self.banned.push(username);
            self.banned.sort();
        }
        self.banned.clone()
    }

    fn unban(&mut self, username: &str) -> Vec<String> {
        self.banned.retain(|user| user != username);
        self.banned.clone()
    }

    fn ignore(&mut self, username: String) -> Vec<String> {
        if !self.ignored.contains(&username) {
            self.ignored.push(username);
            self.ignored.sort();
        }
        self.ignored.clone()
    }

    fn unignore(&mut self, username: &str) -> Vec<String> {
        self.ignored.retain(|user| user != username);
        self.ignored.clone()
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
    let mut data = app.projection.write();
    if let Some(buddy) = data
        .users
        .set_status(&username, status_string(status), privileged)
    {
        data.broadcast(AppEvent::Buddy { buddy });
    }
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
    let mut data = app.projection.write();
    let received_at = now();
    data.users.insert_browse(BrowseView {
        username: username.clone(),
        folders: shares,
        private_folders: private_shares,
        received_at,
    });
    data.broadcast(AppEvent::BrowseLoaded {
        username,
        received_at,
    });
}

pub fn user_info_received(app: &App, received: UserInfoReceived) {
    let mut data = app.projection.write();
    let info = data.users.info_received(received);
    data.broadcast(AppEvent::UserInfo { info });
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
    let status = user_exists.then(|| status_string(status));
    let mut data = app.projection.write();
    if let Some(buddy) = data.users.apply_watch(&user, status, stats) {
        data.broadcast(AppEvent::Buddy { buddy });
    }
}

pub async fn user_stats(app: &Arc<App>, user: String, stats: UserStats) {
    record_user_shares(&app.db, &user, stats.files, stats.dirs, now())
        .await
        .unwrap_or_else(|error| fatal(error));
    behavior::stats_received(app, &user, stats.files, stats.dirs).await;
    let mut data = app.projection.write();
    if let Some(buddy) = data.users.set_buddy_stats(&user, &stats) {
        data.broadcast(AppEvent::Buddy { buddy });
    }
    if let Some(info) = data.users.set_info_stats(&user, stats) {
        data.broadcast(AppEvent::UserInfo { info });
    }
}

pub fn user_interests(app: &App, user: String, likes: Vec<String>, hates: Vec<String>) {
    let mut data = app.projection.write();
    if let Some(info) = data.users.set_info_interests(&user, likes, hates) {
        data.broadcast(AppEvent::UserInfo { info });
    }
}
