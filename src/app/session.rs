use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::chat;
use super::contract::AppEvent;
use super::state::App;
use super::users;

#[derive(Default, Clone, serde::Serialize)]
pub struct Status {
    pub connected: bool,
    pub logged_in: bool,
    pub username: String,
    pub server: String,
    pub banner: String,
    pub listen_port: u16,
    pub shared_folders: u32,
    pub shared_files: u32,
    pub scanning: bool,
    pub scan_progress: u64,
    pub share_scan_error: Option<String>,
    pub privileges_secs: u32,
    pub peer_connections: usize,
}

pub struct Session {
    status: Status,
}

impl Session {
    pub fn new(server: String, username: String, listen_port: u16) -> Self {
        Self {
            status: Status {
                server,
                username,
                listen_port,
                ..Default::default()
            },
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }
}

fn update_status(app: &App, apply: impl FnOnce(&mut Status)) {
    let mut data = app.projection.write();
    apply(&mut data.session.status);
    let event = AppEvent::Status {
        status: data.session.status.clone(),
    };
    data.broadcast(event);
}

pub fn set_listen_port(app: &App, port: u16) {
    update_status(app, |status| status.listen_port = port);
}

pub fn apply_settings(app: &App, server: String, listen_port: u16, username: &str) {
    update_status(app, |status| {
        status.server = server;
        status.listen_port = listen_port;
        if !status.logged_in {
            status.username = username.to_owned();
        }
    });
}

pub fn connecting(app: &App) {
    update_status(app, |status| {
        status.connected = true;
        status.logged_in = false;
    });
}

pub fn logged_in(app: &App, username: String, banner: String) {
    update_status(app, |status| {
        status.connected = true;
        status.logged_in = true;
        status.username = username;
        status.banner = banner;
    });
}

pub fn login_failed(app: &App, reason: String, detail: Option<String>) {
    {
        let mut data = app.projection.write();
        data.broadcast(AppEvent::LoginFailed { reason, detail });
    }
    update_status(app, |status| {
        status.connected = false;
        status.logged_in = false;
    });
}

pub fn disconnected(app: &App) {
    let mut data = app.projection.write();
    chat::server_disconnected(&mut data);
    users::server_disconnected(&mut data);
    data.session.status.peer_connections = 0;
    data.broadcast(AppEvent::ConnCount { count: 0 });
    data.session.status.connected = false;
    data.session.status.logged_in = false;
    let event = AppEvent::Status {
        status: data.session.status.clone(),
    };
    data.broadcast(event);
}

pub fn connection_count(app: &App, count: usize) {
    let mut data = app.projection.write();
    data.session.status.peer_connections = count;
    data.broadcast(AppEvent::ConnCount { count });
}

pub fn share_scan_started(app: &App) {
    update_status(app, |status| {
        status.scanning = true;
        status.scan_progress = 0;
    });
}

pub fn share_scan_progress(app: &App, files: u64) {
    update_status(app, |status| status.scan_progress = files);
}

pub fn shares_scanned(app: &App, folders: u32, files: u32) {
    update_status(app, |status| {
        status.shared_folders = folders;
        status.shared_files = files;
        status.scanning = false;
        status.scan_progress = 0;
        status.share_scan_error = None;
    });
}

pub fn share_scan_failed(app: &App, error: String) {
    update_status(app, |status| {
        status.scanning = false;
        status.scan_progress = 0;
        status.share_scan_error = Some(error);
    });
}

pub fn privileges(app: &App, seconds: u32) {
    update_status(app, |status| status.privileges_secs = seconds);
}

pub fn server_message(app: &App, message: String) {
    let mut data = app.projection.write();
    data.broadcast(AppEvent::ServerMessage { message });
}

pub(super) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/connect", post(connect))
        .route("/api/disconnect", post(disconnect))
        .route("/api/reconnect", post(reconnect))
        .route("/api/shares/rescan", post(rescan_shares))
}

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "status": data.session.status() }))
}

async fn connect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.connect().await;
    StatusCode::ACCEPTED
}

async fn disconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.disconnect().await;
    StatusCode::ACCEPTED
}

async fn reconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.reconnect().await;
    StatusCode::ACCEPTED
}

async fn rescan_shares(State(app): State<Arc<App>>) -> StatusCode {
    app.client.rescan_shares().await;
    StatusCode::ACCEPTED
}
