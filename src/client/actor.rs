use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tracing::{info, warn};

use super::downloads::Downloads;
use super::search::{Searches, sanitize_search_term};
use super::shares::{self, SharesIndex};
use super::uploads::Uploads;
use super::users::Users;
use super::{ClientCommand, ClientConfig, ClientEvent};
use crate::network::{NetworkCommand, NetworkEvent, NetworkHandle, spawn as spawn_network};
use crate::protocol::{PeerMessage, ServerRequest, ServerResponse};
use crate::types::{Observation, Restriction, SearchScope, TransferDirection, UserStatus};

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(15);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(300);
const DEFAULT_WISHLIST_INTERVAL: Duration = Duration::from_secs(720);

struct ClientActor {
    config: ClientConfig,
    net: NetworkHandle,
    events: mpsc::UnboundedSender<ClientEvent>,
    token_counter: Arc<AtomicU32>,
    searches: Searches,
    downloads: Downloads,
    uploads: Uploads,
    users: Users,
    shares: Option<SharesIndex>,
    scan_results: mpsc::UnboundedSender<SharesIndex>,
    excluded_phrases: Vec<String>,
    logged_in: bool,
    connected: bool,
    reconnect_now: bool,
    reconnect_at: Option<Instant>,
    reconnect_delay: Duration,
    wishlist: Vec<String>,
    wishlist_interval: Duration,
    wish_cursor: usize,
    wish_at: Option<Instant>,
}

pub(crate) async fn run(
    config: ClientConfig,
    mut commands: mpsc::UnboundedReceiver<ClientCommand>,
    events: mpsc::UnboundedSender<ClientEvent>,
    token_counter: Arc<AtomicU32>,
) {
    let (net, mut net_events) = spawn_network();
    let (scan_tx, mut scan_rx) = mpsc::unbounded_channel();

    let mut actor = ClientActor {
        net: net.clone(),
        events,
        token_counter,
        searches: Searches::new(),
        downloads: Downloads::new(
            net.clone(),
            config.download_dir.clone(),
            config.incomplete_dir.clone(),
        ),
        uploads: Uploads::new(net, config.upload_slots, config.queue_file_limit),
        users: Users::new(
            config.buddies.iter().cloned().collect(),
            config.banned.iter().cloned().collect(),
            config.ignored.iter().cloned().collect(),
            config.ip_bans.clone(),
        ),
        shares: None,
        scan_results: scan_tx,
        excluded_phrases: Vec::new(),
        logged_in: false,
        connected: false,
        reconnect_now: false,
        reconnect_at: None,
        reconnect_delay: RECONNECT_INITIAL_DELAY,
        wishlist: config.wishlist.clone(),
        wishlist_interval: DEFAULT_WISHLIST_INTERVAL,
        wish_cursor: 0,
        wish_at: None,
        config,
    };

    actor.set_transfer_limits(
        actor.config.upload_limit_kbps,
        actor.config.download_limit_kbps,
    );
    if actor.config.username.is_empty() {
        info!("no login configured, waiting for credentials");
    } else {
        actor.connect();
    }
    actor.start_scan();

    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        let reconnect_deadline = actor.reconnect_at.unwrap_or_else(Instant::now);
        let wish_deadline = actor.wish_at.unwrap_or_else(Instant::now);
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(command) => actor.handle_command(command),
                    None => {
                        actor.net.send(NetworkCommand::Quit);
                        return;
                    }
                }
            }
            event = net_events.recv() => {
                match event {
                    Some(event) => actor.handle_network_event(event),
                    None => return,
                }
            }
            index = scan_rx.recv() => {
                if let Some(index) = index {
                    actor.handle_scan_complete(index);
                }
            }
            _ = sweep.tick() => actor.sweep(),
            _ = sleep_until(reconnect_deadline), if actor.reconnect_at.is_some() => {
                actor.reconnect_at = None;
                info!("reconnecting to server");
                actor.connect();
            }
            _ = sleep_until(wish_deadline), if actor.wish_at.is_some() => {
                actor.do_wishlist_search();
            }
        }
    }
}

impl ClientActor {
    fn emit(&self, event: ClientEvent) {
        let _ = self.events.send(event);
    }

    fn next_token(&self) -> u32 {
        self.token_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn connect(&mut self) {
        self.connected = true;
        self.net.send(NetworkCommand::ServerConnect {
            address: self.config.server,
            username: self.config.username.clone(),
            password: self.config.password.clone(),
            listen_port: self.config.listen_port,
        });
    }

    fn reconnect(&mut self) {
        self.reconnect_at = None;
        if self.connected {
            self.reconnect_now = true;
            self.net.send(NetworkCommand::ServerDisconnect);
        } else {
            self.connect();
        }
    }

    fn set_transfer_limits(&self, upload_kbps: u32, download_kbps: u32) {
        self.net.send(NetworkCommand::SetTransferLimits {
            upload_bps: upload_kbps as u64 * 1024,
            download_bps: download_kbps as u64 * 1024,
        });
    }

    fn start_scan(&self) {
        if self.config.shared_folders.is_empty() {
            return;
        }
        let shared_folders = self.config.shared_folders.clone();
        let results = self.scan_results.clone();
        tokio::task::spawn_blocking(move || {
            let _ = results.send(shares::scan(&shared_folders));
        });
    }

    fn handle_scan_complete(&mut self, index: SharesIndex) {
        let (folders, files) = index.counts();
        self.shares = Some(index);
        if self.logged_in {
            self.net
                .server(ServerRequest::SharedFoldersFiles { folders, files });
        }
        self.emit(ClientEvent::SharesScanned { folders, files });
    }

    fn sweep(&mut self) {
        for update in self.downloads.sweep_request_timeouts() {
            self.emit(ClientEvent::Download(update));
        }
        for update in self.uploads.sweep_request_timeouts(&self.users) {
            self.emit(ClientEvent::Upload(update));
        }
    }

    fn schedule_wishlist(&mut self) {
        self.wish_at = if self.logged_in && !self.wishlist.is_empty() {
            Some(Instant::now() + self.wishlist_interval)
        } else {
            None
        };
    }

    fn do_wishlist_search(&mut self) {
        if self.wishlist.is_empty() {
            self.wish_at = None;
            return;
        }
        let term = self.wishlist[self.wish_cursor % self.wishlist.len()].clone();
        self.wish_cursor += 1;
        let token = self.next_token();
        self.searches.add(token, term.clone());
        self.net.send(NetworkCommand::AllowSearchToken(token));
        self.net.server(ServerRequest::WishlistSearch {
            token,
            search_term: sanitize_search_term(&term),
        });
        self.schedule_wishlist();
    }

    fn handle_command(&mut self, command: ClientCommand) {
        match command {
            ClientCommand::Search {
                token,
                query,
                scope,
            } => {
                self.searches.add(token, query.clone());
                self.net.send(NetworkCommand::AllowSearchToken(token));
                let search_term = sanitize_search_term(&query);
                match scope {
                    SearchScope::Global => {
                        self.net
                            .server(ServerRequest::FileSearch { token, search_term });
                    }
                    SearchScope::Room(room) => {
                        self.net.server(ServerRequest::RoomSearch {
                            room,
                            token,
                            search_term,
                        });
                    }
                    SearchScope::Buddies => {
                        for buddy in &self.users.buddies {
                            self.net.server(ServerRequest::UserSearch {
                                search_username: buddy.clone(),
                                token,
                                search_term: search_term.clone(),
                            });
                        }
                    }
                    SearchScope::User(username) => {
                        self.net.server(ServerRequest::UserSearch {
                            search_username: username,
                            token,
                            search_term,
                        });
                    }
                }
            }
            ClientCommand::CancelSearch { token } => {
                self.searches.remove(token);
                self.net.send(NetworkCommand::DisallowSearchToken(token));
            }
            ClientCommand::Download {
                username,
                virtual_path,
                size,
                attributes,
            } => {
                self.downloads
                    .enqueue(username, virtual_path, size, attributes);
            }
            ClientCommand::AbortDownload {
                username,
                virtual_path,
            } => {
                self.downloads.abort(&username, &virtual_path);
            }
            ClientCommand::AbortUpload {
                username,
                virtual_path,
            } => {
                self.uploads.abort(&username, &virtual_path, &self.users);
            }
            ClientCommand::BrowseUser { username } => {
                self.net
                    .send(NetworkCommand::AllowSharedListUser(username.clone()));
                self.net.peer(username, PeerMessage::SharedFileListRequest);
            }
            ClientCommand::RequestUserInfo { username } => {
                self.net
                    .send(NetworkCommand::AllowUserInfoUser(username.clone()));
                self.net.peer(username, PeerMessage::UserInfoRequest);
            }
            ClientCommand::AddBuddy { username } => {
                self.users.add_buddy(&self.net, username);
            }
            ClientCommand::RemoveBuddy { username } => {
                self.users.remove_buddy(&self.net, &username);
            }
            ClientCommand::BanUser { username } => {
                self.users.banned.insert(username);
            }
            ClientCommand::UnbanUser { username } => {
                self.users.banned.remove(&username);
            }
            ClientCommand::IgnoreUser { username } => {
                self.users.ignored.insert(username);
            }
            ClientCommand::UnignoreUser { username } => {
                self.users.ignored.remove(&username);
            }
            ClientCommand::SetIpBans { patterns } => {
                self.users.set_ip_bans(patterns);
            }
            ClientCommand::SetUserRestriction {
                username,
                restriction,
            } => {
                if let Restriction::Denied { reason } = &restriction {
                    let updates = self.uploads.deny_all(&username, reason, &self.users);
                    for update in updates {
                        self.emit(ClientEvent::Upload(update));
                    }
                }
                self.users.set_restriction(username, restriction);
                self.uploads.check_queue(&self.users);
            }
            ClientCommand::AddWish { term } => {
                if !self.wishlist.contains(&term) {
                    self.wishlist.push(term);
                    if self.wish_at.is_none() {
                        self.schedule_wishlist();
                    }
                }
            }
            ClientCommand::RemoveWish { term } => {
                self.wishlist.retain(|wish| *wish != term);
                if self.wishlist.is_empty() {
                    self.wish_at = None;
                }
            }
            ClientCommand::RescanShares => self.start_scan(),
            ClientCommand::SetTransferLimits {
                upload_kbps,
                download_kbps,
            } => {
                self.set_transfer_limits(upload_kbps, download_kbps);
            }
            ClientCommand::SetListenPort { port } => {
                if self.config.listen_port != port {
                    info!(port, "listen port changed, reconnecting");
                    self.config.listen_port = port;
                    self.reconnect();
                }
            }
            ClientCommand::SetLogin {
                server,
                username,
                password,
            } => {
                self.config.server = server;
                self.config.username = username;
                self.config.password = password;
                if self.connected {
                    self.reconnect();
                } else if !self.config.username.is_empty() {
                    self.connect();
                }
            }
            ClientCommand::SetDescription { description } => {
                self.config.description = description;
            }
            ClientCommand::SetSharedFolders { folders } => {
                self.config.shared_folders = folders;
                if self.config.shared_folders.is_empty() {
                    self.shares = None;
                    if self.logged_in {
                        self.net.server(ServerRequest::SharedFoldersFiles {
                            folders: 0,
                            files: 0,
                        });
                    }
                    self.emit(ClientEvent::SharesScanned {
                        folders: 0,
                        files: 0,
                    });
                } else {
                    self.start_scan();
                }
            }
            ClientCommand::SetUploadSlots {
                upload_slots,
                queue_file_limit,
            } => {
                self.uploads.set_limits(upload_slots, queue_file_limit);
            }
            ClientCommand::SetDownloadDirs {
                download_dir,
                incomplete_dir,
            } => {
                self.downloads.set_dirs(download_dir, incomplete_dir);
            }
            ClientCommand::SetAutoReconnect { enabled } => {
                self.config.auto_reconnect = enabled;
            }
            ClientCommand::Server(request) => self.net.server(request),
            ClientCommand::Connect => {
                self.reconnect_at = None;
                self.connect();
            }
            ClientCommand::Reconnect => self.reconnect(),
            ClientCommand::Disconnect => {
                self.reconnect_at = None;
                self.reconnect_now = false;
                self.net.send(NetworkCommand::ServerDisconnect);
            }
        }
    }

    fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::ServerConnected => {}
            NetworkEvent::LoggedIn {
                username, banner, ..
            } => {
                self.logged_in = true;
                self.reconnect_delay = RECONNECT_INITIAL_DELAY;
                self.reconnect_at = None;
                self.users.watch_buddies(&self.net);
                if let Some(shares) = &self.shares {
                    let (folders, files) = shares.counts();
                    self.net
                        .server(ServerRequest::SharedFoldersFiles { folders, files });
                }
                for thing in &self.config.liked_interests {
                    self.net.server(ServerRequest::AddThingILike {
                        thing: thing.clone(),
                    });
                }
                for thing in &self.config.hated_interests {
                    self.net.server(ServerRequest::AddThingIHate {
                        thing: thing.clone(),
                    });
                }
                self.net.server(ServerRequest::CheckPrivileges);
                self.schedule_wishlist();
                self.emit(ClientEvent::LoggedIn { username, banner });
            }
            NetworkEvent::LoginRejected { reason, detail } => {
                self.emit(ClientEvent::LoginFailed { reason, detail });
            }
            NetworkEvent::ServerDisconnected { manual } => {
                self.logged_in = false;
                self.connected = false;
                self.searches.clear();
                self.downloads.reset();
                self.uploads.reset();
                self.users.reset();
                self.wish_at = None;
                if self.reconnect_now {
                    self.reconnect_now = false;
                    self.connect();
                } else if !manual && self.config.auto_reconnect {
                    self.reconnect_at = Some(Instant::now() + self.reconnect_delay);
                    warn!(delay = ?self.reconnect_delay, "server connection lost, reconnecting");
                    self.reconnect_delay = (self.reconnect_delay * 2).min(RECONNECT_MAX_DELAY);
                }
                self.emit(ClientEvent::Disconnected { manual });
            }
            NetworkEvent::ServerMessage(message) => self.handle_server_message(message),
            NetworkEvent::PeerMessage {
                username,
                conn_id,
                message,
            } => {
                self.handle_peer_message(username, conn_id, message);
            }
            NetworkEvent::PeerConnected {
                username,
                conn_type,
                conn_id,
                ip,
            } => {
                if let Some(ip) = ip
                    && self.users.is_ip_banned(ip)
                {
                    info!(username, %ip, "closing connection from banned IP");
                    self.net.send(NetworkCommand::CloseConnection(conn_id));
                }
                self.emit(ClientEvent::Observed(Observation::PeerConnected {
                    username,
                    ip,
                    conn_type,
                }));
            }
            NetworkEvent::PeerConnectionClosed { .. } => {}
            NetworkEvent::ConnectionCount(count) => {
                self.emit(ClientEvent::ConnectionCount(count));
            }
            NetworkEvent::PeerConnectionError {
                username,
                unsent,
                is_offline,
                ..
            } => {
                for update in self
                    .downloads
                    .handle_peer_connection_error(&username, &unsent, is_offline)
                {
                    self.emit(ClientEvent::Download(update));
                }
                for update in self.uploads.handle_peer_connection_error(
                    &username,
                    &unsent,
                    is_offline,
                    &self.users,
                ) {
                    self.emit(ClientEvent::Upload(update));
                }
            }
            NetworkEvent::DistributedSearch {
                username,
                token,
                search_term,
            } => {
                self.respond_to_search(&username, token, &search_term);
            }
            NetworkEvent::FileTransferInit {
                username,
                token,
                conn_id,
            } => {
                if self.downloads.owns_token(&username, token) {
                    for update in self
                        .downloads
                        .handle_file_transfer_init(&username, token, conn_id)
                    {
                        self.emit(ClientEvent::Download(update));
                    }
                } else if self.uploads.owns_token(&username, token) {
                    for update in self.uploads.handle_file_transfer_init(
                        &username,
                        token,
                        conn_id,
                        &self.users,
                    ) {
                        self.emit(ClientEvent::Upload(update));
                    }
                } else {
                    self.net.send(NetworkCommand::CloseConnection(conn_id));
                }
            }
            NetworkEvent::FileDownloadProgress {
                username,
                token,
                bytes_left,
                ..
            } => {
                for update in self
                    .downloads
                    .handle_download_progress(&username, token, bytes_left)
                {
                    self.emit(ClientEvent::Download(update));
                }
            }
            NetworkEvent::FileUploadProgress {
                username,
                token,
                offset,
                bytes_sent,
                ..
            } => {
                for update in self
                    .uploads
                    .handle_upload_progress(&username, token, offset, bytes_sent)
                {
                    self.emit(ClientEvent::Upload(update));
                }
            }
            NetworkEvent::FileTransferError {
                username,
                token,
                error,
            } => {
                for update in
                    self.uploads
                        .handle_transfer_error(&username, token, &error, &self.users)
                {
                    self.emit(ClientEvent::Upload(update));
                }
            }
            NetworkEvent::FileConnectionClosed {
                username,
                token,
                conn_id,
            } => {
                for update in self
                    .downloads
                    .handle_file_connection_closed(&username, token, conn_id)
                {
                    self.emit(ClientEvent::Download(update));
                }
                for update in self.uploads.handle_file_connection_closed(
                    &username,
                    token,
                    conn_id,
                    &self.users,
                ) {
                    self.emit(ClientEvent::Upload(update));
                }
            }
        }
    }

    fn respond_to_search(&mut self, username: &str, token: u32, search_term: &str) {
        if username == self.config.username {
            return;
        }
        let blocked = self.users.is_banned(username)
            || matches!(
                self.users.restriction(username),
                Some(Restriction::Denied { .. })
            );
        let results = match (&self.shares, blocked) {
            (Some(shares), false) => shares.search(
                search_term,
                self.users.is_buddy(username),
                &self.excluded_phrases,
            ),
            _ => Vec::new(),
        };
        self.emit(ClientEvent::Observed(Observation::SearchSeen {
            username: username.to_owned(),
            query: search_term.to_owned(),
            matched: !results.is_empty(),
        }));
        if results.is_empty() {
            return;
        }
        self.net.peer(
            username.to_owned(),
            PeerMessage::FileSearchResponse {
                username: self.config.username.clone(),
                token,
                results,
                free_upload_slots: self.uploads.is_new_upload_accepted(),
                upload_speed: self.uploads.upload_speed,
                queue_size: self.uploads.queue_size(),
                unknown: 0,
                private_results: Vec::new(),
            },
        );
    }

    fn handle_server_message(&mut self, message: ServerResponse) {
        match message {
            ServerResponse::GetUserStatus {
                user,
                status,
                privileged,
            } => {
                self.users.handle_user_status(&user, status, privileged);
                if status == UserStatus::Online.as_u32() {
                    self.downloads.retry_offline(&user);
                }
                self.emit(ClientEvent::UserStatus {
                    username: user,
                    status: UserStatus::from_u32(status),
                    privileged,
                });
            }
            ServerResponse::WatchUser {
                ref user,
                user_exists,
                status,
                ref stats,
                ..
            } => {
                self.users
                    .handle_watch_user(user, user_exists, status, stats.clone());
                self.emit(ClientEvent::Server(message));
            }
            ServerResponse::GetUserStats {
                ref user,
                ref stats,
            } => {
                self.users.handle_user_stats(user, stats.clone());
                self.emit(ClientEvent::Server(message));
            }
            ServerResponse::PrivilegedUsers { users } => {
                self.users.handle_privileged_users(users);
            }
            ServerResponse::FileSearch {
                search_username,
                token,
                search_term,
            } => {
                self.respond_to_search(&search_username, token, &search_term);
            }
            ServerResponse::ExcludedSearchPhrases { phrases } => {
                self.excluded_phrases = phrases
                    .into_iter()
                    .map(|phrase| phrase.to_lowercase())
                    .collect();
            }
            ServerResponse::WishlistInterval { seconds } => {
                self.wishlist_interval = Duration::from_secs(seconds.max(1) as u64);
                self.schedule_wishlist();
            }
            ServerResponse::MessageUser {
                message_id,
                timestamp,
                user,
                message,
                is_new_message,
            } => {
                self.net.server(ServerRequest::MessageAcked { message_id });
                if !self.users.is_ignored(&user) {
                    self.emit(ClientEvent::PrivateMessage {
                        username: user,
                        message,
                        timestamp,
                        is_new_message,
                    });
                }
            }
            ServerResponse::SayChatroom {
                room,
                user,
                message,
            } => {
                if !self.users.is_ignored(&user) {
                    self.emit(ClientEvent::RoomMessage {
                        room,
                        username: user,
                        message,
                    });
                }
            }
            other => self.emit(ClientEvent::Server(other)),
        }
    }

    fn handle_peer_message(&mut self, username: String, conn_id: u64, message: PeerMessage) {
        match message {
            PeerMessage::FileSearchResponse {
                token,
                results,
                free_upload_slots,
                upload_speed,
                queue_size,
                ..
            } => {
                if let Some(query) = self.searches.query(token)
                    && !self.users.is_ignored(&username)
                {
                    self.emit(ClientEvent::SearchResults {
                        token,
                        query: query.clone(),
                        username,
                        results,
                        free_upload_slots,
                        upload_speed,
                        queue_size,
                    });
                }
            }
            PeerMessage::TransferRequest {
                direction,
                token,
                file,
                filesize,
            } => match direction {
                TransferDirection::Upload => {
                    self.downloads
                        .handle_transfer_request(&username, token, &file, filesize);
                }
                TransferDirection::Download => {
                    let (updates, accepted) = self.uploads.handle_legacy_transfer_request(
                        &username,
                        token,
                        &file,
                        self.shares.as_ref(),
                        &self.users,
                    );
                    self.emit(ClientEvent::Observed(Observation::QueueRequest {
                        username: username.clone(),
                        virtual_path: file,
                        accepted,
                    }));
                    for update in updates {
                        self.emit(ClientEvent::Upload(update));
                    }
                }
            },
            PeerMessage::TransferResponse {
                token,
                allowed,
                reason,
                ..
            } => {
                let updates = self.uploads.handle_transfer_response(
                    &username,
                    token,
                    allowed,
                    reason.as_deref(),
                    &self.users,
                );
                for update in updates {
                    self.emit(ClientEvent::Upload(update));
                }
            }
            PeerMessage::UploadDenied { file, reason } => {
                for update in self
                    .downloads
                    .handle_upload_denied(&username, &file, &reason)
                {
                    self.emit(ClientEvent::Download(update));
                }
            }
            PeerMessage::UploadFailed { file } => {
                for update in self.downloads.handle_upload_failed(&username, &file) {
                    self.emit(ClientEvent::Download(update));
                }
            }
            PeerMessage::PlaceInQueueResponse { filename, place } => {
                self.emit(ClientEvent::PlaceInQueue {
                    username,
                    virtual_path: filename,
                    place,
                });
            }
            PeerMessage::PlaceInQueueRequest { file, .. } => {
                self.uploads.handle_place_in_queue_request(&username, &file);
            }
            PeerMessage::SharedFileListResponse {
                shares,
                private_shares,
                ..
            } => {
                self.net
                    .send(NetworkCommand::DisallowSharedListUser(username.clone()));
                self.emit(ClientEvent::SharedFileList {
                    username,
                    shares,
                    private_shares,
                });
            }
            PeerMessage::UserInfoResponse {
                description,
                picture,
                total_uploads,
                queue_size,
                slots_available,
                ..
            } => {
                self.net
                    .send(NetworkCommand::DisallowUserInfoUser(username.clone()));
                self.emit(ClientEvent::UserInfo {
                    username,
                    description,
                    picture,
                    total_uploads,
                    queue_size,
                    slots_available,
                });
            }
            PeerMessage::SharedFileListRequest => {
                self.emit(ClientEvent::Observed(Observation::BrowseRequest {
                    username: username.clone(),
                }));
                let shares = match (&self.shares, self.users.is_banned(&username)) {
                    (Some(index), false) => index.browse(self.users.is_buddy(&username)),
                    _ => Vec::new(),
                };
                self.net.peer(
                    username,
                    PeerMessage::SharedFileListResponse {
                        shares,
                        unknown: 0,
                        private_shares: Vec::new(),
                    },
                );
            }
            PeerMessage::UserInfoRequest => {
                self.emit(ClientEvent::Observed(Observation::UserInfoRequest {
                    username: username.clone(),
                }));
                self.net.peer(
                    username,
                    PeerMessage::UserInfoResponse {
                        description: self.config.description.clone(),
                        picture: None,
                        total_uploads: self.uploads.total_slots(),
                        queue_size: self.uploads.queue_size(),
                        slots_available: self.uploads.is_new_upload_accepted(),
                        upload_allowed: Some(0),
                    },
                );
            }
            PeerMessage::QueueUpload { file, .. } => {
                let (updates, accepted) = self.uploads.handle_queue_upload(
                    &username,
                    &file,
                    self.shares.as_ref(),
                    &self.users,
                );
                self.emit(ClientEvent::Observed(Observation::QueueRequest {
                    username: username.clone(),
                    virtual_path: file,
                    accepted,
                }));
                for update in updates {
                    self.emit(ClientEvent::Upload(update));
                }
            }
            PeerMessage::FolderContentsRequest {
                token, directory, ..
            } => {
                self.emit(ClientEvent::Observed(Observation::FolderContentsRequest {
                    username: username.clone(),
                }));
                let folders = match (&self.shares, self.users.is_banned(&username)) {
                    (Some(index), false) => {
                        index.folder_contents(&directory, self.users.is_buddy(&username))
                    }
                    _ => Vec::new(),
                };
                self.net.peer(
                    username,
                    PeerMessage::FolderContentsResponse {
                        token,
                        directory,
                        folders,
                    },
                );
            }
            other => self.emit(ClientEvent::Peer {
                username,
                conn_id,
                message: other,
            }),
        }
    }
}
