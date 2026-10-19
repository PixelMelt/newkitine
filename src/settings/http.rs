use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api;
use crate::behavior;
use crate::contract::{AppEvent, SettingsPayload};
use crate::session;
use crate::state::App;

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
    if let Some(shares) = change.shares {
        app.client.set_shares(Arc::new(shares)).await;
    }
    if let Some(downloads) = change.downloads {
        app.client.set_downloads(Arc::new(downloads)).await;
    }
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
    data.broadcast(AppEvent::Settings(Box::new(app.settings.payload())));
    Ok(())
}

#[derive(Deserialize)]
struct SettingsUpdate {
    password: Option<String>,
    #[serde(flatten)]
    settings: Settings,
}

impl SettingsUpdate {
    fn into_settings(self, current: &Settings) -> Settings {
        Settings {
            password: self.password.unwrap_or_else(|| current.password.clone()),
            ..self.settings
        }
    }
}

pub(crate) fn router() -> Router<Arc<App>> {
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
        let body = serde_json::to_value(Settings::default()).unwrap();
        assert!(body.get("password").is_none());
        let update: SettingsUpdate = serde_json::from_value(body).unwrap();
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
