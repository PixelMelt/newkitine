mod distributed;
mod indirect;
mod peers;
mod server;
mod transfers;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::network::conn::{ConnControl, ConnEvent, SharedAllowed, SharedLimits};
use crate::protocol::ServerRequest;
use crate::types::{NetworkCommand, NetworkEvent};

use distributed::Distributed;
use indirect::{Indirect, QueuedItem};
use peers::Peers;
use server::Server;

const SERVER_PING_INTERVAL: Duration = Duration::from_secs(60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
const CONN_EVENT_QUEUE_CAPACITY: usize = 1024;
const ACCEPT_QUEUE_CAPACITY: usize = 64;
const CONN_CONTROL_QUEUE_CAPACITY: usize = 1024;

pub struct Actor {
    commands: mpsc::Receiver<NetworkCommand>,
    events: mpsc::Sender<NetworkEvent>,
    conn_events_tx: mpsc::Sender<ConnEvent>,
    conn_events: mpsc::Receiver<ConnEvent>,
    accepted: mpsc::Receiver<(tokio::net::TcpStream, std::net::SocketAddr)>,
    accepted_tx: mpsc::Sender<(tokio::net::TcpStream, std::net::SocketAddr)>,
    allowed: SharedAllowed,
    limits: SharedLimits,
    server: Server,
    peers: Peers,
    indirect: Indirect,
    distributed: Distributed,
}

impl Actor {
    pub fn new(
        commands: mpsc::Receiver<NetworkCommand>,
        events: mpsc::Sender<NetworkEvent>,
    ) -> Self {
        let (conn_events_tx, conn_events) = mpsc::channel(CONN_EVENT_QUEUE_CAPACITY);
        let (accepted_tx, accepted) = mpsc::channel(ACCEPT_QUEUE_CAPACITY);
        Self {
            commands,
            events,
            conn_events_tx,
            conn_events,
            accepted,
            accepted_tx,
            allowed: Arc::default(),
            limits: Arc::default(),
            server: Server::new(),
            peers: Peers::new(),
            indirect: Indirect::new(),
            distributed: Distributed::new(),
        }
    }

    pub async fn run(mut self) {
        let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
        let mut ping = tokio::time::interval(SERVER_PING_INTERVAL);
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    match command {
                        Some(NetworkCommand::Quit) | None => {
                            self.teardown();
                            return;
                        }
                        Some(command) => self.handle_command(command),
                    }
                }
                event = self.conn_events.recv() => {
                    if let Some(event) = event {
                        self.handle_conn_event(event);
                    }
                }
                stream = self.accepted.recv() => {
                    if let Some((stream, addr)) = stream {
                        self.handle_accepted(stream, addr);
                    }
                }
                _ = sweep.tick() => self.sweep_indirect_requests(),
                _ = ping.tick() => {
                    if self.server.is_logged_in() {
                        self.send_to_server(ServerRequest::ServerPing);
                    }
                }
            }
        }
    }

    fn emit(&self, event: NetworkEvent) {
        match self.events.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(_)) => panic!("network event queue overflowed"),
        }
    }

    fn handle_command(&mut self, command: NetworkCommand) {
        match command {
            NetworkCommand::ServerConnect {
                address,
                username,
                password,
                listen_port,
            } => {
                self.server_connect(address, username, password, listen_port);
            }
            NetworkCommand::ServerDisconnect => {
                self.server.mark_manual_disconnect();
                self.push_server(ConnControl::Close);
            }
            NetworkCommand::SendServerMessage(request) => {
                match &request {
                    ServerRequest::WatchUser { user } => {
                        self.indirect.watch_user(user);
                    }
                    ServerRequest::UnwatchUser { user }
                        if self.server.username() != Some(user.as_str()) =>
                    {
                        self.indirect.unwatch_user(user);
                    }
                    _ => {}
                }
                self.send_to_server(request);
            }
            NetworkCommand::SendPeerMessage { username, message } => {
                self.send_to_peer(username, QueuedItem::Peer(message));
            }
            NetworkCommand::RequestFileConnection { username, token } => {
                self.send_to_peer(username, QueuedItem::FileInit(token));
            }
            NetworkCommand::DownloadFile {
                conn_id,
                file,
                offset,
                bytes_left,
            } => {
                self.push_conn(
                    conn_id,
                    ConnControl::Download {
                        file,
                        offset,
                        bytes_left,
                    },
                );
            }
            NetworkCommand::UploadFile {
                conn_id,
                file,
                size,
            } => {
                self.push_conn(conn_id, ConnControl::Upload { file, size });
            }
            NetworkCommand::SetTransferLimits {
                upload_bps,
                download_bps,
            } => {
                self.limits
                    .upload_bps
                    .store(upload_bps, std::sync::atomic::Ordering::Relaxed);
                self.limits
                    .download_bps
                    .store(download_bps, std::sync::atomic::Ordering::Relaxed);
            }
            NetworkCommand::AllowSearchToken(token) => {
                self.allowed.write().unwrap().search_tokens.insert(token);
            }
            NetworkCommand::DisallowSearchToken(token) => {
                self.allowed.write().unwrap().search_tokens.remove(&token);
            }
            NetworkCommand::AllowSharedListUser(username) => {
                self.allowed
                    .write()
                    .unwrap()
                    .shared_list_users
                    .insert(username);
            }
            NetworkCommand::DisallowSharedListUser(username) => {
                self.allowed
                    .write()
                    .unwrap()
                    .shared_list_users
                    .remove(&username);
            }
            NetworkCommand::AllowUserInfoUser(username) => {
                self.allowed
                    .write()
                    .unwrap()
                    .user_info_users
                    .insert(username);
            }
            NetworkCommand::DisallowUserInfoUser(username) => {
                self.allowed
                    .write()
                    .unwrap()
                    .user_info_users
                    .remove(&username);
            }
            NetworkCommand::CloseConnection(conn_id) => {
                self.close_conn(conn_id);
            }
            NetworkCommand::Quit => unreachable!(),
        }
    }

    fn handle_conn_event(&mut self, event: ConnEvent) {
        match event {
            ConnEvent::ServerMessage(message) => self.handle_server_message(message),
            ConnEvent::ServerClosed { error } => self.handle_server_closed(error),
            ConnEvent::OutgoingEstablished { conn_id } => self.handle_established(conn_id),
            ConnEvent::IncomingInit {
                conn_id,
                init,
                addr,
            } => {
                self.handle_incoming_init(conn_id, init, addr);
            }
            ConnEvent::Peer { conn_id, message } => self.handle_peer_message(conn_id, message),
            ConnEvent::Distrib { conn_id, message } => {
                self.handle_distrib_message(conn_id, message);
            }
            ConnEvent::FileInit { .. }
            | ConnEvent::FileOffsetReceived { .. }
            | ConnEvent::DownloadProgress { .. }
            | ConnEvent::UploadProgress { .. }
            | ConnEvent::FileDone { .. }
            | ConnEvent::FileError { .. } => self.handle_file_event(event),
            ConnEvent::Closed { conn_id, error } => self.handle_conn_closed(conn_id, error),
        }
    }

    fn teardown(&mut self) {
        self.server.teardown();
        self.peers.close_all();
    }
}
