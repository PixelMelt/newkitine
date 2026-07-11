use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::MySqlPool;
use sqlx::Row;

use crate::types::{RuntimeConfig, Settings, SharedFolder};

use super::api;
use super::behavior;
use super::config;
use super::contract::{AppEvent, SettingsPayload};
use super::session;
use super::state::App;

pub struct SettingsState {
    locked: Vec<&'static str>,
    gluetun_enabled: bool,
    inner: RwLock<Inner>,
}

struct Inner {
    stored: Settings,
    live_port: Option<u16>,
}

pub struct SettingsChange {
    pub stored: Settings,
    pub effective: Settings,
    pub runtime: RuntimeConfig,
    pub behavior_changed: bool,
}

impl SettingsState {
    pub fn new(stored: Settings, locked: Vec<&'static str>, live_port: Option<u16>) -> Self {
        Self {
            locked,
            gluetun_enabled: live_port.is_some(),
            inner: RwLock::new(Inner { stored, live_port }),
        }
    }

    pub fn effective(&self) -> Settings {
        let inner = self.inner.read().unwrap();
        let mut settings = inner.stored.clone();
        config::apply_settings_env(&mut settings);
        if let Some(port) = inner.live_port {
            settings.listen_port = port;
        }
        settings
    }

    pub fn behavior_policy(&self) -> (String, String) {
        let inner = self.inner.read().unwrap();
        (
            inner.stored.filter_level.clone(),
            inner.stored.denied_message.clone(),
        )
    }

    pub fn set_live_port(&self, port: u16) {
        self.inner.write().unwrap().live_port = Some(port);
    }

    pub fn payload(&self) -> SettingsPayload {
        SettingsPayload {
            settings: PublicSettings::from(&self.effective()),
            locked: self.locked.clone(),
            gluetun: self.gluetun_enabled,
        }
    }

    pub fn stage_update(&self, update: SettingsUpdate) -> Result<SettingsChange, String> {
        let old = self.effective();
        let effective = update.into_settings(&old);
        if !matches!(
            effective.filter_level.as_str(),
            "open" | "guarded" | "strict"
        ) {
            return Err(format!("unknown filter level {}", effective.filter_level));
        }
        if let Some(key) = self
            .locked
            .iter()
            .find(|key| old.locked_differs(&effective, key))
        {
            return Err(format!("{key} is set by the environment"));
        }
        if self.gluetun_enabled && effective.listen_port != old.listen_port {
            return Err("listen_port is managed by gluetun".into());
        }
        let runtime = effective.runtime_config()?;
        let stored = {
            let inner = self.inner.read().unwrap();
            let mut stored = effective.clone();
            for key in &self.locked {
                stored.copy_locked_from(&inner.stored, key);
            }
            if self.gluetun_enabled {
                stored.listen_port = inner.stored.listen_port;
            }
            stored
        };
        let behavior_changed = effective.filter_level != old.filter_level
            || effective.denied_message != old.denied_message;
        Ok(SettingsChange {
            stored,
            effective,
            runtime,
            behavior_changed,
        })
    }

    pub fn commit(&self, stored: Settings) {
        self.inner.write().unwrap().stored = stored;
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsUpdate {
    pub server: String,
    pub username: String,
    pub password: Option<String>,
    pub listen_port: u16,
    pub description: String,
    pub download_dir: PathBuf,
    pub incomplete_dir: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
    pub filter_level: String,
    pub denied_message: String,
}

impl SettingsUpdate {
    pub fn into_settings(self, current: &Settings) -> Settings {
        Settings {
            server: self.server,
            username: self.username,
            password: self.password.unwrap_or_else(|| current.password.clone()),
            listen_port: self.listen_port,
            description: self.description,
            download_dir: self.download_dir,
            incomplete_dir: self.incomplete_dir,
            shares: self.shares,
            upload_slots: self.upload_slots,
            queue_file_limit: self.queue_file_limit,
            upload_limit_kbps: self.upload_limit_kbps,
            download_limit_kbps: self.download_limit_kbps,
            auto_reconnect: self.auto_reconnect,
            theme: self.theme,
            filter_level: self.filter_level,
            denied_message: self.denied_message,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSettings {
    pub server: String,
    pub username: String,
    pub password_set: bool,
    pub listen_port: u16,
    pub description: String,
    pub download_dir: PathBuf,
    pub incomplete_dir: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
    pub filter_level: String,
    pub denied_message: String,
}

impl From<&Settings> for PublicSettings {
    fn from(settings: &Settings) -> Self {
        Self {
            server: settings.server.clone(),
            username: settings.username.clone(),
            password_set: !settings.password.is_empty(),
            listen_port: settings.listen_port,
            description: settings.description.clone(),
            download_dir: settings.download_dir.clone(),
            incomplete_dir: settings.incomplete_dir.clone(),
            shares: settings.shares.clone(),
            upload_slots: settings.upload_slots,
            queue_file_limit: settings.queue_file_limit,
            upload_limit_kbps: settings.upload_limit_kbps,
            download_limit_kbps: settings.download_limit_kbps,
            auto_reconnect: settings.auto_reconnect,
            theme: settings.theme.clone(),
            filter_level: settings.filter_level.clone(),
            denied_message: settings.denied_message.clone(),
        }
    }
}

pub async fn load_settings(pool: &MySqlPool) -> Option<String> {
    sqlx::query("SELECT data FROM settings WHERE id = 1")
        .fetch_optional(pool)
        .await
        .expect("load settings")
        .map(|row| row.get(0))
}

pub async fn save_settings(pool: &MySqlPool, data: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (id, data) VALUES (1, ?)
         ON DUPLICATE KEY UPDATE data = VALUES(data)",
    )
    .bind(data)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn apply_runtime(app: &App) -> Result<(), String> {
    let runtime = app.settings.effective().runtime_config()?;
    app.client
        .apply_config(app.next_settings_revision(), runtime)
        .await;
    Ok(())
}

pub async fn get_settings(State(app): State<Arc<App>>) -> Json<SettingsPayload> {
    Json(app.settings.payload())
}

pub async fn put_settings(
    State(app): State<Arc<App>>,
    Json(update): Json<SettingsUpdate>,
) -> impl IntoResponse {
    let change = match app.settings.stage_update(update) {
        Ok(change) => change,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    if let Err(error) = save_settings(&app.db, &serde_json::to_string(&change.stored).unwrap()).await
    {
        return api::db_failed(error).into_response();
    }
    app.settings.commit(change.stored);

    app.client
        .apply_config(app.next_settings_revision(), change.runtime)
        .await;
    if change.behavior_changed {
        behavior::apply_level(&app).await;
    }

    session::apply_settings(
        &app,
        change.effective.server.clone(),
        change.effective.listen_port,
        &change.effective.username,
    );
    {
        let mut data = app.data.write().unwrap();
        app.broadcast(&mut data, AppEvent::Settings(app.settings.payload()));
    }
    StatusCode::ACCEPTED.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_round_trip() {
        let settings = Settings {
            username: "someone".into(),
            shares: vec![SharedFolder {
                virtual_name: "Music".into(),
                path: PathBuf::from("/data/shared"),
                buddy_only: true,
            }],
            theme: "catppuccin".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, loaded);
    }

    #[test]
    fn omitted_password_retains_current_secret() {
        let update: SettingsUpdate = serde_json::from_value(serde_json::json!({
            "server": "server.slsknet.org:2242",
            "username": "someone",
            "listen_port": 2234,
            "description": "",
            "download_dir": "/downloads",
            "incomplete_dir": null,
            "shares": [],
            "upload_slots": 2,
            "queue_file_limit": 100,
            "upload_limit_kbps": 0,
            "download_limit_kbps": 0,
            "auto_reconnect": true,
            "theme": "dark",
            "filter_level": "open",
            "denied_message": "denied"
        }))
        .unwrap();
        let current = Settings {
            password: "secret".into(),
            ..Default::default()
        };
        assert_eq!(update.into_settings(&current).password, "secret");
    }

    #[test]
    fn partial_update_is_rejected() {
        let result: Result<SettingsUpdate, _> =
            serde_json::from_value(serde_json::json!({ "server": "server.slsknet.org:2242" }));
        assert!(result.is_err());
    }
}
