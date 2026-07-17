use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use tower_http::services::{ServeDir, ServeFile};

use super::state::App;
use super::{chat, interests, search, session, settings, stats, transfers, users};

pub fn require_login(app: &super::state::App) -> Result<(), axum::http::StatusCode> {
    if app.projection.read().session.status().logged_in {
        Ok(())
    } else {
        Err(axum::http::StatusCode::CONFLICT)
    }
}

pub fn db_failed(error: sqlx::Error) -> axum::http::StatusCode {
    tracing::error!(%error, "database write failed");
    axum::http::StatusCode::INTERNAL_SERVER_ERROR
}

pub fn limit_rejected(requested: u32, max: u32) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({
            "error": format!("limit {requested} exceeds the maximum of {max}"),
        })),
    )
        .into_response()
}

pub fn router(app: Arc<App>, web_root: &str) -> Router {
    let static_files =
        ServeDir::new(web_root).fallback(ServeFile::new(format!("{web_root}/index.html")));
    Router::new()
        .merge(session::router())
        .merge(search::router())
        .merge(transfers::router())
        .merge(users::router())
        .merge(chat::router())
        .merge(interests::router())
        .merge(stats::router())
        .merge(settings::router())
        .route("/api/ws", get(ws_upgrade))
        .fallback_service(static_files)
        .with_state(app)
}

async fn ws_upgrade(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(app, socket))
}

async fn ws_session(app: Arc<App>, mut socket: WebSocket) {
    let mut events = app.projection.subscribe();
    let snapshot = app.projection.snapshot_json(&app.settings);
    if socket.send(Message::Text(snapshot.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            message = socket.recv() => match message {
                Some(Ok(_)) => continue,
                _ => return,
            },
        }
    }
}
