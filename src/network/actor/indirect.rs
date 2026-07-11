use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::time::Instant;
use tracing::debug;

use super::Actor;
use super::peers::PeerIdentity;
use crate::network::conn::ConnControl;
use crate::protocol::{
    PeerInitMessage, PeerMessage, ServerRequest, increment_token, initial_token,
};
use crate::types::{ConnId, ConnectionType, NetworkEvent};

const INDIRECT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const USER_ADDRESS_TTL: Duration = Duration::from_secs(1800);

pub(super) type InitId = u64;

#[derive(Debug)]
pub(super) enum QueuedItem {
    Peer(PeerMessage),
    FileInit(u32),
}

pub(super) struct Init {
    pub(super) username: String,
    pub(super) conn_type: ConnectionType,
    indirect_token: u32,
    pub(super) queued: Vec<QueuedItem>,
    pub(super) conn_id: Option<ConnId>,
    pub(super) established: bool,
    created: Instant,
}

struct UserAddress {
    addr: Option<SocketAddrV4>,
    updated: Instant,
}

pub(super) struct Indirect {
    inits: HashMap<InitId, Init>,
    inits_by_user: HashMap<(String, ConnectionType), InitId>,
    inits_by_token: HashMap<u32, InitId>,
    inits_pending_address: HashMap<String, Vec<InitId>>,
    user_addresses: HashMap<String, Option<UserAddress>>,
    indirect_token: u32,
    next_init_id: InitId,
}

impl Indirect {
    pub(super) fn new() -> Self {
        Self {
            inits: HashMap::new(),
            inits_by_user: HashMap::new(),
            inits_by_token: HashMap::new(),
            inits_pending_address: HashMap::new(),
            user_addresses: HashMap::new(),
            indirect_token: initial_token(),
            next_init_id: 1,
        }
    }

    pub(super) fn watch_user(&mut self, username: &str) {
        self.user_addresses
            .entry(username.to_owned())
            .or_insert(None);
    }

    pub(super) fn unwatch_user(&mut self, username: &str) {
        self.user_addresses.remove(username);
    }

    pub(super) fn forget_address(&mut self, username: &str) {
        if let Some(entry) = self.user_addresses.get_mut(username) {
            *entry = None;
        }
    }

    pub(super) fn cache_address(&mut self, username: String, addr: SocketAddrV4) {
        self.user_addresses.insert(
            username,
            Some(UserAddress {
                addr: Some(addr),
                updated: Instant::now(),
            }),
        );
    }

    fn record_address(&mut self, username: &str, addr: Option<SocketAddrV4>) {
        if let Some(entry) = self.user_addresses.get_mut(username) {
            *entry = addr.map(|addr| UserAddress {
                addr: Some(addr),
                updated: Instant::now(),
            });
        }
    }

    fn next_indirect_token(&mut self) -> u32 {
        self.indirect_token = increment_token(self.indirect_token);
        self.indirect_token
    }

    fn next_init_id(&mut self) -> InitId {
        let init_id = self.next_init_id;
        self.next_init_id += 1;
        init_id
    }

    fn lookup_fresh_address(
        &self,
        username: &str,
        own_username: Option<&str>,
    ) -> Option<SocketAddrV4> {
        let entry = self.user_addresses.get(username)?.as_ref()?;
        let addr = entry.addr?;
        if addr.port() == 0 {
            return None;
        }
        if Some(username) != own_username && entry.updated.elapsed() > USER_ADDRESS_TTL {
            return None;
        }
        Some(addr)
    }

    pub(super) fn get(&self, init_id: InitId) -> Option<&Init> {
        self.inits.get(&init_id)
    }

    fn contains(&self, init_id: InitId) -> bool {
        self.inits.contains_key(&init_id)
    }

    pub(super) fn existing_attempt(
        &self,
        username: &str,
        conn_type: ConnectionType,
    ) -> Option<InitId> {
        if conn_type == ConnectionType::File {
            return None;
        }
        self.inits_by_user
            .get(&(username.to_owned(), conn_type))
            .copied()
    }

    pub(super) fn attempt_by_token(&self, token: u32) -> Option<InitId> {
        self.inits_by_token.get(&token).copied()
    }

    fn push_queued(&mut self, init_id: InitId, item: QueuedItem) -> bool {
        let init = self.inits.get_mut(&init_id).unwrap();
        init.queued.push(item);
        init.established
    }

    fn register(
        &mut self,
        username: String,
        conn_type: ConnectionType,
        item: Option<QueuedItem>,
    ) -> (InitId, u32) {
        let indirect_token = self.next_indirect_token();
        let init_id = self.next_init_id();
        self.inits.insert(
            init_id,
            Init {
                username: username.clone(),
                conn_type,
                indirect_token,
                queued: item.into_iter().collect(),
                conn_id: None,
                established: false,
                created: Instant::now(),
            },
        );
        self.inits_by_token.insert(indirect_token, init_id);
        if conn_type != ConnectionType::File {
            self.inits_by_user.insert((username, conn_type), init_id);
        }
        (init_id, indirect_token)
    }

    fn pend_address(&mut self, username: String, init_id: InitId) {
        self.inits_pending_address
            .entry(username)
            .or_default()
            .push(init_id);
    }

    fn take_pending_address(&mut self, username: &str) -> Vec<InitId> {
        self.inits_pending_address
            .remove(username)
            .unwrap_or_default()
    }

    fn set_conn(&mut self, init_id: InitId, conn_id: ConnId) {
        if let Some(init) = self.inits.get_mut(&init_id) {
            init.conn_id = Some(conn_id);
        }
    }

    pub(super) fn mark_established(&mut self, init_id: InitId, conn_id: ConnId) -> bool {
        match self.inits.get_mut(&init_id) {
            Some(init) => {
                init.established = true;
                init.conn_id = Some(conn_id);
                true
            }
            None => false,
        }
    }

    fn replace_conn(&mut self, init_id: InitId, conn_id: ConnId) -> Option<ConnId> {
        self.inits.get_mut(&init_id).unwrap().conn_id.replace(conn_id)
    }

    fn drain_queue(&mut self, init_id: InitId) -> Option<(ConnId, String, Vec<QueuedItem>)> {
        let init = self.inits.get_mut(&init_id)?;
        let conn_id = init.conn_id?;
        Some((
            conn_id,
            init.username.clone(),
            std::mem::take(&mut init.queued),
        ))
    }

    fn adopt_incoming(
        &mut self,
        username: String,
        conn_type: ConnectionType,
        conn_id: ConnId,
    ) -> InitId {
        let key = (username.clone(), conn_type);
        let mut inherited = Vec::new();
        if let Some(&prev_id) = self.inits_by_user.get(&key)
            && let Some(prev) = self.inits.get_mut(&prev_id)
        {
            inherited = std::mem::take(&mut prev.queued);
            self.remove_init(prev_id);
        }
        let init_id = self.next_init_id();
        self.inits.insert(
            init_id,
            Init {
                username,
                conn_type,
                indirect_token: 0,
                queued: inherited,
                conn_id: Some(conn_id),
                established: true,
                created: Instant::now(),
            },
        );
        self.inits_by_user.insert(key, init_id);
        init_id
    }

    fn claim_pierce(&mut self, token: u32) -> Option<InitId> {
        self.inits_by_token.remove(&token)
    }

    fn detach_conn(&mut self, init_id: InitId, conn_id: ConnId) -> Option<String> {
        let init = self.inits.get_mut(&init_id)?;
        if init.conn_id != Some(conn_id) {
            return None;
        }
        if init.established {
            self.remove_init(init_id);
            None
        } else {
            init.conn_id = None;
            Some(init.username.clone())
        }
    }

    fn expired(&self) -> Vec<InitId> {
        self.inits
            .iter()
            .filter(|(_, init)| {
                !init.established && init.created.elapsed() > INDIRECT_REQUEST_TIMEOUT
            })
            .map(|(&id, _)| id)
            .collect()
    }

    pub(super) fn remove_init(&mut self, init_id: InitId) -> Option<Init> {
        let init = self.inits.remove(&init_id)?;
        self.inits_by_token.remove(&init.indirect_token);
        if let Some(existing) = self
            .inits_by_user
            .get(&(init.username.clone(), init.conn_type))
            && *existing == init_id
        {
            self.inits_by_user
                .remove(&(init.username.clone(), init.conn_type));
        }
        if let Some(pending) = self.inits_pending_address.get_mut(&init.username) {
            pending.retain(|&id| id != init_id);
            if pending.is_empty() {
                self.inits_pending_address.remove(&init.username);
            }
        }
        Some(init)
    }

    pub(super) fn reset(&mut self) {
        self.inits.clear();
        self.inits_by_user.clear();
        self.inits_by_token.clear();
        self.inits_pending_address.clear();
        self.user_addresses.clear();
    }
}

impl Actor {
    pub(super) fn send_to_peer(&mut self, username: String, item: QueuedItem) {
        let conn_type = match &item {
            QueuedItem::Peer(_) => ConnectionType::Peer,
            QueuedItem::FileInit(_) => ConnectionType::File,
        };
        if let Some(init_id) = self.indirect.existing_attempt(&username, conn_type) {
            if self.indirect.push_queued(init_id, item) {
                self.flush_init_queue(init_id);
            }
            return;
        }
        self.initiate_peer_connection(username, conn_type, Some(item), None);
    }

    pub(super) fn initiate_peer_connection(
        &mut self,
        username: String,
        conn_type: ConnectionType,
        item: Option<QueuedItem>,
        in_address: Option<SocketAddrV4>,
    ) {
        let addr = in_address.or_else(|| {
            self.indirect
                .lookup_fresh_address(&username, self.server.username())
        });
        let (init_id, indirect_token) = self.indirect.register(username.clone(), conn_type, item);
        self.send_to_server(ServerRequest::ConnectToPeer {
            token: indirect_token,
            user: username.clone(),
            conn_type,
        });

        match addr {
            Some(addr) => self.connect_direct(init_id, addr),
            None => {
                self.indirect.pend_address(username.clone(), init_id);
                self.send_to_server(ServerRequest::GetPeerAddress { user: username });
            }
        }
    }

    pub(super) fn connect_direct(&mut self, init_id: InitId, addr: SocketAddrV4) {
        let Some(server_username) = self.server.username().map(str::to_owned) else {
            return;
        };
        let init = self.indirect.get(init_id).unwrap();
        if addr.port() == 0 {
            debug!(
                username = init.username,
                "skipping direct connection, invalid port"
            );
            return;
        }
        let username = init.username.clone();
        let conn_type = init.conn_type;
        let first_message = PeerInitMessage::PeerInit {
            username: server_username,
            conn_type,
        };
        let conn_id =
            self.spawn_outgoing_peer(addr, username, first_message, conn_type, Some(init_id));
        self.indirect.set_conn(init_id, conn_id);
    }

    pub(super) fn flush_init_queue(&mut self, init_id: InitId) {
        let Some((conn_id, username, queued)) = self.indirect.drain_queue(init_id) else {
            return;
        };
        let Some(conn) = self.peers.get_mut(conn_id) else {
            return;
        };
        let mut sent_file_init = None;
        let mut controls = Vec::with_capacity(queued.len());
        for item in queued {
            controls.push(match item {
                QueuedItem::Peer(message) => ConnControl::Send(message.to_bytes()),
                QueuedItem::FileInit(token) => {
                    conn.file_token = Some(token);
                    sent_file_init = Some(token);
                    ConnControl::SendFileInit(token)
                }
            });
        }
        for control in controls {
            self.push_conn(conn_id, control);
        }
        if let Some(token) = sent_file_init
            && self.peers.contains(conn_id)
        {
            self.emit(NetworkEvent::FileTransferInit {
                username,
                token,
                conn_id,
            });
        }
    }

    pub(super) fn fail_init(&mut self, init_id: InitId, is_offline: bool) {
        let Some(init) = self.indirect.remove_init(init_id) else {
            return;
        };
        if init.established {
            return;
        }
        if let Some(conn_id) = init.conn_id {
            self.close_conn(conn_id);
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

    pub(super) fn detach_init_conn(
        &mut self,
        init_id: InitId,
        conn_id: ConnId,
        error: Option<&str>,
    ) {
        if let Some(username) = self.indirect.detach_conn(init_id, conn_id)
            && let Some(error) = error
        {
            debug!(username, error, "direct connection attempt failed");
        }
    }

    pub(super) fn sweep_indirect_requests(&mut self) {
        for init_id in self.indirect.expired() {
            self.fail_init(init_id, false);
        }
    }

    pub(super) fn handle_connect_to_peer(
        &mut self,
        username: String,
        conn_type: ConnectionType,
        addr: SocketAddrV4,
        pierce_token: u32,
    ) {
        if self.indirect.existing_attempt(&username, conn_type).is_some() {
            debug!(username, "existing connection, ignoring indirect request");
        } else {
            self.spawn_outgoing_peer(
                addr,
                username,
                PeerInitMessage::PierceFireWall {
                    token: pierce_token,
                },
                conn_type,
                None,
            );
        }
    }

    pub(super) fn handle_cant_connect(&mut self, token: u32) {
        if let Some(init_id) = self.indirect.attempt_by_token(token) {
            self.fail_init(init_id, false);
        }
    }

    pub(super) fn handle_peer_address(&mut self, user: &str, ip_address: Ipv4Addr, port: u16) {
        let user_offline = ip_address == Ipv4Addr::UNSPECIFIED;
        let addr = SocketAddrV4::new(ip_address, port);
        for init_id in self.indirect.take_pending_address(user) {
            if user_offline {
                self.fail_init(init_id, true);
            } else if self.indirect.contains(init_id) {
                self.connect_direct(init_id, addr);
            }
        }
        if self.server.username() != Some(user) {
            self.indirect
                .record_address(user, (!user_offline && port != 0).then_some(addr));
        }
    }

    pub(super) fn handle_incoming_init(
        &mut self,
        conn_id: ConnId,
        init: PeerInitMessage,
        addr: SocketAddr,
    ) {
        match init {
            PeerInitMessage::PeerInit {
                username,
                conn_type,
            } => {
                let Some(conn) = self.peers.get_mut(conn_id) else {
                    return;
                };
                conn.identity = Some(PeerIdentity {
                    username: username.clone(),
                    conn_type,
                });
                debug!(username, ?conn_type, %addr, "incoming direct connection");

                if conn_type != ConnectionType::File {
                    let init_id =
                        self.indirect
                            .adopt_incoming(username.clone(), conn_type, conn_id);
                    self.peers.get_mut(conn_id).unwrap().init_id = Some(init_id);
                    self.flush_init_queue(init_id);
                }
                self.emit(NetworkEvent::PeerConnected {
                    username: username.clone(),
                    conn_type,
                    conn_id,
                    ip: self.peers.get(conn_id).and_then(|conn| conn.ip),
                });
                if conn_type == ConnectionType::Distributed {
                    self.accept_child_peer(conn_id, &username);
                }
            }
            PeerInitMessage::PierceFireWall { token } => {
                let Some(init_id) = self.indirect.claim_pierce(token) else {
                    debug!(token, "indirect connection attempt expired, closing");
                    self.close_conn(conn_id);
                    return;
                };
                let init = self.indirect.get(init_id).unwrap();
                let username = init.username.clone();
                let conn_type = init.conn_type;
                let previous = init.conn_id;
                debug!(username, token, "indirect connection established");

                if let Some(previous) = previous
                    && self.peers.is_established(previous)
                {
                    debug!(
                        username,
                        "direct connection already established, keeping it"
                    );
                    if let Some(conn) = self.peers.get_mut(conn_id) {
                        conn.identity = Some(PeerIdentity {
                            username: username.clone(),
                            conn_type,
                        });
                    }
                    self.push_conn(
                        conn_id,
                        ConnControl::AssumeIdentity {
                            username,
                            conn_type,
                        },
                    );
                    return;
                }

                if let Some(previous) = self.indirect.replace_conn(init_id, conn_id) {
                    if let Some(conn) = self.peers.get_mut(previous) {
                        conn.init_id = None;
                    }
                    self.close_conn(previous);
                }
                self.indirect.mark_established(init_id, conn_id);
                let conn = self.peers.get_mut(conn_id).unwrap();
                conn.identity = Some(PeerIdentity {
                    username: username.clone(),
                    conn_type,
                });
                conn.init_id = Some(init_id);
                self.push_conn(
                    conn_id,
                    ConnControl::AssumeIdentity {
                        username: username.clone(),
                        conn_type,
                    },
                );
                if !self.peers.contains(conn_id) {
                    return;
                }
                self.emit(NetworkEvent::PeerConnected {
                    username,
                    conn_type,
                    conn_id,
                    ip: self.peers.get(conn_id).and_then(|conn| conn.ip),
                });
                self.flush_init_queue(init_id);
            }
        }
    }
}
