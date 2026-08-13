use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::warn;

use super::state::App;

const API_URL: &str = "https://api.pushover.net/1/messages.json";
const MAX_TITLE_CHARS: usize = 250;
const MAX_MESSAGE_CHARS: usize = 1024;

pub struct Keys {
    pub token: String,
    pub user_key: String,
}

struct Notification {
    keys: Keys,
    title: String,
    message: String,
}

pub struct Notifier {
    tx: mpsc::Sender<Notification>,
    http: reqwest::Client,
}

pub struct Worker {
    http: reqwest::Client,
    rx: mpsc::Receiver<Notification>,
}

impl Notifier {
    pub fn new() -> (Self, Worker) {
        let (tx, rx) = mpsc::channel(64);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("cannot build pushover http client");
        let worker = Worker {
            http: http.clone(),
            rx,
        };
        (Self { tx, http }, worker)
    }

    pub fn notify(&self, keys: Keys, title: &str, message: &str) {
        match self.tx.try_reserve() {
            Ok(permit) => permit.send(Notification {
                keys,
                title: truncate(title, MAX_TITLE_CHARS),
                message: truncate(message, MAX_MESSAGE_CHARS),
            }),
            Err(error) => warn!(%error, "dropping pushover notification"),
        }
    }

    async fn test(&self, keys: &Keys) -> Result<(), String> {
        send(&self.http, keys, "Newkitine", "Test notification").await
    }
}

impl Worker {
    pub async fn run(mut self) {
        while let Some(notification) = self.rx.recv().await {
            if let Err(error) = send(
                &self.http,
                &notification.keys,
                &notification.title,
                &notification.message,
            )
            .await
            {
                warn!(%error, "cannot deliver pushover notification");
            }
        }
    }
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

async fn send(
    http: &reqwest::Client,
    keys: &Keys,
    title: &str,
    message: &str,
) -> Result<(), String> {
    let response = http
        .post(API_URL)
        .form(&[
            ("token", keys.token.as_str()),
            ("user", keys.user_key.as_str()),
            ("title", title),
            ("message", message),
        ])
        .send()
        .await
        .map_err(|error| format!("pushover request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => format!("(unreadable body: {error})"),
        };
        return Err(format!("pushover returned {status}: {body}"));
    }
    Ok(())
}

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new().route("/api/pushover/test", post(send_test))
}

async fn send_test(State(app): State<Arc<App>>) -> impl IntoResponse {
    let Some(keys) = app.settings.pushover_keys() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "pushover_token and pushover_user_key must be set" })),
        )
            .into_response();
    };
    match app.pushover.test(&keys).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": error }))).into_response(),
    }
}
