use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::MySqlPool;
use tokio::sync::{broadcast, mpsc};

use crate::client::Client;

use super::behavior::Behavior;
use super::chat::Chat;
use super::contract::{AppEvent, Envelope};
use super::geo::Geo;
use super::interests::Interests;
use super::search::SearchState;
use super::session::Session;
use super::settings::SettingsState;
use super::stats::StatsSink;
use super::transfers::{TransferWork, Transfers};
use super::users::UsersState;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub struct App {
    pub client: Client,
    pub db: MySqlPool,
    pub data: RwLock<AppData>,
    pub events: broadcast::Sender<String>,
    pub settings: SettingsState,
    pub geo: Option<Geo>,
    pub stats: StatsSink,
    pub behavior: Behavior,
    pub settings_revision: AtomicU64,
    pub transfer_work: mpsc::Sender<TransferWork>,
}

impl App {
    pub fn broadcast(&self, data: &mut AppData, event: AppEvent) {
        data.revision += 1;
        let message = serde_json::to_string(&Envelope {
            rev: data.revision,
            event: &event,
        })
        .expect("app event serialization");
        if self.events.send(message).is_err() {
            tracing::trace!("no websocket subscribers, event kept in projection only");
        }
    }

    pub fn next_settings_revision(&self) -> u64 {
        self.settings_revision.fetch_add(1, Ordering::Relaxed) + 1
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

    pub fn revision(&self) -> u64 {
        self.revision
    }
}
