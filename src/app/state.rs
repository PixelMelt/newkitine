use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::MySqlPool;

use crate::client::Client;

use super::behavior::Behavior;
use super::geo::Geo;
use super::projection::Projection;
use super::settings::SettingsState;
use super::stats::StatsSink;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub struct App {
    pub client: Client,
    pub db: MySqlPool,
    pub projection: Projection,
    pub settings: SettingsState,
    pub geo: Option<Geo>,
    pub stats: StatsSink,
    pub behavior: Behavior,
    pub list_mutation: tokio::sync::Mutex<()>,
}
