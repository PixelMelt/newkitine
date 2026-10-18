use std::sync::Arc;

use soulseek::client::{ClientEvent, Observation};
use soulseek::types::{SimilarUser, USERNAME_LIMIT};
use tokio::sync::mpsc;

use super::contract::AppEvent;
use super::state::{App, now};
use super::{behavior, chat, db, interests, peer_history, search, session, users};

pub async fn run(app: Arc<App>, mut events: mpsc::Receiver<ClientEvent>) {
    while let Some(event) = events.recv().await {
        handle(&app, event).await;
    }
    tracing::error!("client event stream ended, client actor is gone");
}

fn stored_names(event: &ClientEvent) -> [Option<&str>; 2] {
    match event {
        ClientEvent::PrivateMessage { username, .. }
        | ClientEvent::WatchedUser { username, .. }
        | ClientEvent::UserStats { username, .. }
        | ClientEvent::PeerAddress { username, .. }
        | ClientEvent::Observed(
            Observation::SearchSeen { username, .. }
            | Observation::QueueRequest { username, .. }
            | Observation::BrowseRequest { username }
            | Observation::UserInfoRequest { username }
            | Observation::FolderContentsRequest { username }
            | Observation::PeerConnected { username, .. },
        ) => [Some(username), None],
        ClientEvent::RoomMessage { room, username, .. } => [Some(room), Some(username)],
        ClientEvent::RoomJoined { room, .. } => [Some(room), None],
        _ => [None, None],
    }
}

async fn handle(app: &Arc<App>, event: ClientEvent) {
    if stored_names(&event)
        .into_iter()
        .flatten()
        .any(|name| name.len() > USERNAME_LIMIT || name == "." || name == "..")
    {
        tracing::warn!("dropping an event whose user or room name cannot be stored or addressed");
        return;
    }
    match event {
        ClientEvent::Connecting => session::connecting(app),
        ClientEvent::LoggedIn { username, banner } => {
            session::logged_in(app, username, banner);
            app.client.request_room_list().await;
            let _mutation = app.list_mutation.lock().await;
            for room in db::load_list(&app.db, "room").await {
                app.client.join_room(&room).await;
            }
        }
        ClientEvent::LoginFailed { reason, detail } => {
            session::login_failed(app, reason, detail);
        }
        ClientEvent::Disconnected => session::disconnected(app),
        ClientEvent::ConnectionCount(count) => session::connection_count(app, count),
        ClientEvent::ShareScanStarted => session::share_scan_started(app),
        ClientEvent::ShareScanProgress { files } => session::share_scan_progress(app, files),
        ClientEvent::SharesInstalled { folders, files } => {
            session::shares_installed(app, folders, files);
        }
        ClientEvent::ShareScanFinished => session::share_scan_finished(app),
        ClientEvent::ShareScanFailed { error } => session::share_scan_failed(app, error),
        ClientEvent::SearchStarted { token, query } => {
            search::search_started(app, token, query).await;
        }
        ClientEvent::SearchResults(result) => search::apply_results(app, result),
        ClientEvent::UserStatus {
            username,
            status,
            privileged,
        } => users::user_status(app, username, status, privileged),
        ClientEvent::WatchedUser {
            username,
            exists,
            status,
            stats,
        } => users::watch_user(app, username, exists, status, stats).await,
        ClientEvent::UserStats { username, stats } => users::user_stats(app, username, stats).await,
        ClientEvent::UserInterests {
            username,
            liked,
            hated,
        } => users::user_interests(app, username, liked, hated),
        ClientEvent::SharedFileList {
            username,
            shares,
            private_shares,
        } => users::shared_file_list(app, username, shares, private_shares).await,
        ClientEvent::BrowseFailed { username } => {
            behavior::browse_failed(app, &username).await;
            app.projection
                .write()
                .broadcast(AppEvent::BrowseFailed { username });
        }
        ClientEvent::FolderRequestFailed {
            username,
            directory,
        } => {
            app.projection
                .write()
                .broadcast(AppEvent::FolderRequestFailed {
                    username,
                    directory,
                });
        }
        ClientEvent::UserInfo(info) => users::user_info_received(app, info),
        ClientEvent::PrivateMessage {
            username,
            message,
            timestamp,
        } => chat::private_message_received(app, username, message, timestamp).await,
        ClientEvent::RoomMessage {
            room,
            username,
            message,
        } => chat::room_message_received(app, room, username, message).await,
        ClientEvent::RoomList { rooms } => chat::room_list(app, rooms),
        ClientEvent::RoomJoined { room, users } => chat::room_joined(app, room, users).await,
        ClientEvent::RoomLeft { room } => chat::room_left(app, room).await,
        ClientEvent::RoomUserJoined { room, username } => {
            chat::room_user(app, room, username, true);
        }
        ClientEvent::RoomUserLeft { room, username } => {
            chat::room_user(app, room, username, false);
        }
        ClientEvent::Recommendations {
            recommendations,
            unrecommendations,
        } => interests::recommendations(app, recommendations, unrecommendations).await,
        ClientEvent::GlobalRecommendations {
            recommendations,
            unrecommendations,
        } => interests::set_recommendations(app, recommendations, unrecommendations, None, true),
        ClientEvent::ItemRecommendations {
            thing,
            recommendations,
            unrecommendations,
        } => interests::set_recommendations(
            app,
            recommendations,
            unrecommendations,
            Some(thing),
            false,
        ),
        ClientEvent::SimilarUsers(users) => interests::similar_users(app, users, None),
        ClientEvent::ItemSimilarUsers { thing, usernames } => {
            let users = usernames
                .into_iter()
                .map(|username| SimilarUser {
                    username,
                    rating: 0,
                })
                .collect();
            interests::similar_users(app, users, Some(thing));
        }
        ClientEvent::PeerAddress { ip, .. } if ip.is_unspecified() => {}
        ClientEvent::PeerAddress { username, ip } => {
            let country = app.geo.as_ref().and_then(|geo| geo.country(ip));
            peer_history::record_address(
                &app.db,
                &username,
                &ip.to_string(),
                country.as_deref(),
                now(),
            )
            .await
            .unwrap_or_else(|error| db::fatal(error));
        }
        ClientEvent::Privileges { seconds } => session::privileges(app, seconds),
        ClientEvent::AdminMessage { message } => session::server_message(app, message),
        ClientEvent::Relogged => session::relogged(app),
        ClientEvent::Observed(observation) => {
            app.stats.record(&observation, app.geo.as_ref());
            if let Observation::QueueRequest {
                username,
                accepted: true,
                ..
            } = &observation
            {
                behavior::queue_request(app, username).await;
            }
        }
    }
}
