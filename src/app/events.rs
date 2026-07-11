use std::sync::Arc;

use base64::Engine;
use serde_json::json;
use tokio::sync::mpsc;

use newkitine::client::{ClientEvent, DownloadUpdate, UploadUpdate};
use newkitine::protocol::ServerResponse;
use newkitine::types::{FileAttributes, UserStatus};

use super::db;
use super::state::{
    App, BuddyView, BrowseView, ChatMessage, RoomEntry, RoomView, SearchResponseView,
    SearchFileView, SearchView, TransferView, UserInfoView, now,
};

const MAX_SEARCH_RESPONSES: usize = 500;
const MAX_ROOM_MESSAGES: usize = 200;

pub fn status_string(status: Option<UserStatus>) -> &'static str {
    match status {
        Some(UserStatus::Online) => "online",
        Some(UserStatus::Away) => "away",
        Some(UserStatus::Offline) => "offline",
        None => "unknown",
    }
}

pub async fn run(app: Arc<App>, mut events: mpsc::UnboundedReceiver<ClientEvent>) {
    while let Some(event) = events.recv().await {
        handle(&app, event).await;
    }
}

fn transfer_event(direction: &str, view: &TransferView) -> serde_json::Value {
    json!({ "type": "transfer", "direction": direction, "transfer": view })
}

async fn upsert_download(app: &App, view: TransferView) {
    db::upsert_transfer(&app.db, "download", &view).await;
    app.broadcast(transfer_event("download", &view));
    app.data.write().unwrap().downloads.insert(
        (view.username.clone(), view.virtual_path.clone()),
        view,
    );
}

async fn upsert_upload(app: &App, view: TransferView) {
    db::upsert_transfer(&app.db, "upload", &view).await;
    app.broadcast(transfer_event("upload", &view));
    app.data.write().unwrap().uploads.insert(
        (view.username.clone(), view.virtual_path.clone()),
        view,
    );
}

fn take_download(app: &App, username: &str, virtual_path: &str) -> Option<TransferView> {
    let data = app.data.read().unwrap();
    let view = data.downloads.get(&(username.to_owned(), virtual_path.to_owned()))?;
    if view.status == "aborted" {
        return None;
    }
    Some(view.clone())
}

fn take_upload(app: &App, username: &str, virtual_path: &str) -> TransferView {
    let data = app.data.read().unwrap();
    data.uploads
        .get(&(username.to_owned(), virtual_path.to_owned()))
        .filter(|view| view.status != "aborted")
        .cloned()
        .unwrap_or_else(|| TransferView {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
            size: 0,
            bytes_done: 0,
            status: "queued".into(),
            file_path: None,
            queue_place: 0,
            attributes: FileAttributes::default(),
            updated_at: now(),
        })
}

async fn handle(app: &Arc<App>, event: ClientEvent) {
    match event {
        ClientEvent::LoggedIn { username, banner } => {
            {
                let mut data = app.data.write().unwrap();
                data.status.connected = true;
                data.status.logged_in = true;
                data.status.username = username;
                data.status.banner = banner;
                app.broadcast(json!({ "type": "status", "status": &data.status }));
            }
            app.client.request_room_list();
            for room in db::load_list(&app.db, "room").await {
                app.client.join_room(&room);
            }
        }
        ClientEvent::LoginFailed { reason, detail } => {
            let mut data = app.data.write().unwrap();
            data.status.connected = false;
            data.status.logged_in = false;
            app.broadcast(json!({ "type": "login_failed", "reason": reason, "detail": detail }));
            app.broadcast(json!({ "type": "status", "status": &data.status }));
        }
        ClientEvent::Disconnected { .. } => {
            let mut data = app.data.write().unwrap();
            data.status.connected = false;
            data.status.logged_in = false;
            data.rooms.joined.clear();
            app.broadcast(json!({ "type": "status", "status": &data.status }));
        }
        ClientEvent::SharesScanned { folders, files } => {
            let mut data = app.data.write().unwrap();
            data.status.shared_folders = folders;
            data.status.shared_files = files;
            app.broadcast(json!({ "type": "status", "status": &data.status }));
        }
        ClientEvent::SearchResults {
            token,
            query,
            username,
            results,
            free_upload_slots,
            upload_speed,
            queue_size,
        } => {
            let response = SearchResponseView {
                username,
                free_upload_slots,
                upload_speed,
                queue_size,
                files: results
                    .into_iter()
                    .map(|file| SearchFileView {
                        name: file.name,
                        size: file.size,
                        attributes: file.attributes,
                    })
                    .collect(),
            };
            let mut data = app.data.write().unwrap();
            let index = match data.searches.iter().position(|search| search.token == token) {
                Some(index) => index,
                None => {
                    data.searches.push(SearchView { token, query, results: Vec::new() });
                    app.broadcast(json!({
                        "type": "search_added",
                        "search": data.searches.last().unwrap(),
                    }));
                    data.searches.len() - 1
                }
            };
            let search = &mut data.searches[index];
            if search.results.len() < MAX_SEARCH_RESPONSES {
                search.results.push(response.clone());
                app.broadcast(json!({
                    "type": "search_results",
                    "token": token,
                    "response": response,
                }));
            }
        }
        ClientEvent::UserStatus { username, status, privileged } => {
            let status = status_string(status);
            let mut data = app.data.write().unwrap();
            if let Some(buddy) = data.buddies.get_mut(&username) {
                buddy.status = status.into();
                buddy.privileged = privileged;
            }
            app.broadcast(json!({
                "type": "user_status",
                "username": username,
                "status": status,
                "privileged": privileged,
            }));
        }
        ClientEvent::Download(update) => handle_download(app, update).await,
        ClientEvent::Upload(update) => handle_upload(app, update).await,
        ClientEvent::PlaceInQueue { username, virtual_path, place } => {
            let view = {
                let mut data = app.data.write().unwrap();
                let key = (username, virtual_path);
                let Some(view) = data.downloads.get_mut(&key) else { return };
                view.queue_place = place;
                view.clone()
            };
            app.broadcast(transfer_event("download", &view));
        }
        ClientEvent::SharedFileList { username, shares, private_shares } => {
            let mut data = app.data.write().unwrap();
            data.browses.insert(username.clone(), BrowseView {
                username: username.clone(),
                folders: shares,
                private_folders: private_shares,
                received_at: now(),
            });
            app.broadcast(json!({ "type": "browse_loaded", "username": username }));
        }
        ClientEvent::UserInfo {
            username,
            description,
            picture,
            total_uploads,
            queue_size,
            slots_available,
        } => {
            let mut data = app.data.write().unwrap();
            let info = data.user_infos.entry(username.clone()).or_insert_with(|| {
                UserInfoView { username, ..Default::default() }
            });
            info.received = true;
            info.description = description;
            info.picture_base64 = picture
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
            info.upload_slots = total_uploads;
            info.queue_size = queue_size;
            info.slots_available = slots_available;
            app.broadcast(json!({ "type": "user_info", "info": info.clone() }));
        }
        ClientEvent::PrivateMessage { username, message, timestamp, .. } => {
            let message = ChatMessage {
                sender: username.clone(),
                message,
                timestamp: timestamp as i64,
            };
            db::insert_chat(&app.db, "private", &username, &message).await;
            db::add_to_list(&app.db, "chat", &username).await;
            app.data
                .write()
                .unwrap()
                .private_chats
                .entry(username.clone())
                .or_default()
                .push(message.clone());
            app.broadcast(json!({
                "type": "private_message",
                "username": username,
                "message": message,
            }));
        }
        ClientEvent::RoomMessage { room, username, message } => {
            let message = ChatMessage { sender: username, message, timestamp: now() };
            db::insert_chat(&app.db, "room", &room, &message).await;
            {
                let mut data = app.data.write().unwrap();
                if let Some(view) = data.rooms.joined.get_mut(&room) {
                    view.messages.push(message.clone());
                    if view.messages.len() > MAX_ROOM_MESSAGES {
                        view.messages.remove(0);
                    }
                }
            }
            app.broadcast(json!({ "type": "room_message", "room": room, "message": message }));
        }
        ClientEvent::Server(response) => handle_server(app, response).await,
        ClientEvent::Peer { .. } => {}
    }
}

async fn handle_download(app: &Arc<App>, update: DownloadUpdate) {
    match update {
        DownloadUpdate::Started { username, virtual_path, .. } => {
            let Some(mut view) = take_download(app, &username, &virtual_path) else { return };
            view.status = "transferring".into();
            view.queue_place = 0;
            view.updated_at = now();
            upsert_download(app, view).await;
        }
        DownloadUpdate::Progress { username, virtual_path, bytes_done, size } => {
            let Some(mut view) = take_download(app, &username, &virtual_path) else { return };
            view.status = "transferring".into();
            view.bytes_done = bytes_done;
            view.size = size;
            view.updated_at = now();
            app.broadcast(transfer_event("download", &view));
            app.data
                .write()
                .unwrap()
                .downloads
                .insert((username, virtual_path), view);
        }
        DownloadUpdate::Finished { username, virtual_path, file_path } => {
            let Some(mut view) = take_download(app, &username, &virtual_path) else { return };
            view.status = "finished".into();
            view.bytes_done = view.size;
            view.file_path = Some(file_path.display().to_string());
            view.updated_at = now();
            upsert_download(app, view).await;
        }
        DownloadUpdate::Failed { username, virtual_path, reason } => {
            let Some(mut view) = take_download(app, &username, &virtual_path) else { return };
            view.status = format!("failed: {reason}");
            view.updated_at = now();
            upsert_download(app, view).await;
        }
    }
}

async fn handle_upload(app: &Arc<App>, update: UploadUpdate) {
    match update {
        UploadUpdate::Queued { username, virtual_path, attributes } => {
            let mut view = take_upload(app, &username, &virtual_path);
            view.status = "queued".into();
            view.attributes = attributes;
            view.updated_at = now();
            upsert_upload(app, view).await;
        }
        UploadUpdate::Started { username, virtual_path, real_path } => {
            let mut view = take_upload(app, &username, &virtual_path);
            view.status = "transferring".into();
            view.file_path = Some(real_path.display().to_string());
            view.updated_at = now();
            upsert_upload(app, view).await;
        }
        UploadUpdate::Progress { username, virtual_path, bytes_done, size } => {
            let mut view = take_upload(app, &username, &virtual_path);
            view.status = "transferring".into();
            view.bytes_done = bytes_done;
            view.size = size;
            view.updated_at = now();
            app.broadcast(transfer_event("upload", &view));
            app.data
                .write()
                .unwrap()
                .uploads
                .insert((username, virtual_path), view);
        }
        UploadUpdate::Finished { username, virtual_path } => {
            let mut view = take_upload(app, &username, &virtual_path);
            view.status = "finished".into();
            view.bytes_done = view.size;
            view.updated_at = now();
            upsert_upload(app, view).await;
        }
        UploadUpdate::Failed { username, virtual_path, reason } => {
            let mut view = take_upload(app, &username, &virtual_path);
            view.status = format!("failed: {reason}");
            view.updated_at = now();
            upsert_upload(app, view).await;
        }
    }
}

async fn handle_server(app: &Arc<App>, response: ServerResponse) {
    match response {
        ServerResponse::RoomList { rooms, .. } => {
            let mut data = app.data.write().unwrap();
            data.rooms.available = rooms
                .into_iter()
                .map(|(name, users)| RoomEntry { name, users })
                .collect();
            data.rooms.available.sort_by(|a, b| b.users.cmp(&a.users));
            app.broadcast(json!({ "type": "room_list", "rooms": &data.rooms.available }));
        }
        ServerResponse::JoinRoom { room, users, .. } => {
            db::add_to_list(&app.db, "room", &room).await;
            let mut data = app.data.write().unwrap();
            let mut usernames: Vec<String> =
                users.into_iter().map(|user| user.username).collect();
            usernames.sort();
            let view = RoomView { users: usernames, messages: Vec::new() };
            app.broadcast(json!({ "type": "room_joined", "room": room, "users": view.users }));
            data.rooms.joined.insert(room, view);
        }
        ServerResponse::LeaveRoom { room } => {
            db::remove_from_list(&app.db, "room", &room).await;
            app.data.write().unwrap().rooms.joined.remove(&room);
            app.broadcast(json!({ "type": "room_left", "room": room }));
        }
        ServerResponse::UserJoinedRoom { room, userdata } => {
            let mut data = app.data.write().unwrap();
            if let Some(view) = data.rooms.joined.get_mut(&room) {
                let username = userdata.username;
                if !view.users.contains(&username) {
                    view.users.push(username.clone());
                    view.users.sort();
                }
                app.broadcast(json!({
                    "type": "room_user_joined",
                    "room": room,
                    "username": username,
                }));
            }
        }
        ServerResponse::UserLeftRoom { room, username } => {
            let mut data = app.data.write().unwrap();
            if let Some(view) = data.rooms.joined.get_mut(&room) {
                view.users.retain(|user| user != &username);
                app.broadcast(json!({
                    "type": "room_user_left",
                    "room": room,
                    "username": username,
                }));
            }
        }
        ServerResponse::WatchUser { user, user_exists, status, stats, .. } => {
            let mut data = app.data.write().unwrap();
            if let Some(buddy) = data.buddies.get_mut(&user) {
                if user_exists {
                    buddy.status = status_string(status.and_then(UserStatus::from_u32)).into();
                }
                if let Some(stats) = stats {
                    buddy.stats = stats;
                }
                app.broadcast(json!({ "type": "buddy", "buddy": buddy.clone() }));
            }
        }
        ServerResponse::GetUserStats { user, stats } => {
            let mut data = app.data.write().unwrap();
            if let Some(buddy) = data.buddies.get_mut(&user) {
                buddy.stats = stats.clone();
                app.broadcast(json!({ "type": "buddy", "buddy": buddy.clone() }));
            }
            if let Some(info) = data.user_infos.get_mut(&user) {
                info.stats = Some(stats.clone());
                app.broadcast(json!({ "type": "user_info", "info": info.clone() }));
            }
            app.broadcast(json!({ "type": "user_stats", "username": user, "stats": stats }));
        }
        ServerResponse::UserInterests { user, likes, hates } => {
            let mut data = app.data.write().unwrap();
            if let Some(info) = data.user_infos.get_mut(&user) {
                info.interests_liked = likes;
                info.interests_hated = hates;
                app.broadcast(json!({ "type": "user_info", "info": info.clone() }));
            }
        }
        ServerResponse::Recommendations { recommendations, unrecommendations } => {
            if recommendations.is_empty() && unrecommendations.is_empty() {
                app.client.server_request(
                    newkitine::protocol::ServerRequest::GlobalRecommendations,
                );
                return;
            }
            let mut data = app.data.write().unwrap();
            data.interests.recommendations = recommendations;
            data.interests.unrecommendations = unrecommendations;
            data.interests.recommendations_for = None;
            data.interests.recommendations_global = false;
            app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
        }
        ServerResponse::GlobalRecommendations { recommendations, unrecommendations } => {
            let mut data = app.data.write().unwrap();
            data.interests.recommendations = recommendations;
            data.interests.unrecommendations = unrecommendations;
            data.interests.recommendations_for = None;
            data.interests.recommendations_global = true;
            app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
        }
        ServerResponse::ItemRecommendations { thing, recommendations, unrecommendations } => {
            let mut data = app.data.write().unwrap();
            data.interests.recommendations = recommendations;
            data.interests.unrecommendations = unrecommendations;
            data.interests.recommendations_for = Some(thing);
            data.interests.recommendations_global = false;
            app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
        }
        ServerResponse::SimilarUsers { users } => {
            let mut data = app.data.write().unwrap();
            data.interests.similar_users = users;
            data.interests.similar_users_for = None;
            app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
        }
        ServerResponse::ItemSimilarUsers { thing, usernames } => {
            let mut data = app.data.write().unwrap();
            data.interests.similar_users = usernames
                .into_iter()
                .map(|username| newkitine::types::SimilarUser { username, rating: 0 })
                .collect();
            data.interests.similar_users_for = Some(thing);
            app.broadcast(json!({ "type": "interests", "interests": &data.interests }));
        }
        ServerResponse::CheckPrivileges { seconds } => {
            let mut data = app.data.write().unwrap();
            data.status.privileges_secs = seconds;
            app.broadcast(json!({ "type": "status", "status": &data.status }));
        }
        ServerResponse::AdminMessage { msg } => {
            app.broadcast(json!({ "type": "server_message", "message": msg }));
        }
        _ => {}
    }
}

pub fn buddy_view(username: &str) -> BuddyView {
    BuddyView {
        username: username.to_owned(),
        status: "unknown".into(),
        ..Default::default()
    }
}
