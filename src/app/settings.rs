use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;
use sqlx::Row;

use crate::types::{PublicSettings, RuntimeConfig, Settings, SharedFolder};

use super::api;
use super::behavior;
use super::config;
use super::contract::{AppEvent, SettingsPayload};
use super::session;
use super::state::App;

pub struct SettingsState {
    locked: Vec<&'static str>,
    gluetun_enabled: bool,
    mutation: tokio::sync::Mutex<()>,
    inner: RwLock<Inner>,
}

struct Inner {
    stored: Settings,
    live_port: Option<u16>,
}

struct SettingsChange {
    stored: Settings,
    effective: Settings,
    runtime: RuntimeConfig,
    behavior_changed: bool,
}

impl SettingsState {
    pub fn new(stored: Settings, locked: Vec<&'static str>, live_port: Option<u16>) -> Self {
        Self {
            locked,
            gluetun_enabled: live_port.is_some(),
            mutation: tokio::sync::Mutex::new(()),
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

    pub fn payload(&self) -> SettingsPayload {
        SettingsPayload {
            settings: PublicSettings::from(&self.effective()),
            locked: self.locked.clone(),
            gluetun: self.gluetun_enabled,
        }
    }

    fn stage_update(&self, update: SettingsUpdate) -> Result<SettingsChange, String> {
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

    fn commit(&self, stored: Settings) {
        self.inner.write().unwrap().stored = stored;
    }

    fn runtime_with_live_port(&self, port: u16) -> Result<RuntimeConfig, String> {
        let mut settings = self.effective();
        settings.listen_port = port;
        settings.runtime_config()
    }

    fn set_live_port(&self, port: u16) {
        self.inner.write().unwrap().live_port = Some(port);
    }
}

pub enum ApplyError {
    Invalid(String),
    Db(sqlx::Error),
}

pub async fn apply_update(app: &Arc<App>, update: SettingsUpdate) -> Result<(), ApplyError> {
    let _mutation = app.settings.mutation.lock().await;
    let change = app
        .settings
        .stage_update(update)
        .map_err(ApplyError::Invalid)?;
    save_settings(&app.db, &serde_json::to_string(&change.stored).unwrap())
        .await
        .map_err(ApplyError::Db)?;
    app.client.apply_config(change.runtime).await;
    app.settings.commit(change.stored);
    if change.behavior_changed {
        behavior::apply_level(app).await;
    }
    session::apply_settings(
        app,
        change.effective.server.clone(),
        change.effective.listen_port,
        &change.effective.username,
    );
    let mut data = app.projection.write();
    data.broadcast(AppEvent::Settings(app.settings.payload()));
    Ok(())
}

pub async fn apply_live_port(app: &Arc<App>, port: u16) -> Result<(), String> {
    let _mutation = app.settings.mutation.lock().await;
    let runtime = app.settings.runtime_with_live_port(port)?;
    app.client.apply_config(runtime).await;
    app.settings.set_live_port(port);
    session::set_listen_port(app, port);
    Ok(())
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
    pub uploads_per_user: usize,
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
            uploads_per_user: self.uploads_per_user,
            upload_limit_kbps: self.upload_limit_kbps,
            download_limit_kbps: self.download_limit_kbps,
            auto_reconnect: self.auto_reconnect,
            theme: self.theme,
            filter_level: self.filter_level,
            denied_message: self.denied_message,
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

pub async fn get_settings(State(app): State<Arc<App>>) -> Json<SettingsPayload> {
    Json(app.settings.payload())
}

pub async fn put_settings(
    State(app): State<Arc<App>>,
    Json(update): Json<SettingsUpdate>,
) -> impl IntoResponse {
    match apply_update(&app, update).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(ApplyError::Invalid(error)) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
        }
        Err(ApplyError::Db(error)) => api::db_failed(error).into_response(),
    }
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
            "uploads_per_user": 1,
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
