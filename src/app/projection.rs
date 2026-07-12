use std::ops::{Deref, DerefMut};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use tokio::sync::broadcast;

use crate::types::TransferDirection;

use super::chat::Chat;
use super::contract::{AppEvent, Envelope, Snapshot};
use super::interests::Interests;
use super::search::SearchState;
use super::session::Session;
use super::settings::SettingsState;
use super::transfers::Transfers;
use super::users::UsersState;

const EVENT_CHANNEL_CAPACITY: usize = 256;

pub struct Projection {
    data: RwLock<AppData>,
    events: broadcast::Sender<String>,
}

impl Projection {
    pub fn new(data: AppData) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            data: RwLock::new(data),
            events,
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, AppData> {
        self.data.read().unwrap()
    }

    pub fn write(&self) -> ProjectionWriter<'_> {
        ProjectionWriter {
            data: self.data.write().unwrap(),
            events: &self.events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    pub fn snapshot_json(&self, settings: &SettingsState) -> String {
        let data = self.read();
        serde_json::to_string(&Snapshot {
            kind: "snapshot",
            rev: data.revision,
            status: data.session.status(),
            downloads: data.transfers.list(TransferDirection::Download),
            uploads: data.transfers.list(TransferDirection::Upload),
            searches: data.search.searches(),
            buddies: data.users.buddies().collect(),
            banned: data.users.banned(),
            ignored: data.users.ignored(),
            wishlist: data.search.wishlist(),
            interests: data.interests.view(),
            rooms: data.chat.rooms(),
            chat_partners: data.chat.partners(),
            browses: data.users.browse_timestamps().collect(),
            settings: settings.payload(),
        })
        .expect("snapshot serialization")
    }
}

pub struct ProjectionWriter<'a> {
    data: RwLockWriteGuard<'a, AppData>,
    events: &'a broadcast::Sender<String>,
}

impl Deref for ProjectionWriter<'_> {
    type Target = AppData;

    fn deref(&self) -> &AppData {
        &self.data
    }
}

impl DerefMut for ProjectionWriter<'_> {
    fn deref_mut(&mut self) -> &mut AppData {
        &mut self.data
    }
}

impl ProjectionWriter<'_> {
    pub fn broadcast(&mut self, event: AppEvent) {
        self.data.revision += 1;
        let message = serde_json::to_string(&Envelope {
            rev: self.data.revision,
            event: &event,
        })
        .expect("app event serialization");
        if self.events.send(message).is_err() {
            tracing::trace!("no websocket subscribers, event kept in projection only");
        }
    }
}

pub struct AppData {
    revision: u64,
    pub session: Session,
    pub transfers: Transfers,
    pub search: SearchState,
    pub users: UsersState,
    pub chat: Chat,
    pub interests: Interests,
}

impl AppData {
    pub fn new(
        session: Session,
        transfers: Transfers,
        search: SearchState,
        users: UsersState,
        chat: Chat,
        interests: Interests,
    ) -> Self {
        Self {
            revision: 0,
            session,
            transfers,
            search,
            users,
            chat,
            interests,
        }
    }
}
