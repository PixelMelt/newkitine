use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use serde_json::json;

use super::contract::AppEvent;
use super::state::App;

#[derive(Default, Clone, Serialize)]
pub struct Status {
    pub connected: bool,
    pub logged_in: bool,
    pub username: String,
    pub server: String,
    pub banner: String,
    pub listen_port: u16,
    pub shared_folders: u32,
    pub shared_files: u32,
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
    let mut data = app.data.write().unwrap();
    apply(&mut data.session.status);
    let event = AppEvent::Status {
        status: data.session.status.clone(),
    };
    app.broadcast(&mut data, event);
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
        let mut data = app.data.write().unwrap();
        app.broadcast(&mut data, AppEvent::LoginFailed { reason, detail });
    }
    update_status(app, |status| {
        status.connected = false;
        status.logged_in = false;
    });
}

pub fn disconnected(app: &App) {
    update_status(app, |status| {
        status.connected = false;
        status.logged_in = false;
    });
}

pub fn connection_count(app: &App, count: usize) {
    let mut data = app.data.write().unwrap();
    data.session.status.peer_connections = count;
    app.broadcast(&mut data, AppEvent::ConnCount { count });
}

pub fn shares_scanned(app: &App, folders: u32, files: u32) {
    update_status(app, |status| {
        status.shared_folders = folders;
        status.shared_files = files;
    });
}

pub fn privileges(app: &App, seconds: u32) {
    update_status(app, |status| status.privileges_secs = seconds);
}

pub fn server_message(app: &App, message: String) {
    let mut data = app.data.write().unwrap();
    app.broadcast(&mut data, AppEvent::ServerMessage { message });
}

pub async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "status": data.session.status() }))
}

pub async fn connect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.connect().await;
    StatusCode::ACCEPTED
}

pub async fn disconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.disconnect().await;
    StatusCode::ACCEPTED
}

pub async fn reconnect(State(app): State<Arc<App>>) -> StatusCode {
    app.client.reconnect().await;
    StatusCode::ACCEPTED
}

pub async fn rescan_shares(State(app): State<Arc<App>>) -> StatusCode {
    app.client.rescan_shares().await;
    StatusCode::ACCEPTED
}
