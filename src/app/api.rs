use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use tower_http::services::{ServeDir, ServeFile};

use super::state::App;
use super::transfers::http as transfers;
use super::users::http as users;
use super::{chat, interests, search, session, settings, stats};

pub fn db_failed(error: sqlx::Error) -> axum::http::StatusCode {
    tracing::error!(%error, "database write failed");
    axum::http::StatusCode::INTERNAL_SERVER_ERROR
}

pub fn router(app: Arc<App>, web_root: &str) -> Router {
    let static_files =
        ServeDir::new(web_root).fallback(ServeFile::new(format!("{web_root}/index.html")));
    Router::new()
        .route("/api/status", get(session::status))
        .route("/api/connect", post(session::connect))
        .route("/api/disconnect", post(session::disconnect))
        .route("/api/reconnect", post(session::reconnect))
        .route(
            "/api/searches",
            get(search::list_searches).post(search::start_search),
        )
        .route(
            "/api/searches/{token}",
            get(search::search_results).delete(search::remove_search),
        )
        .route(
            "/api/downloads",
            get(transfers::list_downloads).post(transfers::enqueue_download),
        )
        .route(
            "/api/downloads/folder",
            post(transfers::enqueue_folder_download),
        )
        .route("/api/downloads/abort", post(transfers::abort_download))
        .route("/api/downloads/retry", post(transfers::retry_download))
        .route("/api/downloads/clear", post(transfers::clear_downloads))
        .route(
            "/api/downloads/clear_all",
            post(transfers::clear_all_downloads),
        )
        .route("/api/uploads", get(transfers::list_uploads))
        .route("/api/uploads/abort", post(transfers::abort_upload))
        .route("/api/uploads/clear", post(transfers::clear_uploads))
        .route("/api/uploads/clear_all", post(transfers::clear_all_uploads))
        .route("/api/users/{username}/browse", post(users::browse_user))
        .route("/api/users/{username}/folders", get(users::user_folders))
        .route("/api/users/{username}/tree", get(users::user_tree))
        .route("/api/users/{username}/files", get(users::user_files))
        .route(
            "/api/users/{username}/info",
            get(users::get_user_info).post(users::request_user_info),
        )
        .route(
            "/api/buddies",
            get(users::list_buddies).post(users::add_buddy),
        )
        .route("/api/buddies/{username}", delete(users::remove_buddy))
        .route("/api/buddies/{username}/note", post(users::set_buddy_note))
        .route("/api/banned", get(users::list_banned).post(users::ban_user))
        .route("/api/banned/{username}", delete(users::unban_user))
        .route(
            "/api/ignored",
            get(users::list_ignored).post(users::ignore_user),
        )
        .route("/api/ignored/{username}", delete(users::unignore_user))
        .route(
            "/api/wishlist",
            get(search::list_wishlist).post(search::add_wish),
        )
        .route("/api/wishlist/remove", post(search::remove_wish))
        .route("/api/chats", get(chat::list_chats))
        .route(
            "/api/chats/{username}",
            get(chat::chat_history)
                .post(chat::send_private_message)
                .delete(chat::close_chat),
        )
        .route("/api/chats/{username}/open", post(chat::open_chat))
        .route("/api/rooms", get(chat::rooms))
        .route("/api/rooms/join", post(chat::join_room))
        .route("/api/rooms/leave", post(chat::leave_room))
        .route(
            "/api/rooms/{room}/messages",
            get(chat::room_history).post(chat::say_room),
        )
        .route("/api/interests", get(interests::interests))
        .route("/api/interests/add", post(interests::add_interest))
        .route("/api/interests/remove", post(interests::remove_interest))
        .route("/api/interests/refresh", post(interests::refresh_interests))
        .route("/api/interests/item", post(interests::item_recommendations))
        .route("/api/shares/rescan", post(session::rescan_shares))
        .route("/api/stats/transfers", get(stats::stats_transfers))
        .route("/api/stats/peers", get(stats::stats_peers))
        .route("/api/stats/verdicts", get(stats::stats_verdicts))
        .route("/api/users/{username}/ban_ip", post(users::ban_user_ip))
        .route(
            "/api/ip_bans",
            get(users::list_ip_bans).post(users::add_ip_ban),
        )
        .route("/api/ip_bans/remove", post(users::remove_ip_ban))
        .route(
            "/api/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
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
