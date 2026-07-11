use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::command::NetworkCommand;
use super::conn::{
    ConnControl, ConnEvent, PeerTask, SharedAllowed, SharedLimits, run_incoming_peer,
    run_outgoing_peer, run_server_conn,
};
use super::event::{ConnId, NetworkEvent};
use crate::protocol::{
    DistributedMessage, LOGIN_MINOR_VERSION, LOGIN_VERSION, LoginOutcome, PeerInitMessage,
    PeerMessage, ServerRequest, ServerResponse, increment_token, initial_token,
};
use crate::types::{ConnectionType, UserStatus};

const INDIRECT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const USER_ADDRESS_TTL: Duration = Duration::from_secs(1800);
const SERVER_PING_INTERVAL: Duration = Duration::from_secs(60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
const MAX_DISTRIB_CHILDREN_LIMIT: u32 = 10;

type InitId = u64;

#[derive(Debug)]
enum QueuedItem {
    Peer(PeerMessage),
    FileInit(u32),
}

struct Init {
    username: String,
    conn_type: ConnectionType,
    indirect_token: u32,
    queued: Vec<QueuedItem>,
    conn_id: Option<ConnId>,
    established: bool,
    created: Instant,
}

struct Conn {
    control: mpsc::UnboundedSender<ConnControl>,
    username: Option<String>,
    conn_type: Option<ConnectionType>,
    init_id: Option<InitId>,
    established: bool,
    file_token: Option<u32>,
}

struct UserAddress {
    addr: Option<SocketAddrV4>,
    updated: Instant,
}

struct PotentialParent {
    conn_id: Option<ConnId>,
    branch_level: Option<i32>,
    branch_root: Option<String>,
}

struct ServerState {
    control: mpsc::UnboundedSender<ConnControl>,
    username: String,
    logged_in: bool,
    manual_disconnect: bool,
}

pub struct Actor {
    commands: mpsc::UnboundedReceiver<NetworkCommand>,
    events: mpsc::UnboundedSender<NetworkEvent>,
    conn_events_tx: mpsc::UnboundedSender<ConnEvent>,
    conn_events: mpsc::UnboundedReceiver<ConnEvent>,
    accepted: mpsc::UnboundedReceiver<(tokio::net::TcpStream, SocketAddr)>,
    accepted_tx: mpsc::UnboundedSender<(tokio::net::TcpStream, SocketAddr)>,
    allowed: SharedAllowed,
    limits: SharedLimits,
    next_conn_id: ConnId,
    next_init_id: InitId,
    server: Option<ServerState>,
    listen_port: u16,
    listener: Option<JoinHandle<()>>,
    conns: HashMap<ConnId, Conn>,
    inits: HashMap<InitId, Init>,
    inits_by_user: HashMap<(String, ConnectionType), InitId>,
    inits_by_token: HashMap<u32, InitId>,
    inits_pending_address: HashMap<String, Vec<InitId>>,
    user_addresses: HashMap<String, Option<UserAddress>>,
    indirect_token: u32,
    parent: Option<String>,
    potential_parents: HashMap<String, PotentialParent>,
    branch_level: u32,
    branch_root: Option<String>,
    is_server_parent: bool,
    child_peers: HashMap<String, ConnId>,
    max_distrib_children: u32,
    upload_speed: u32,
    distrib_parent_min_speed: u32,
    distrib_parent_speed_ratio: u32,
}

impl Actor {
    pub fn new(
        commands: mpsc::UnboundedReceiver<NetworkCommand>,
        events: mpsc::UnboundedSender<NetworkEvent>,
    ) -> Self {
        let (conn_events_tx, conn_events) = mpsc::unbounded_channel();
        let (accepted_tx, accepted) = mpsc::unbounded_channel();
        Self {
            commands,
            events,
            conn_events_tx,
            conn_events,
            accepted,
            accepted_tx,
            allowed: Arc::default(),
            limits: Arc::default(),
            next_conn_id: 1,
            next_init_id: 1,
            server: None,
            listen_port: 0,
            listener: None,
            conns: HashMap::new(),
            inits: HashMap::new(),
            inits_by_user: HashMap::new(),
            inits_by_token: HashMap::new(),
            inits_pending_address: HashMap::new(),
            user_addresses: HashMap::new(),
            indirect_token: initial_token(),
            parent: None,
            potential_parents: HashMap::new(),
            branch_level: 0,
            branch_root: None,
            is_server_parent: false,
            child_peers: HashMap::new(),
            max_distrib_children: 0,
            upload_speed: 0,
            distrib_parent_min_speed: 0,
            distrib_parent_speed_ratio: 1,
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
                    if self.server.as_ref().is_some_and(|server| server.logged_in) {
                        self.send_to_server(ServerRequest::ServerPing);
                    }
                }
            }
        }
    }

    fn emit(&self, event: NetworkEvent) {
        let _ = self.events.send(event);
    }

    fn handle_command(&mut self, command: NetworkCommand) {
        match command {
            NetworkCommand::ServerConnect { address, username, password, listen_port } => {
                self.server_connect(address, username, password, listen_port);
            }
            NetworkCommand::ServerDisconnect => {
                if let Some(server) = &mut self.server {
                    server.manual_disconnect = true;
                    let _ = server.control.send(ConnControl::Close);
                }
            }
            NetworkCommand::SendServerMessage(request) => {
                match &request {
                    ServerRequest::WatchUser { user } => {
                        self.user_addresses.entry(user.clone()).or_insert(None);
                    }
                    ServerRequest::UnwatchUser { user } => {
                        if self.server.as_ref().is_none_or(|server| &server.username != user) {
                            self.user_addresses.remove(user);
                        }
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
            NetworkCommand::DownloadFile { conn_id, file, offset, bytes_left } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    let _ = conn.control.send(ConnControl::Download { file, offset, bytes_left });
                }
            }
            NetworkCommand::UploadFile { conn_id, file, size } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    let _ = conn.control.send(ConnControl::Upload { file, size });
                }
            }
            NetworkCommand::SetTransferLimits { upload_bps, download_bps } => {
                self.limits.upload_bps.store(upload_bps, std::sync::atomic::Ordering::Relaxed);
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
                self.allowed.write().unwrap().shared_list_users.insert(username);
            }
            NetworkCommand::DisallowSharedListUser(username) => {
                self.allowed.write().unwrap().shared_list_users.remove(&username);
            }
            NetworkCommand::AllowUserInfoUser(username) => {
                self.allowed.write().unwrap().user_info_users.insert(username);
            }
            NetworkCommand::DisallowUserInfoUser(username) => {
                self.allowed.write().unwrap().user_info_users.remove(&username);
            }
            NetworkCommand::AllowFolderContents { username, directory } => {
                self.allowed.write().unwrap().folder_contents.insert((username, directory));
            }
            NetworkCommand::DisallowFolderContents { username, directory } => {
                self.allowed.write().unwrap().folder_contents.remove(&(username, directory));
            }
            NetworkCommand::CloseConnection(conn_id) => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    let _ = conn.control.send(ConnControl::Close);
                }
            }
            NetworkCommand::Quit => unreachable!(),
        }
    }

    fn server_connect(
        &mut self,
        address: SocketAddr,
        username: String,
        password: String,
        listen_port: u16,
    ) {
        if self.server.is_some() {
            warn!("already connected to server, ignoring connect request");
            return;
        }
        self.listen_port = listen_port;
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_server_conn(address, self.conn_events_tx.clone(), control_rx));

        let login = ServerRequest::Login {
            username: username.clone(),
            password,
            version: LOGIN_VERSION,
            minor_version: LOGIN_MINOR_VERSION,
        };
        let _ = control_tx.send(ConnControl::Send(login.to_bytes()));
        let _ = control_tx.send(ConnControl::Send(
            ServerRequest::SetWaitPort { port: listen_port as u32 }.to_bytes(),
        ));

        self.branch_root = Some(username.clone());
        self.user_addresses.insert(
            username.clone(),
            Some(UserAddress {
                addr: Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, listen_port)),
                updated: Instant::now(),
            }),
        );
        self.server = Some(ServerState {
            control: control_tx,
            username,
            logged_in: false,
            manual_disconnect: false,
        });

        let listener_events = self.accepted_tx.clone();
        self.listener = Some(tokio::spawn(async move {
            let listener = match TcpListener::bind(("0.0.0.0", listen_port)).await {
                Ok(listener) => listener,
                Err(error) => {
                    warn!(%error, listen_port, "cannot bind listen port");
                    return;
                }
            };
            info!(listen_port, "listening for peer connections");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        if listener_events.send((stream, addr)).is_err() {
                            return;
                        }
                    }
                    Err(error) => debug!(%error, "incoming connection failed"),
                }
            }
        }));
    }

    fn send_to_server(&mut self, request: ServerRequest) {
        match &self.server {
            Some(server) => {
                let _ = server.control.send(ConnControl::Send(request.to_bytes()));
            }
            None => debug!(?request, "dropping server message, not connected"),
        }
    }

    fn server_username(&self) -> Option<&str> {
        self.server.as_ref().map(|server| server.username.as_str())
    }

    fn handle_accepted(&mut self, stream: tokio::net::TcpStream, addr: SocketAddr) {
        let conn_id = self.next_conn_id;
        self.next_conn_id += 1;
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        self.conns.insert(
            conn_id,
            Conn {
                control: control_tx,
                username: None,
                conn_type: None,
                init_id: None,
                established: true,
                file_token: None,
            },
        );
        let task = PeerTask {
            conn_id,
            events: self.conn_events_tx.clone(),
            control: control_rx,
            allowed: self.allowed.clone(),
            limits: self.limits.clone(),
        };
        tokio::spawn(run_incoming_peer(task, stream, addr));
    }

    fn spawn_outgoing_peer(
        &mut self,
        addr: SocketAddrV4,
        username: String,
        first_message: PeerInitMessage,
        conn_type: ConnectionType,
        init_id: Option<InitId>,
    ) -> ConnId {
        let conn_id = self.next_conn_id;
        self.next_conn_id += 1;
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        self.conns.insert(
            conn_id,
            Conn {
                control: control_tx,
                username: Some(username.clone()),
                conn_type: Some(conn_type),
                init_id,
                established: false,
                file_token: None,
            },
        );
        let task = PeerTask {
            conn_id,
            events: self.conn_events_tx.clone(),
            control: control_rx,
            allowed: self.allowed.clone(),
            limits: self.limits.clone(),
        };
        tokio::spawn(run_outgoing_peer(
            task,
            SocketAddr::V4(addr),
            username,
            first_message,
            conn_type,
        ));
        conn_id
    }

    fn send_to_peer(&mut self, username: String, item: QueuedItem) {
        let conn_type = match &item {
            QueuedItem::Peer(_) => ConnectionType::Peer,
            QueuedItem::FileInit(_) => ConnectionType::File,
        };
        if conn_type != ConnectionType::File
            && let Some(&init_id) = self.inits_by_user.get(&(username.clone(), conn_type))
        {
            let init = self.inits.get_mut(&init_id).unwrap();
            init.queued.push(item);
            if init.established {
                self.flush_init_queue(init_id);
            }
            return;
        }
        self.initiate_peer_connection(username, conn_type, Some(item), None);
    }

    fn initiate_peer_connection(
        &mut self,
        username: String,
        conn_type: ConnectionType,
        item: Option<QueuedItem>,
        in_address: Option<SocketAddrV4>,
    ) {
        self.indirect_token = increment_token(self.indirect_token);
        let indirect_token = self.indirect_token;
        let init_id = self.next_init_id;
        self.next_init_id += 1;

        let mut init = Init {
            username: username.clone(),
            conn_type,
            indirect_token,
            queued: Vec::new(),
            conn_id: None,
            established: false,
            created: Instant::now(),
        };
        if let Some(item) = item {
            init.queued.push(item);
        }

        let addr = in_address.or_else(|| self.lookup_fresh_address(&username));

        self.inits.insert(init_id, init);
        self.inits_by_token.insert(indirect_token, init_id);
        if conn_type != ConnectionType::File {
            self.inits_by_user.insert((username.clone(), conn_type), init_id);
        }
        self.send_to_server(ServerRequest::ConnectToPeer {
            token: indirect_token,
            user: username.clone(),
            conn_type,
        });

        match addr {
            Some(addr) => self.connect_direct(init_id, addr),
            None => {
                self.inits_pending_address.entry(username.clone()).or_default().push(init_id);
                self.send_to_server(ServerRequest::GetPeerAddress { user: username });
            }
        }
    }

    fn lookup_fresh_address(&self, username: &str) -> Option<SocketAddrV4> {
        let entry = self.user_addresses.get(username)?.as_ref()?;
        let addr = entry.addr?;
        if addr.port() == 0 {
            return None;
        }
        if Some(username) != self.server_username()
            && entry.updated.elapsed() > USER_ADDRESS_TTL
        {
            return None;
        }
        Some(addr)
    }

    fn connect_direct(&mut self, init_id: InitId, addr: SocketAddrV4) {
        let Some(server_username) = self.server_username().map(str::to_owned) else {
            return;
        };
        let init = self.inits.get_mut(&init_id).unwrap();
        if addr.port() == 0 {
            debug!(username = init.username, "skipping direct connection, invalid port");
            return;
        }
        let username = init.username.clone();
        let conn_type = init.conn_type;
        let first_message = PeerInitMessage::PeerInit { username: server_username, conn_type };
        let conn_id =
            self.spawn_outgoing_peer(addr, username, first_message, conn_type, Some(init_id));
        self.inits.get_mut(&init_id).unwrap().conn_id = Some(conn_id);
    }

    fn flush_init_queue(&mut self, init_id: InitId) {
        let init = self.inits.get_mut(&init_id).unwrap();
        let Some(conn_id) = init.conn_id else { return };
        let username = init.username.clone();
        let queued: Vec<QueuedItem> = init.queued.drain(..).collect();
        let Some(conn) = self.conns.get_mut(&conn_id) else { return };
        let mut sent_file_init = None;
        for item in queued {
            let control = match item {
                QueuedItem::Peer(message) => ConnControl::Send(message.to_bytes()),
                QueuedItem::FileInit(token) => {
                    conn.file_token = Some(token);
                    sent_file_init = Some(token);
                    ConnControl::SendFileInit(token)
                }
            };
            let _ = conn.control.send(control);
        }
        if let Some(token) = sent_file_init {
            self.emit(NetworkEvent::FileTransferInit { username, token, conn_id });
        }
    }

    fn remove_init(&mut self, init_id: InitId) -> Option<Init> {
        let init = self.inits.remove(&init_id)?;
        self.inits_by_token.remove(&init.indirect_token);
        if let Some(existing) = self.inits_by_user.get(&(init.username.clone(), init.conn_type))
            && *existing == init_id
        {
            self.inits_by_user.remove(&(init.username.clone(), init.conn_type));
        }
        if let Some(pending) = self.inits_pending_address.get_mut(&init.username) {
            pending.retain(|&id| id != init_id);
            if pending.is_empty() {
                self.inits_pending_address.remove(&init.username);
            }
        }
        Some(init)
    }

    fn fail_init(&mut self, init_id: InitId, is_offline: bool) {
        let Some(init) = self.remove_init(init_id) else { return };
        if init.established {
            return;
        }
        if let Some(conn_id) = init.conn_id
            && let Some(conn) = self.conns.get(&conn_id)
        {
            let _ = conn.control.send(ConnControl::Close);
        }
        let unsent = init
            .queued
            .into_iter()
            .filter_map(|item| match item {
                QueuedItem::Peer(message) => Some(message),
                _ => None,
            })
            .collect();
        self.emit(NetworkEvent::PeerConnectionError {
            username: init.username,
            conn_type: init.conn_type,
            unsent,
            is_offline,
        });
    }

    fn sweep_indirect_requests(&mut self) {
        let expired: Vec<InitId> = self
            .inits
            .iter()
            .filter(|(_, init)| {
                !init.established && init.created.elapsed() > INDIRECT_REQUEST_TIMEOUT
            })
            .map(|(&id, _)| id)
            .collect();
        for init_id in expired {
            self.fail_init(init_id, false);
        }
    }

    fn handle_conn_event(&mut self, event: ConnEvent) {
        match event {
            ConnEvent::ServerMessage(message) => self.handle_server_message(message),
            ConnEvent::ServerClosed { error } => self.handle_server_closed(error),
            ConnEvent::OutgoingEstablished { conn_id } => self.handle_established(conn_id),
            ConnEvent::IncomingInit { conn_id, init, addr } => {
                self.handle_incoming_init(conn_id, init, addr);
            }
            ConnEvent::Peer { conn_id, message } => self.handle_peer_message(conn_id, message),
            ConnEvent::Distrib { conn_id, message } => {
                self.handle_distrib_message(conn_id, message);
            }
            ConnEvent::FileInit { conn_id, token } => {
                if let Some(conn) = self.conns.get_mut(&conn_id) {
                    conn.file_token = Some(token);
                    let username = conn.username.clone().unwrap_or_default();
                    self.emit(NetworkEvent::FileTransferInit { username, token, conn_id });
                }
            }
            ConnEvent::FileOffsetReceived { conn_id, offset } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    self.emit(NetworkEvent::FileUploadProgress {
                        username: conn.username.clone().unwrap_or_default(),
                        token: conn.file_token.unwrap_or_default(),
                        offset,
                        bytes_sent: 0,
                        speed: 0,
                    });
                }
            }
            ConnEvent::DownloadProgress { conn_id, bytes_left, bytes_received } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    self.emit(NetworkEvent::FileDownloadProgress {
                        username: conn.username.clone().unwrap_or_default(),
                        token: conn.file_token.unwrap_or_default(),
                        bytes_left,
                        speed: bytes_received,
                    });
                }
            }
            ConnEvent::UploadProgress { conn_id, offset, bytes_sent } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    self.emit(NetworkEvent::FileUploadProgress {
                        username: conn.username.clone().unwrap_or_default(),
                        token: conn.file_token.unwrap_or_default(),
                        offset,
                        bytes_sent,
                        speed: 0,
                    });
                }
            }
            ConnEvent::FileDone { conn_id } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    let _ = conn.control.send(ConnControl::Close);
                }
            }
            ConnEvent::FileError { conn_id, error } => {
                if let Some(conn) = self.conns.get(&conn_id) {
                    self.emit(NetworkEvent::FileTransferError {
                        username: conn.username.clone().unwrap_or_default(),
                        token: conn.file_token.unwrap_or_default(),
                        error,
                    });
                }
            }
            ConnEvent::Closed { conn_id, error } => self.handle_conn_closed(conn_id, error),
        }
    }

    fn handle_server_message(&mut self, message: ServerResponse) {
        match &message {
            ServerResponse::Login(outcome) => match outcome {
                LoginOutcome::Success { banner, ip_address, is_supporter } => {
                    let username = match &mut self.server {
                        Some(server) => {
                            server.logged_in = true;
                            server.username.clone()
                        }
                        None => return,
                    };
                    self.emit(NetworkEvent::LoggedIn {
                        username,
                        banner: banner.clone(),
                        ip_address: *ip_address,
                        is_supporter: *is_supporter,
                    });
                    self.send_have_no_parent();
                }
                LoginOutcome::Failure { reason, detail } => {
                    self.emit(NetworkEvent::LoginRejected {
                        reason: reason.clone(),
                        detail: detail.clone(),
                    });
                    if let Some(server) = &mut self.server {
                        server.manual_disconnect = true;
                        let _ = server.control.send(ConnControl::Close);
                    }
                }
            },
            ServerResponse::ConnectToPeer {
                user, conn_type, ip_address, port, token, ..
            } => {
                match ConnectionType::from_str_value(conn_type) {
                    Ok(conn_type) => {
                        let addr = SocketAddrV4::new(*ip_address, *port as u16);
                        let username = user.clone();
                        let pierce_token = *token;
                        if conn_type != ConnectionType::File
                            && self
                                .inits_by_user
                                .contains_key(&(username.clone(), conn_type))
                        {
                            debug!(username, "existing connection, ignoring indirect request");
                        } else {
                            self.spawn_outgoing_peer(
                                addr,
                                username,
                                PeerInitMessage::PierceFireWall { token: pierce_token },
                                conn_type,
                                None,
                            );
                        }
                    }
                    Err(_) => debug!(conn_type, "unknown connection type"),
                }
            }
            ServerResponse::CantConnectToPeer { token } => {
                if let Some(&init_id) = self.inits_by_token.get(token) {
                    self.fail_init(init_id, false);
                }
            }
            ServerResponse::GetUserStatus { user, status, .. } => {
                if *status == UserStatus::Offline.as_u32()
                    && let Some(entry) = self.user_addresses.get_mut(user)
                {
                    *entry = None;
                }
            }
            ServerResponse::GetPeerAddress { user, ip_address, port, .. } => {
                let user_offline = *ip_address == Ipv4Addr::UNSPECIFIED;
                let addr = SocketAddrV4::new(*ip_address, *port as u16);
                let pending = self.inits_pending_address.remove(user).unwrap_or_default();
                for init_id in pending {
                    if user_offline {
                        self.fail_init(init_id, true);
                    } else if self.inits.contains_key(&init_id) {
                        self.connect_direct(init_id, addr);
                    }
                }
                if Some(user.as_str()) != self.server_username()
                    && let Some(entry) = self.user_addresses.get_mut(user)
                {
                    *entry = if user_offline || *port == 0 {
                        None
                    } else {
                        Some(UserAddress { addr: Some(addr), updated: Instant::now() })
                    };
                }
            }
            ServerResponse::WatchUser { user, user_exists, stats, .. } => {
                if Some(user.as_str()) == self.server_username() {
                    if let Some(stats) = stats {
                        self.upload_speed = stats.avgspeed;
                        self.update_max_distrib_children();
                    }
                } else if !user_exists {
                    self.user_addresses.remove(user);
                }
            }
            ServerResponse::GetUserStats { user, stats } => {
                if Some(user.as_str()) == self.server_username() {
                    self.upload_speed = stats.avgspeed;
                    self.update_max_distrib_children();
                }
            }
            ServerResponse::Relogged => {
                if let Some(server) = &mut self.server {
                    server.manual_disconnect = true;
                }
            }
            ServerResponse::PossibleParents { parents } => {
                if self.parent.is_none() {
                    self.close_parent_candidate_connections();
                    self.potential_parents.clear();
                    for parent in parents {
                        let addr =
                            SocketAddrV4::new(parent.ip_address, parent.port as u16);
                        self.potential_parents.insert(
                            parent.username.clone(),
                            PotentialParent {
                                conn_id: None,
                                branch_level: None,
                                branch_root: None,
                            },
                        );
                        self.initiate_peer_connection(
                            parent.username.clone(),
                            ConnectionType::Distributed,
                            None,
                            Some(addr),
                        );
                    }
                }
            }
            ServerResponse::ParentMinSpeed { speed } => {
                self.distrib_parent_min_speed = *speed;
                self.update_max_distrib_children();
            }
            ServerResponse::ParentSpeedRatio { ratio } => {
                self.distrib_parent_speed_ratio = *ratio;
                self.update_max_distrib_children();
            }
            ServerResponse::ResetDistributed => {
                if let Some(parent) = self.parent.take()
                    && let Some(&init_id) =
                        self.inits_by_user.get(&(parent, ConnectionType::Distributed))
                    && let Some(conn_id) = self.inits.get(&init_id).and_then(|init| init.conn_id)
                    && let Some(conn) = self.conns.get(&conn_id)
                {
                    let _ = conn.control.send(ConnControl::Close);
                }
                for conn_id in self.child_peers.values() {
                    if let Some(conn) = self.conns.get(conn_id) {
                        let _ = conn.control.send(ConnControl::Close);
                    }
                }
                self.send_have_no_parent();
            }
            ServerResponse::EmbeddedMessage { distrib_code, distrib_message } => {
                if self.parent.is_none() {
                    if let Ok(DistributedMessage::Search {
                        identifier,
                        search_username,
                        token,
                        search_term,
                    }) = DistributedMessage::parse(*distrib_code, distrib_message)
                        && identifier == 49
                    {
                        if !self.is_server_parent {
                            self.is_server_parent = true;
                            self.branch_level = 0;
                            self.branch_root = self.server_username().map(str::to_owned);
                            if (self.child_peers.len() as u32) < self.max_distrib_children {
                                self.send_to_server(ServerRequest::AcceptChildren {
                                    enabled: true,
                                });
                            }
                        }
                        let forwarded = DistributedMessage::Search {
                            identifier,
                            search_username: search_username.clone(),
                            token,
                            search_term: search_term.clone(),
                        };
                        self.send_to_child_peers(&forwarded);
                        self.emit(NetworkEvent::DistributedSearch {
                            username: search_username,
                            token,
                            search_term,
                        });
                    }
                    return;
                }
                return;
            }
            _ => {}
        }
        self.emit(NetworkEvent::ServerMessage(message));
    }

    fn handle_server_closed(&mut self, error: Option<String>) {
        let Some(server) = self.server.take() else { return };
        if let Some(error) = error {
            warn!(%error, "server connection closed");
        }
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
        for conn in self.conns.values() {
            let _ = conn.control.send(ConnControl::Close);
        }
        self.conns.clear();
        self.inits.clear();
        self.inits_by_user.clear();
        self.inits_by_token.clear();
        self.inits_pending_address.clear();
        self.user_addresses.clear();
        self.parent = None;
        self.potential_parents.clear();
        self.branch_level = 0;
        self.branch_root = None;
        self.is_server_parent = false;
        self.child_peers.clear();
        self.max_distrib_children = 0;
        self.upload_speed = 0;
        {
            let mut allowed = self.allowed.write().unwrap();
            allowed.search_tokens.clear();
            allowed.shared_list_users.clear();
            allowed.user_info_users.clear();
            allowed.folder_contents.clear();
        }
        self.emit(NetworkEvent::ServerDisconnected { manual: server.manual_disconnect });
    }

    fn handle_established(&mut self, conn_id: ConnId) {
        let Some(conn) = self.conns.get_mut(&conn_id) else { return };
        conn.established = true;
        let conn_type = conn.conn_type.unwrap();
        let username = conn.username.clone().unwrap();
        self.emit(NetworkEvent::PeerConnected {
            username: username.clone(),
            conn_type,
            conn_id,
        });
        if let Some(init_id) = self.conns.get(&conn_id).and_then(|conn| conn.init_id) {
            if let Some(init) = self.inits.get_mut(&init_id) {
                init.established = true;
                init.conn_id = Some(conn_id);
                self.flush_init_queue(init_id);
            }
        } else if conn_type == ConnectionType::Distributed {
            self.accept_child_peer(conn_id, &username);
        }
    }

    fn handle_incoming_init(
        &mut self,
        conn_id: ConnId,
        init: PeerInitMessage,
        addr: SocketAddr,
    ) {
        match init {
            PeerInitMessage::PeerInit { username, conn_type } => {
                let Some(conn) = self.conns.get_mut(&conn_id) else { return };
                conn.username = Some(username.clone());
                conn.conn_type = Some(conn_type);
                debug!(username, ?conn_type, %addr, "incoming direct connection");

                if conn_type != ConnectionType::File {
                    let key = (username.clone(), conn_type);
                    let mut inherited = Vec::new();
                    if let Some(&prev_id) = self.inits_by_user.get(&key)
                        && let Some(prev) = self.inits.get_mut(&prev_id)
                    {
                        inherited = prev.queued.drain(..).collect();
                        self.remove_init(prev_id);
                    }
                    let init_id = self.next_init_id;
                    self.next_init_id += 1;
                    self.inits.insert(
                        init_id,
                        Init {
                            username: username.clone(),
                            conn_type,
                            indirect_token: 0,
                            queued: inherited,
                            conn_id: Some(conn_id),
                            established: true,
                            created: Instant::now(),
                        },
                    );
                    self.inits_by_user.insert(key, init_id);
                    self.conns.get_mut(&conn_id).unwrap().init_id = Some(init_id);
                    self.flush_init_queue(init_id);
                }
                self.emit(NetworkEvent::PeerConnected {
                    username: username.clone(),
                    conn_type,
                    conn_id,
                });
                if conn_type == ConnectionType::Distributed {
                    self.accept_child_peer(conn_id, &username);
                }
            }
            PeerInitMessage::PierceFireWall { token } => {
                let Some(&init_id) = self.inits_by_token.get(&token) else {
                    debug!(token, "indirect connection attempt expired, closing");
                    if let Some(conn) = self.conns.get(&conn_id) {
                        let _ = conn.control.send(ConnControl::Close);
                    }
                    return;
                };
                self.inits_by_token.remove(&token);
                let init = self.inits.get_mut(&init_id).unwrap();
                let username = init.username.clone();
                let conn_type = init.conn_type;
                debug!(username, token, "indirect connection established");

                if let Some(previous) = init.conn_id
                    && self.conns.get(&previous).is_some_and(|conn| conn.established)
                {
                    debug!(username, "direct connection already established, keeping it");
                    if let Some(conn) = self.conns.get_mut(&conn_id) {
                        conn.username = Some(username.clone());
                        conn.conn_type = Some(conn_type);
                        let _ = conn.control.send(ConnControl::AssumeIdentity {
                            username,
                            conn_type,
                        });
                    }
                    return;
                }

                if let Some(previous) = init.conn_id.replace(conn_id)
                    && let Some(conn) = self.conns.get(&previous)
                {
                    let _ = conn.control.send(ConnControl::Close);
                    self.conns.get_mut(&previous).unwrap().init_id = None;
                }
                let init = self.inits.get_mut(&init_id).unwrap();
                init.established = true;
                let conn = self.conns.get_mut(&conn_id).unwrap();
                conn.username = Some(username.clone());
                conn.conn_type = Some(conn_type);
                conn.init_id = Some(init_id);
                let _ = conn
                    .control
                    .send(ConnControl::AssumeIdentity { username: username.clone(), conn_type });
                self.emit(NetworkEvent::PeerConnected { username, conn_type, conn_id });
                self.flush_init_queue(init_id);
            }
        }
    }

    fn handle_peer_message(&mut self, conn_id: ConnId, message: PeerMessage) {
        let Some(conn) = self.conns.get(&conn_id) else { return };
        let username = conn.username.clone().unwrap_or_default();
        let close_after = matches!(message, PeerMessage::FileSearchResponse { .. });
        self.emit(NetworkEvent::PeerMessage { username, conn_id, message });
        if close_after && let Some(conn) = self.conns.get(&conn_id) {
            let _ = conn.control.send(ConnControl::Close);
        }
    }

    fn handle_distrib_message(&mut self, conn_id: ConnId, message: DistributedMessage) {
        let Some(conn) = self.conns.get(&conn_id) else { return };
        let username = conn.username.clone().unwrap_or_default();
        match message {
            DistributedMessage::Search { identifier, search_username, token, search_term } => {
                if identifier != 49 {
                    return;
                }
                if self.parent.is_none() {
                    self.adopt_parent(&username);
                }
                if !self.is_parent_conn(conn_id) {
                    if self.parent.is_some() {
                        self.close_conn(conn_id);
                    }
                    return;
                }
                let forwarded = DistributedMessage::Search {
                    identifier,
                    search_username: search_username.clone(),
                    token,
                    search_term: search_term.clone(),
                };
                self.send_to_child_peers(&forwarded);
                self.emit(NetworkEvent::DistributedSearch {
                    username: search_username,
                    token,
                    search_term,
                });
            }
            DistributedMessage::EmbeddedMessage { distrib_code, distrib_message } => {
                if let Ok(unpacked) = DistributedMessage::parse(distrib_code, &distrib_message) {
                    self.handle_distrib_message(conn_id, unpacked);
                }
            }
            DistributedMessage::BranchLevel { level } => {
                if level < 0 {
                    self.close_conn(conn_id);
                    return;
                }
                if self.is_parent_conn(conn_id) {
                    self.branch_level = level as u32 + 1;
                    self.send_to_server(ServerRequest::BranchLevel { value: self.branch_level });
                    let forwarded =
                        DistributedMessage::BranchLevel { level: self.branch_level as i32 };
                    self.send_to_child_peers(&forwarded);
                } else if self.parent.is_none()
                    && let Some(candidate) = self.potential_parents.get_mut(&username)
                {
                    candidate.conn_id = Some(conn_id);
                    candidate.branch_level = Some(level);
                    if level == 0 {
                        candidate.branch_root = Some(username);
                    }
                } else if self.parent.is_some() {
                    self.close_conn(conn_id);
                }
            }
            DistributedMessage::BranchRoot { root_username } => {
                if root_username.is_empty() {
                    self.close_conn(conn_id);
                    return;
                }
                if self.is_parent_conn(conn_id) {
                    self.branch_root = Some(root_username.clone());
                    self.send_to_server(ServerRequest::BranchRoot { user: root_username.clone() });
                    let forwarded = DistributedMessage::BranchRoot { root_username };
                    self.send_to_child_peers(&forwarded);
                } else if self.parent.is_none()
                    && let Some(candidate) = self.potential_parents.get_mut(&username)
                {
                    candidate.conn_id = Some(conn_id);
                    candidate.branch_root = Some(root_username);
                } else if self.parent.is_some() {
                    self.close_conn(conn_id);
                }
            }
            DistributedMessage::Ping | DistributedMessage::ChildDepth { .. } => {}
        }
    }

    fn is_parent_conn(&self, conn_id: ConnId) -> bool {
        let Some(parent) = &self.parent else { return false };
        self.conns
            .get(&conn_id)
            .and_then(|conn| conn.username.as_ref())
            .is_some_and(|username| username == parent)
    }

    fn adopt_parent(&mut self, username: &str) {
        let Some(candidate) = self.potential_parents.get_mut(username) else { return };
        if candidate.conn_id.is_none() {
            return;
        }
        let (Some(branch_level), Some(branch_root)) =
            (candidate.branch_level.take(), candidate.branch_root.take())
        else {
            return;
        };
        info!(username, branch_level, branch_root, "adopting distributed parent");
        self.parent = Some(username.to_owned());
        self.branch_level = branch_level as u32 + 1;
        self.branch_root = Some(branch_root.clone());
        self.is_server_parent = false;

        self.close_parent_candidate_connections();

        self.send_to_server(ServerRequest::HaveNoParent { no_parent: false });
        self.send_to_server(ServerRequest::BranchRoot { user: branch_root.clone() });
        self.send_to_server(ServerRequest::BranchLevel { value: self.branch_level });
        if (self.child_peers.len() as u32) < self.max_distrib_children {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
        }
        let level = DistributedMessage::BranchLevel { level: self.branch_level as i32 };
        let root = DistributedMessage::BranchRoot { root_username: branch_root };
        self.send_to_child_peers(&level);
        self.send_to_child_peers(&root);
        self.child_peers.remove(username);
    }

    fn close_parent_candidate_connections(&mut self) {
        let parent = self.parent.clone();
        for (username, candidate) in &mut self.potential_parents {
            if Some(username) == parent.as_ref() {
                continue;
            }
            if let Some(conn_id) = candidate.conn_id.take()
                && let Some(conn) = self.conns.get(&conn_id)
            {
                let _ = conn.control.send(ConnControl::Close);
            }
        }
    }

    fn send_have_no_parent(&mut self) {
        self.parent = None;
        self.branch_level = 0;
        self.branch_root = self.server_username().map(str::to_owned);
        self.send_to_server(ServerRequest::HaveNoParent { no_parent: true });
        if let Some(root) = self.branch_root.clone() {
            self.send_to_server(ServerRequest::BranchRoot { user: root });
        }
        self.send_to_server(ServerRequest::BranchLevel { value: 0 });
        self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
    }

    fn send_to_child_peers(&mut self, message: &DistributedMessage) {
        let bytes = message.to_bytes();
        for conn_id in self.child_peers.values() {
            if let Some(conn) = self.conns.get(conn_id) {
                let _ = conn.control.send(ConnControl::Send(bytes.clone()));
            }
        }
    }

    fn accept_child_peer(&mut self, conn_id: ConnId, username: &str) {
        if Some(username) == self.server_username() {
            return;
        }
        if self.potential_parents.contains_key(username) {
            return;
        }
        let reject = (self.parent.is_none() && !self.is_server_parent)
            || self.child_peers.contains_key(username)
            || self.child_peers.len() as u32 >= self.max_distrib_children;
        if reject {
            debug!(username, "rejecting distributed child peer");
            self.close_conn(conn_id);
            return;
        }
        self.child_peers.insert(username.to_owned(), conn_id);
        let level = DistributedMessage::BranchLevel { level: self.branch_level as i32 };
        let root = DistributedMessage::BranchRoot {
            root_username: self.branch_root.clone().unwrap_or_default(),
        };
        if let Some(conn) = self.conns.get(&conn_id) {
            let _ = conn.control.send(ConnControl::Send(level.to_bytes()));
            let _ = conn.control.send(ConnControl::Send(root.to_bytes()));
        }
        if self.child_peers.len() as u32 >= self.max_distrib_children {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
        }
    }

    fn update_max_distrib_children(&mut self) {
        let previous = self.max_distrib_children;
        let num_children = self.child_peers.len() as u32;
        if self.upload_speed >= self.distrib_parent_min_speed
            && self.distrib_parent_speed_ratio > 0
        {
            self.max_distrib_children = (self.upload_speed
                / self.distrib_parent_speed_ratio
                / 100)
                .min(MAX_DISTRIB_CHILDREN_LIMIT);
        } else {
            self.max_distrib_children = 0;
        }
        if self.max_distrib_children <= num_children && num_children < previous {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
        }
    }

    fn close_conn(&mut self, conn_id: ConnId) {
        if let Some(conn) = self.conns.get(&conn_id) {
            let _ = conn.control.send(ConnControl::Close);
        }
    }

    fn handle_conn_closed(&mut self, conn_id: ConnId, error: Option<String>) {
        let Some(conn) = self.conns.remove(&conn_id) else { return };
        let username = conn.username.clone().unwrap_or_default();

        if let Some(init_id) = conn.init_id
            && let Some(init) = self.inits.get_mut(&init_id)
            && init.conn_id == Some(conn_id)
        {
            if init.established {
                self.remove_init(init_id);
            } else {
                init.conn_id = None;
                if let Some(error) = &error {
                    debug!(username, %error, "direct connection attempt failed");
                }
            }
        }

        match conn.conn_type {
            Some(ConnectionType::File) => {
                self.emit(NetworkEvent::FileConnectionClosed {
                    username,
                    token: conn.file_token,
                    conn_id,
                });
            }
            Some(conn_type) => {
                if conn.established {
                    self.emit(NetworkEvent::PeerConnectionClosed {
                        username: username.clone(),
                        conn_type,
                        conn_id,
                    });
                }
                if conn_type == ConnectionType::Distributed {
                    if self.parent.as_deref() == Some(username.as_str()) {
                        self.parent = None;
                        self.send_have_no_parent();
                    }
                    if self.child_peers.get(&username) == Some(&conn_id) {
                        self.child_peers.remove(&username);
                        if self.child_peers.len() as u32 + 1 == self.max_distrib_children {
                            self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
                        }
                    }
                    if let Some(candidate) = self.potential_parents.get_mut(&username)
                        && candidate.conn_id == Some(conn_id)
                    {
                        candidate.conn_id = None;
                    }
                }
            }
            None => {}
        }
    }

    fn teardown(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
        if let Some(server) = self.server.take() {
            let _ = server.control.send(ConnControl::Close);
        }
        for conn in self.conns.values() {
            let _ = conn.control.send(ConnControl::Close);
        }
    }
}
