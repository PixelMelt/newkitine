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
use super::projection::{ProjectionWriter, set_member};
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
    #[serde(skip)]
    pub sequence: u64,
}

#[derive(Clone)]
pub struct BrowseView {
    pub username: String,
    pub folders: Arc<Vec<FolderContents>>,
    pub private_folders: Arc<Vec<FolderContents>>,
    pub received_at: i64,
}

impl BrowseView {
    pub fn all(&self) -> impl Iterator<Item = &FolderContents> + Clone {
        self.folders.iter().chain(self.private_folders.iter())
    }
}

pub fn browse(app: &App, username: &str) -> Option<BrowseView> {
    app.projection.read().users.browses.get(username).cloned()
}

const MAX_RETAINED_BROWSES: usize = 8;
const MAX_RETAINED_USER_INFOS: usize = 32;

#[derive(Default)]
pub struct UsersState {
    buddies: HashMap<String, BuddyView>,
    banned: Vec<String>,
    ignored: Vec<String>,
    user_infos: HashMap<String, UserInfoView>,
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
            ..Default::default()
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

    pub fn browse_timestamps(&self) -> impl Iterator<Item = (&String, i64)> {
        self.browses
            .iter()
            .map(|(username, browse)| (username, browse.received_at))
    }

    fn insert_browse(&mut self, browse: BrowseView) -> Vec<String> {
        let newest = browse.username.clone();
        self.browses.insert(newest.clone(), browse);
        let mut evicted = Vec::new();
        while self.browses.len() > MAX_RETAINED_BROWSES {
            let oldest = self
                .browses
                .values()
                .filter(|browse| browse.username != newest)
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

    fn update_buddy(
        &mut self,
        username: &str,
        apply: impl FnOnce(&mut BuddyView),
    ) -> Option<BuddyView> {
        let buddy = self.buddies.get_mut(username)?;
        apply(buddy);
        Some(buddy.clone())
    }

    fn update_info(
        &mut self,
        username: &str,
        apply: impl FnOnce(&mut UserInfoView),
    ) -> Option<UserInfoView> {
        let info = self.user_infos.get_mut(username)?;
        apply(info);
        Some(info.clone())
    }

    pub fn infos(&self) -> impl Iterator<Item = &UserInfoView> {
        self.user_infos.values()
    }

    fn info(&self, username: &str) -> Option<&UserInfoView> {
        self.user_infos.get(username)
    }

    fn touch_info(
        &mut self,
        username: &str,
        apply: impl FnOnce(&mut UserInfoView),
    ) -> (UserInfoView, Vec<String>) {
        self.info_counter += 1;
        let info = self
            .user_infos
            .entry(username.to_owned())
            .or_insert_with(|| UserInfoView {
                username: username.to_owned(),
                ..Default::default()
            });
        info.sequence = self.info_counter;
        apply(info);
        let info = info.clone();
        let mut evicted = Vec::new();
        while self.user_infos.len() > MAX_RETAINED_USER_INFOS {
            let oldest = self
                .user_infos
                .values()
                .min_by_key(|info| info.sequence)
                .unwrap()
                .username
                .clone();
            self.user_infos.remove(&oldest);
            evicted.push(oldest);
        }
        (info, evicted)
    }

    fn set_blocked(&mut self, block: Block, username: &str, blocked: bool) -> Vec<String> {
        let list = match block {
            Block::Banned => &mut self.banned,
            Block::Ignored => &mut self.ignored,
        };
        set_member(list, username, blocked);
        list.clone()
    }
}

#[derive(Clone, Copy)]
enum Block {
    Banned,
    Ignored,
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

fn publish_buddy(app: &App, username: &str, apply: impl FnOnce(&mut BuddyView)) {
    let mut data = app.projection.write();
    if let Some(buddy) = data.users.update_buddy(username, apply) {
        data.broadcast(AppEvent::Buddy { buddy });
    }
}

fn publish_info(app: &App, username: &str, apply: impl FnOnce(&mut UserInfoView)) {
    let mut data = app.projection.write();
    if let Some(info) = data.users.update_info(username, apply) {
        data.broadcast(AppEvent::UserInfo { info });
    }
}

fn open_info(app: &App, username: &str, apply: impl FnOnce(&mut UserInfoView)) {
    let mut data = app.projection.write();
    let (info, evicted) = data.users.touch_info(username, apply);
    for username in evicted {
        data.broadcast(AppEvent::UserInfoRemoved { username });
    }
    data.broadcast(AppEvent::UserInfo { info });
}

pub fn user_status(app: &App, username: String, status: Option<UserStatus>, privileged: bool) {
    publish_buddy(app, &username, |buddy| {
        buddy.status = status_string(status).into();
        buddy.privileged = privileged;
    });
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
    open_info(app, &received.username, |info| {
        info.received = true;
        info.description = received.description;
        info.picture_base64 = received
            .picture
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        info.upload_slots = received.total_uploads;
        info.queue_size = received.queue_size;
        info.slots_available = received.slots_available;
    });
}

async fn record_stats(app: &Arc<App>, user: &str, stats: &UserStats) {
    record_user_shares(&app.db, user, stats.files, stats.dirs, now())
        .await
        .unwrap_or_else(|error| fatal(error));
    behavior::stats_received(app, user, stats.files, stats.dirs).await;
}

pub async fn watch_user(
    app: &Arc<App>,
    user: String,
    user_exists: bool,
    status: Option<UserStatus>,
    stats: Option<UserStats>,
) {
    if let Some(stats) = &stats {
        record_stats(app, &user, stats).await;
    }
    publish_buddy(app, &user, |buddy| {
        if user_exists {
            buddy.status = status_string(status).into();
        }
        if let Some(stats) = stats {
            buddy.stats = stats;
        }
    });
}

pub async fn user_stats(app: &Arc<App>, user: String, stats: UserStats) {
    record_stats(app, &user, &stats).await;
    let mut data = app.projection.write();
    if let Some(buddy) = data
        .users
        .update_buddy(&user, |buddy| buddy.stats = stats.clone())
    {
        data.broadcast(AppEvent::Buddy { buddy });
    }
    if let Some(info) = data
        .users
        .update_info(&user, |info| info.stats = Some(stats))
    {
        data.broadcast(AppEvent::UserInfo { info });
    }
}

pub fn user_interests(app: &App, user: String, likes: Vec<String>, hates: Vec<String>) {
    publish_info(app, &user, |info| {
        info.interests_liked = likes;
        info.interests_hated = hates;
    });
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
        assert!(!state.browses.contains_key("user0"));
        assert!(state.browses.contains_key("newcomer"));
    }

    #[test]
    fn replies_to_a_requested_profile_land_on_its_entry() {
        let mut state = UsersState::default();
        state.touch_info("peer", |_| {});
        let stats = state
            .update_info("peer", |info| info.stats = Some(UserStats::default()))
            .expect("stats reply after request");
        assert_eq!(stats.stats, Some(UserStats::default()));
        let interests = state
            .update_info("peer", |info| info.interests_liked = vec!["jazz".into()])
            .expect("interests reply after request");
        assert_eq!(interests.interests_liked, vec!["jazz".to_string()]);
        assert!(!interests.received);
        assert!(state.update_info("stranger", |_| {}).is_none());
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
