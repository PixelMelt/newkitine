mod db;
mod http;

pub use db::load_notes;
pub(super) use http::router;

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;

use crate::client::UserInfoReceived;
use crate::types::{FolderContents, UserStats, UserStatus};

use super::behavior;
use super::contract::AppEvent;
use super::db::fatal;
use super::peer_history::record_user_shares;
use super::projection::ProjectionWriter;
use super::state::{App, now};

#[derive(Default, Clone, serde::Serialize)]
pub struct BuddyView {
    pub username: String,
    pub status: String,
    pub privileged: bool,
    pub stats: UserStats,
    pub note: String,
}

#[derive(Default, Clone, serde::Serialize)]
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

pub struct BrowseView {
    pub username: String,
    pub folders: Arc<Vec<FolderContents>>,
    pub private_folders: Arc<Vec<FolderContents>>,
    pub received_at: i64,
}

const MAX_RETAINED_BROWSES: usize = 8;
const MAX_RETAINED_USER_INFOS: usize = 32;

#[derive(Default)]
pub struct UsersState {
    buddies: HashMap<String, BuddyView>,
    banned: Vec<String>,
    ignored: Vec<String>,
    user_infos: HashMap<String, UserInfoView>,
    info_sequence: HashMap<String, u64>,
    info_counter: u64,
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
            info_sequence: HashMap::new(),
            info_counter: 0,
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

    fn insert_browse(&mut self, browse: BrowseView) -> Vec<String> {
        self.browses.insert(browse.username.clone(), browse);
        let mut evicted = Vec::new();
        while self.browses.len() > MAX_RETAINED_BROWSES {
            let oldest = self
                .browses
                .values()
                .min_by_key(|browse| browse.received_at)
                .unwrap()
                .username
                .clone();
            self.browses.remove(&oldest);
            evicted.push(oldest);
        }
        evicted
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

    fn reset_statuses(&mut self) -> Vec<BuddyView> {
        self.buddies
            .values_mut()
            .map(|buddy| {
                buddy.status = "unknown".into();
                buddy.privileged = false;
                buddy.clone()
            })
            .collect()
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

    fn evict_excess_infos(&mut self, keep: &str) -> Vec<String> {
        self.info_counter += 1;
        self.info_sequence
            .insert(keep.to_owned(), self.info_counter);
        let mut evicted = Vec::new();
        while self.user_infos.len() > MAX_RETAINED_USER_INFOS {
            let oldest = self
                .info_sequence
                .iter()
                .min_by_key(|(_, sequence)| **sequence)
                .map(|(username, _)| username.clone())
                .unwrap();
            self.user_infos.remove(&oldest);
            self.info_sequence.remove(&oldest);
            evicted.push(oldest);
        }
        evicted
    }

    pub fn infos(&self) -> impl Iterator<Item = &UserInfoView> {
        self.user_infos.values()
    }

    fn ensure_info(&mut self, username: &str) -> (UserInfoView, Vec<String>) {
        let info = self
            .user_infos
            .entry(username.to_owned())
            .or_insert_with(|| UserInfoView {
                username: username.to_owned(),
                ..Default::default()
            })
            .clone();
        let evicted = self.evict_excess_infos(username);
        (info, evicted)
    }

    fn info(&self, username: &str) -> Option<&UserInfoView> {
        self.user_infos.get(username)
    }

    fn info_received(&mut self, received: UserInfoReceived) -> (UserInfoView, Vec<String>) {
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
        let info = info.clone();
        let evicted = self.evict_excess_infos(&info.username);
        (info, evicted)
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

pub fn server_disconnected(data: &mut ProjectionWriter<'_>) {
    for buddy in data.users.reset_statuses() {
        data.broadcast(AppEvent::Buddy { buddy });
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
    let evicted = data.users.insert_browse(BrowseView {
        username: username.clone(),
        folders: Arc::new(shares),
        private_folders: Arc::new(private_shares),
        received_at,
    });
    for username in evicted {
        data.broadcast(AppEvent::BrowseRemoved { username });
    }
    data.broadcast(AppEvent::BrowseLoaded {
        username,
        received_at,
    });
}

pub fn user_info_received(app: &App, received: UserInfoReceived) {
    let mut data = app.projection.write();
    let (info, evicted) = data.users.info_received(received);
    for username in evicted {
        data.broadcast(AppEvent::UserInfoRemoved { username });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn browse(username: &str, received_at: i64) -> BrowseView {
        BrowseView {
            username: username.into(),
            folders: Arc::new(Vec::new()),
            private_folders: Arc::new(Vec::new()),
            received_at,
        }
    }

    #[test]
    fn oldest_browse_evicts_beyond_retention_cap() {
        let mut state = UsersState::default();
        for i in 0..MAX_RETAINED_BROWSES as i64 {
            assert!(
                state
                    .insert_browse(browse(&format!("user{i}"), i))
                    .is_empty()
            );
        }
        let evicted = state.insert_browse(browse("newcomer", 100));
        assert_eq!(evicted, vec!["user0".to_string()]);
        assert_eq!(state.browses.len(), MAX_RETAINED_BROWSES);
        assert!(state.browse("user0").is_none());
        assert!(state.browse("newcomer").is_some());
    }

    #[test]
    fn rebrowsing_same_user_does_not_evict() {
        let mut state = UsersState::default();
        for i in 0..MAX_RETAINED_BROWSES as i64 {
            state.insert_browse(browse(&format!("user{i}"), i));
        }
        let evicted = state.insert_browse(browse("user3", 100));
        assert!(evicted.is_empty());
        assert_eq!(state.browses.len(), MAX_RETAINED_BROWSES);
    }
}
