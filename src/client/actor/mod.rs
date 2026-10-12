mod handle;
mod peers;
mod profile;
mod search;
mod server;
mod session;
mod sharing;
mod transfers;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tracing::{info, warn};

use super::transfers::{Downloads, FolderRequests, TransferIds, Uploads};
use super::users::Users;
use crate::client::{ClientBootstrap, ClientEvent, Observation, TransferWork};
use crate::network::{
    AllowedResponse, NetworkCommand, NetworkEvent, NetworkHandle, spawn as spawn_network,
};
use crate::protocol::PeerMessage;
use crate::types::{RuntimeConfig, TransferDirection};

use search::{ActiveSearch, Wishlist};
use session::Session;
use sharing::{ScanJob, Sharing};

pub use handle::Client;

type Command = Box<dyn FnOnce(&mut ClientActor) + Send>;

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

struct ClientActor {
    config: RuntimeConfig,
    net: NetworkHandle,
    events: mpsc::Sender<ClientEvent>,
    transfers: mpsc::Sender<TransferWork>,
    token_counter: Arc<AtomicU32>,
    transfer_ids: TransferIds,
    downloads: Downloads,
    folder_requests: FolderRequests,
    uploads: Uploads,
    users: Users,
    sharing: Sharing,
    session: Session,
    wishlist: Wishlist,
    searches: HashMap<u32, ActiveSearch>,
    liked_interests: Vec<String>,
    hated_interests: Vec<String>,
}

async fn run(
    config: ClientBootstrap,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<ClientEvent>,
    transfers: mpsc::Sender<TransferWork>,
    token_counter: Arc<AtomicU32>,
) {
    let (net, mut net_events) = spawn_network();
    let (scan_tx, mut scan_rx) = mpsc::unbounded_channel();
    let (place_tx, mut place_rx) = mpsc::unbounded_channel();

    let mut actor = ClientActor {
        net: net.clone(),
        events,
        transfers,
        token_counter,
        transfer_ids: TransferIds::new(&config.transfers),
        downloads: Downloads::new(
            net.clone(),
            config.runtime.transfers.download_dir.clone(),
            config.runtime.transfers.incomplete_dir.clone(),
            config.runtime.transfers.download_username_subfolders,
            place_tx,
        ),
        folder_requests: FolderRequests::new(net.clone()),
        uploads: Uploads::new(
            net.clone(),
            config.runtime.transfers.upload_slots,
            config.runtime.transfers.queue_file_limit,
            config.runtime.transfers.queue_size_limit_mb,
            config.runtime.transfers.banned_message.clone(),
        ),
        users: Users::new(
            config.buddies.into_iter().collect(),
            config.banned.into_iter().collect(),
            config.ignored.into_iter().collect(),
            config.ip_bans.clone(),
        ),
        sharing: Sharing::new(scan_tx, config.scan_cache),
        session: Session::new(),
        wishlist: Wishlist::default(),
        searches: HashMap::new(),
        liked_interests: config.liked_interests,
        hated_interests: config.hated_interests,
        config: config.runtime,
    };

    actor
        .users
        .set_login_username(actor.config.login.username.clone());
    actor
        .net
        .send(NetworkCommand::BanAddresses(config.ip_bans.clone()));
    for seed in config.transfers {
        match seed.direction {
            TransferDirection::Download => actor.downloads.seed(seed),
            TransferDirection::Upload => actor.uploads.seed(seed),
        }
    }
    for term in config.wishlist {
        actor.add_wish(term);
    }

    actor.set_transfer_limits(
        actor.config.transfers.upload_limit_kbps,
        actor.config.transfers.download_limit_kbps,
    );
    if actor.config.login.username.is_empty() {
        info!("no login configured, waiting for credentials");
    } else if config.connect_on_startup {
        actor.connect();
    }
    actor.start_scan(ScanJob {
        install_cached: true,
        walk: config.scan_on_startup,
    });

    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        let reconnect_deadline = actor.session.reconnect_at.unwrap_or_else(Instant::now);
        let wish_deadline = actor.wishlist.at.unwrap_or_else(Instant::now);
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(command) => command(&mut actor),
                    None => {
                        actor.net.send(NetworkCommand::Quit);
                        return;
                    }
                }
            }
            event = net_events.recv() => {
                match event {
                    Some(event) => actor.handle_network_event(event),
                    None => {
                        warn!("network actor terminated, stopping client actor");
                        return;
                    }
                }
            }
            result = scan_rx.recv() => {
                let (generation, update) =
                    result.expect("scan channel closed while the actor holds the sender");
                actor.handle_scan_update(generation, update);
            }
            result = place_rx.recv() => {
                let (key, result) =
                    result.expect("placement channel closed while the actor holds the sender");
                let updates = actor.downloads.handle_placement_done(&key, result);
                actor.emit_transfers(updates);
            }
            _ = sweep.tick() => actor.sweep(),
            _ = sleep_until(reconnect_deadline), if actor.session.reconnect_at.is_some() => {
                info!("reconnecting to server");
                actor.connect();
            }
            _ = sleep_until(wish_deadline), if actor.wishlist.at.is_some() => {
                actor.do_wishlist_search();
            }
        }
    }
}

impl ClientActor {
    fn emit(&self, event: ClientEvent) {
        match self.events.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(_)) => panic!("client event queue overflowed"),
        }
    }

    fn emit_transfer_work(&self, work: TransferWork) {
        match self.transfers.try_send(work) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                panic!("transfer persistence cannot keep up, transfer queue overflowed")
            }
        }
    }

    fn next_token(&self) -> u32 {
        self.token_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn browse_user(&mut self, username: String) {
        self.net
            .allow(AllowedResponse::SharedList(username.clone()));
        self.net.peer(username, PeerMessage::SharedFileListRequest);
    }

    fn request_user_info(&mut self, username: String) {
        self.net.allow(AllowedResponse::UserInfo(username.clone()));
        self.net.peer(username, PeerMessage::UserInfoRequest);
    }

    fn remove_buddy(&mut self, username: &str) {
        let keep_watch =
            username == self.config.login.username || self.downloads.needs_watch(username);
        self.users.remove_buddy(&self.net, username, keep_watch);
    }

    fn set_ip_bans(&mut self, patterns: Vec<String>) {
        self.net
            .send(NetworkCommand::BanAddresses(patterns.clone()));
        self.users.set_ip_bans(patterns);
    }

    fn request_connect(&mut self) {
        if self.session.connected {
            warn!("already connected, ignoring connect request");
        } else {
            self.connect();
        }
    }

    fn disconnect(&mut self) {
        self.session.connect_deferred = false;
        self.session.reconnect_at = None;
        self.session.reconnect_now = false;
        self.net.send(NetworkCommand::ServerDisconnect);
    }

    fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::LoggedIn { username, banner } => self.handle_logged_in(username, banner),
            NetworkEvent::LoginRejected { reason, detail } => {
                self.emit(ClientEvent::LoginFailed { reason, detail });
            }
            NetworkEvent::ListenFailed { port, error } => {
                self.handle_listen_failed(port, error);
            }
            NetworkEvent::ServerDisconnected { manual } => self.handle_server_disconnected(manual),
            NetworkEvent::ServerMessage(message) => self.handle_server_message(message),
            NetworkEvent::PeerMessage { username, message } => {
                self.handle_peer_message(username, message);
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
            NetworkEvent::ConnectionCount(count) => {
                self.emit(ClientEvent::ConnectionCount(count));
            }
            NetworkEvent::PeerConnectionError {
                username,
                unsent,
                is_offline,
            } => self.handle_peer_connection_error(&username, &unsent, is_offline),
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
                direction,
            } => self.handle_file_transfer_init(&username, token, conn_id, direction),
            NetworkEvent::FileDownloadProgress {
                conn_id,
                bytes_left,
            } => {
                let updates = self.downloads.handle_download_progress(conn_id, bytes_left);
                self.emit_transfers(updates);
            }
            NetworkEvent::FileUploadProgress {
                username,
                token,
                offset,
                bytes_sent,
            } => {
                let updates = self
                    .uploads
                    .handle_upload_progress(&username, token, offset, bytes_sent);
                self.emit_transfers(updates);
            }
            NetworkEvent::FileTransferError {
                username,
                token,
                conn_id,
                direction,
                error,
            } => {
                let updates = match direction {
                    TransferDirection::Download => {
                        self.downloads.handle_transfer_error(conn_id, &error)
                    }
                    TransferDirection::Upload => {
                        self.uploads
                            .handle_transfer_error(&username, token, &error, &self.users)
                    }
                };
                self.emit_transfers(updates);
            }
            NetworkEvent::FileConnectionClosed {
                username,
                token,
                conn_id,
                direction,
            } => self.handle_file_connection_closed(&username, token, conn_id, direction),
        }
    }
}
