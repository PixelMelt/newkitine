use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::app::api;
use crate::app::behavior;
use crate::app::contract::{AppEvent, SettingsPayload};
use crate::app::session;
use crate::app::state::App;
use crate::types::{FilterLevel, SharedFolder};

use super::{Settings, save_settings};

enum ApplyError {
    Invalid(String),
    Db(sqlx::Error),
}

async fn apply_update(app: &Arc<App>, update: SettingsUpdate) -> Result<(), ApplyError> {
    let _mutation = app.settings.mutation.lock().await;
    let change = app
        .settings
        .stage_update(update.into_settings(&app.settings.effective()))
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

#[derive(Debug, Clone, Deserialize)]
struct SettingsUpdate {
    server: String,
    username: String,
    password: Option<String>,
    listen_port: u16,
    description: String,
    download_dir: PathBuf,
    incomplete_dir: Option<PathBuf>,
    shares: Vec<SharedFolder>,
    upload_slots: usize,
    queue_file_limit: usize,
    uploads_per_user: usize,
    upload_limit_kbps: u32,
    download_limit_kbps: u32,
    auto_reconnect: bool,
    theme: String,
    filter_level: FilterLevel,
    denied_message: String,
    max_search_results: usize,
    min_search_chars: usize,
    respond_to_searches: bool,
    max_search_responses: usize,
    rescan_daily: bool,
    rescan_hour_utc: u8,
    autoclear_downloads: bool,
    autoclear_uploads: bool,
    queue_size_limit_mb: u64,
    banned_message: String,
    download_username_subfolders: bool,
    share_filters: Vec<String>,
}

impl SettingsUpdate {
    fn into_settings(self, current: &Settings) -> Settings {
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
            max_search_results: self.max_search_results,
            min_search_chars: self.min_search_chars,
            respond_to_searches: self.respond_to_searches,
            max_search_responses: self.max_search_responses,
            rescan_daily: self.rescan_daily,
            rescan_hour_utc: self.rescan_hour_utc,
            autoclear_downloads: self.autoclear_downloads,
            autoclear_uploads: self.autoclear_uploads,
            queue_size_limit_mb: self.queue_size_limit_mb,
            banned_message: self.banned_message,
            download_username_subfolders: self.download_username_subfolders,
            share_filters: self.share_filters,
        }
    }
}

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new().route("/api/settings", get(get_settings).put(put_settings))
}

async fn get_settings(State(app): State<Arc<App>>) -> Json<SettingsPayload> {
    Json(app.settings.payload())
}

async fn put_settings(
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
            "denied_message": "denied",
            "max_search_results": 300,
            "min_search_chars": 3,
            "respond_to_searches": true,
            "max_search_responses": 500,
            "rescan_daily": false,
            "rescan_hour_utc": 0,
            "autoclear_downloads": false,
            "autoclear_uploads": false,
            "queue_size_limit_mb": 0,
            "banned_message": "Banned",
            "download_username_subfolders": false,
            "share_filters": []
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
