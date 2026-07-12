mod peers;
mod search;
mod server;
mod session;
mod sharing;
mod transfers;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use tracing::{debug, info, warn};

use super::ClientCommand;
use super::downloads::{Downloads, TransferIds};
use super::search::Searches;
use super::uploads::Uploads;
use super::users::Users;
use crate::network::{NetworkHandle, spawn as spawn_network};
use crate::protocol::PeerMessage;
use crate::types::{
    ClientBootstrap, ClientEvent, NetworkCommand, NetworkEvent, Observation, RuntimeConfig,
    TransferDirection, TransferWork,
};

use search::Wishlist;
use session::Session;
use sharing::Sharing;

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

struct ClientActor {
    config: RuntimeConfig,
    net: NetworkHandle,
    events: mpsc::Sender<ClientEvent>,
    transfers: mpsc::Sender<TransferWork>,
    token_counter: Arc<AtomicU32>,
    searches: Searches,
    transfer_ids: TransferIds,
    downloads: Downloads,
    uploads: Uploads,
    users: Users,
    sharing: Sharing,
    session: Session,
    wishlist: Wishlist,
    liked_interests: Vec<String>,
    hated_interests: Vec<String>,
}

pub(crate) async fn run(
    config: ClientBootstrap,
    mut commands: mpsc::Receiver<ClientCommand>,
    events: mpsc::Sender<ClientEvent>,
    transfers: mpsc::Sender<TransferWork>,
    token_counter: Arc<AtomicU32>,
) {
    let (net, mut net_events) = spawn_network();
    let (scan_tx, mut scan_rx) = mpsc::unbounded_channel();

    let mut actor = ClientActor {
        net: net.clone(),
        events,
        transfers,
        token_counter,
        searches: Searches::new(),
        transfer_ids: TransferIds::new(&config.transfers),
        downloads: Downloads::new(
            net.clone(),
            config.runtime.transfers.download_dir.clone(),
            config.runtime.transfers.incomplete_dir.clone(),
        ),
        uploads: Uploads::new(
            net,
            config.runtime.transfers.upload_slots,
            config.runtime.transfers.queue_file_limit,
        ),
        users: Users::new(
            config.buddies.into_iter().collect(),
            config.banned.into_iter().collect(),
            config.ignored.into_iter().collect(),
            config.ip_bans,
        ),
        sharing: Sharing::new(scan_tx, config.scan_cache),
        session: Session::new(),
        wishlist: Wishlist::new(config.wishlist),
        liked_interests: config.liked_interests,
        hated_interests: config.hated_interests,
        config: config.runtime,
    };

    for seed in config.transfers {
        match seed.direction {
            TransferDirection::Download => actor.downloads.seed(seed),
            TransferDirection::Upload => actor.uploads.seed(seed),
        }
    }

    actor.set_transfer_limits(
        actor.config.transfers.upload_limit_kbps,
        actor.config.transfers.download_limit_kbps,
    );
    if actor.config.login.username.is_empty() {
        info!("no login configured, waiting for credentials");
    } else {
        actor.connect();
    }
    actor.start_scan();

    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        let reconnect_deadline = actor.session.reconnect_at.unwrap_or_else(Instant::now);
        let wish_deadline = actor.wishlist.at.unwrap_or_else(Instant::now);
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
                    None => {
                        warn!("network actor terminated, stopping client actor");
                        return;
                    }
                }
            }
            result = scan_rx.recv() => {
                if let Some((generation, update)) = result {
                    actor.handle_scan_update(generation, update);
                }
            }
            _ = sweep.tick() => actor.sweep(),
            _ = sleep_until(reconnect_deadline), if actor.session.reconnect_at.is_some() => {
                actor.session.reconnect_at = None;
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

    fn ack<T>(ack: oneshot::Sender<T>, value: T) {
        if ack.send(value).is_err() {
            debug!("command acknowledgement dropped, caller went away");
        }
    }

    fn next_token(&self) -> u32 {
        self.token_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn handle_command(&mut self, command: ClientCommand) {
        match command {
            ClientCommand::Search {
                token,
                query,
                scope,
            } => self.start_search(token, query, scope),
            ClientCommand::CancelSearch { token } => {
                self.searches.remove(token);
                self.net.send(NetworkCommand::DisallowSearchToken(token));
            }
            ClientCommand::Download {
                username,
                virtual_path,
                size,
                attributes,
                ack,
            } => self.enqueue_download(username, virtual_path, size, attributes, ack),
            ClientCommand::RetryDownload { id, ack } => self.retry_download(id, ack),
            ClientCommand::AbortTransfer { direction, id, ack } => {
                self.abort_transfer(direction, id, ack);
            }
            ClientCommand::ClearTransfers {
                direction,
                statuses,
                ack,
            } => self.clear_transfers(direction, statuses, ack),
            ClientCommand::ClearAllTransfers { direction, ack } => {
                self.clear_all_transfers(direction, ack);
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
            } => self.set_user_restriction(username, restriction),
            ClientCommand::AddWish { term } => self.add_wish(term),
            ClientCommand::RemoveWish { term } => self.remove_wish(&term),
            ClientCommand::RescanShares => self.start_scan(),
            ClientCommand::ApplyConfig { config, ack } => {
                self.apply_config(config);
                Self::ack(ack, ());
            }
            ClientCommand::Server(request) => self.net.server(request),
            ClientCommand::Connect => {
                self.session.reconnect_at = None;
                self.connect();
            }
            ClientCommand::Reconnect => self.reconnect(),
            ClientCommand::Disconnect => {
                self.session.reconnect_at = None;
                self.session.reconnect_now = false;
                self.net.send(NetworkCommand::ServerDisconnect);
            }
        }
    }

    fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::LoggedIn { username, banner } => self.handle_logged_in(username, banner),
            NetworkEvent::LoginRejected { reason, detail } => {
                self.emit(ClientEvent::LoginFailed { reason, detail });
            }
            NetworkEvent::ServerDisconnected { manual } => self.handle_server_disconnected(manual),
            NetworkEvent::ServerMessage(message) => self.handle_server_message(message),
            NetworkEvent::PeerMessage {
                username, message, ..
            } => {
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
                ..
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
            } => self.handle_file_transfer_init(&username, token, conn_id),
            NetworkEvent::FileDownloadProgress {
                username,
                token,
                bytes_left,
            } => {
                let updates = self
                    .downloads
                    .handle_download_progress(&username, token, bytes_left);
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
                error,
            } => {
                let updates =
                    self.uploads
                        .handle_transfer_error(&username, token, &error, &self.users);
                self.emit_transfers(updates);
            }
            NetworkEvent::FileConnectionClosed {
                username,
                token,
                conn_id,
            } => self.handle_file_connection_closed(&username, token, conn_id),
        }
    }
}
