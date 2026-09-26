use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use tokio::sync::mpsc;
use tracing::warn;

use super::indirect::InitId;
use super::{Actor, CONN_CONTROL_QUEUE_CAPACITY};
use crate::network::ConnId;
use crate::network::NetworkEvent;
use crate::network::conn::{
    ConnControl, PeerTask, SharedTraffic, run_incoming_peer, run_outgoing_peer,
};
use crate::protocol::{PeerInitMessage, PeerMessage, ServerRequest};
use crate::types::ConnectionType;

#[derive(Clone)]
pub(super) struct PeerIdentity {
    pub(super) username: String,
    pub(super) conn_type: ConnectionType,
}

pub(super) struct Conn {
    pub(super) control: mpsc::Sender<ConnControl>,
    pub(super) identity: Option<PeerIdentity>,
    pub(super) init_id: Option<InitId>,
    pub(super) established: bool,
    pub(super) file_token: Option<u32>,
    pub(super) pierce_token: Option<u32>,
    pub(super) ip: Option<Ipv4Addr>,
    traffic: SharedTraffic,
    sends_pushed: u64,
}

impl Conn {
    #[cfg(test)]
    pub(super) fn established(control: mpsc::Sender<ConnControl>, identity: PeerIdentity) -> Self {
        Self {
            control,
            identity: Some(identity),
            init_id: None,
            established: true,
            file_token: None,
            pierce_token: None,
            ip: None,
            traffic: SharedTraffic::default(),
            sends_pushed: 0,
        }
    }

    fn is_quiescent(&self, received_through: u64) -> bool {
        self.traffic.received() == received_through
            && self.traffic.sends_written() == self.sends_pushed
    }
}

pub(super) struct Peers {
    conns: HashMap<ConnId, Conn>,
    next_conn_id: ConnId,
}

impl Peers {
    pub(super) fn new() -> Self {
        Self {
            conns: HashMap::new(),
            next_conn_id: 1,
        }
    }

    pub(super) fn add(&mut self, conn: Conn) -> ConnId {
        let conn_id = self.next_conn_id;
        self.next_conn_id += 1;
        self.conns.insert(conn_id, conn);
        conn_id
    }

    pub(super) fn get(&self, conn_id: ConnId) -> Option<&Conn> {
        self.conns.get(&conn_id)
    }

    pub(super) fn get_mut(&mut self, conn_id: ConnId) -> Option<&mut Conn> {
        self.conns.get_mut(&conn_id)
    }

    pub(super) fn remove(&mut self, conn_id: ConnId) -> Option<Conn> {
        self.conns.remove(&conn_id)
    }

    pub(super) fn contains(&self, conn_id: ConnId) -> bool {
        self.conns.contains_key(&conn_id)
    }

    pub(super) fn count(&self) -> usize {
        self.conns.len()
    }

    pub(super) fn close_all(&mut self) {
        for conn in self.conns.values() {
            let _ = conn.control.try_send(ConnControl::Close);
        }
        self.conns.clear();
    }
}

impl Actor {
    pub(super) fn push_conn(&mut self, conn_id: ConnId, control: ConnControl) {
        let Some(conn) = self.peers.get_mut(conn_id) else {
            return;
        };
        let is_send = matches!(control, ConnControl::Send(_) | ConnControl::SendPeer(_));
        let (rejected, overflowed) = match conn.control.try_send(control) {
            Ok(()) => {
                if is_send {
                    conn.sends_pushed += 1;
                }
                return;
            }
            Err(mpsc::error::TrySendError::Full(rejected)) => (rejected, true),
            Err(mpsc::error::TrySendError::Closed(rejected)) => (rejected, false),
        };
        if let ConnControl::SendPeer(message) = rejected {
            let username = conn
                .identity
                .as_ref()
                .expect("peer message pushed to an unidentified connection")
                .username
                .clone();
            self.emit_unsent(username, vec![message]);
        }
        if overflowed {
            warn!(conn_id, "connection outbound queue overflowed, dropping");
            self.handle_conn_closed(conn_id, Some("outbound queue overflowed".into()));
        } else {
            self.detach_conn_routing(conn_id);
        }
    }

    fn detach_conn_routing(&mut self, conn_id: ConnId) {
        if let Some(init_id) = self
            .peers
            .get_mut(conn_id)
            .and_then(|conn| conn.init_id.take())
        {
            self.detach_init_conn(init_id, conn_id, None);
        }
    }

    pub(super) fn emit_unsent(&self, username: String, unsent: Vec<PeerMessage>) {
        self.emit(NetworkEvent::PeerConnectionError {
            username,
            unsent,
            is_offline: false,
        });
    }

    pub(super) fn close_conn(&mut self, conn_id: ConnId) {
        self.detach_conn_routing(conn_id);
        self.push_conn(conn_id, ConnControl::Close);
    }

    pub(super) fn discard_conn(&mut self, conn_id: ConnId) {
        self.close_conn(conn_id);
        self.handle_conn_closed(conn_id, None);
    }

    pub(super) fn handle_accepted(&mut self, stream: tokio::net::TcpStream, addr: SocketAddr) {
        let (control_tx, control_rx) = mpsc::channel(CONN_CONTROL_QUEUE_CAPACITY);
        let ip = match addr {
            SocketAddr::V4(addr) => Some(*addr.ip()),
            SocketAddr::V6(_) => None,
        };
        let traffic = SharedTraffic::default();
        let conn_id = self.peers.add(Conn {
            control: control_tx,
            identity: None,
            init_id: None,
            established: true,
            file_token: None,
            pierce_token: None,
            ip,
            traffic: traffic.clone(),
            sends_pushed: 0,
        });
        let task = PeerTask {
            conn_id,
            events: self.conn_events_tx.clone(),
            control: control_rx,
            allowed: self.allowed.clone(),
            limits: self.limits.clone(),
            traffic,
        };
        super::spawn_conn_task("incoming peer", run_incoming_peer(task, stream, addr));
        self.emit(NetworkEvent::ConnectionCount(self.peers.count()));
    }

    pub(super) fn spawn_outgoing_peer(
        &mut self,
        addr: SocketAddrV4,
        username: String,
        first_message: PeerInitMessage,
        conn_type: ConnectionType,
        init_id: Option<InitId>,
    ) -> ConnId {
        let pierce_token = match first_message {
            PeerInitMessage::PierceFireWall { token } => Some(token),
            _ => None,
        };
        let (control_tx, control_rx) = mpsc::channel(CONN_CONTROL_QUEUE_CAPACITY);
        let traffic = SharedTraffic::default();
        let conn_id = self.peers.add(Conn {
            control: control_tx,
            identity: Some(PeerIdentity {
                username: username.clone(),
                conn_type,
            }),
            init_id,
            established: false,
            file_token: None,
            pierce_token,
            ip: Some(*addr.ip()),
            traffic: traffic.clone(),
            sends_pushed: 0,
        });
        let task = PeerTask {
            conn_id,
            events: self.conn_events_tx.clone(),
            control: control_rx,
            allowed: self.allowed.clone(),
            limits: self.limits.clone(),
            traffic,
        };
        super::spawn_conn_task(
            "outgoing peer",
            run_outgoing_peer(
                task,
                SocketAddr::V4(addr),
                username,
                first_message,
                conn_type,
            ),
        );
        self.emit(NetworkEvent::ConnectionCount(self.peers.count()));
        conn_id
    }

    pub(super) fn handle_established(&mut self, conn_id: ConnId) {
        let Some(conn) = self.peers.get_mut(conn_id) else {
            return;
        };
        conn.established = true;
        let PeerIdentity {
            username,
            conn_type,
        } = conn
            .identity
            .clone()
            .expect("outgoing connection established without identity");
        let ip = conn.ip;
        let init_id = conn.init_id;
        let pierced = conn.pierce_token.is_some();
        if let Some(init_id) = init_id
            && !self.indirect.mark_established(init_id, conn_id)
        {
            return;
        }
        self.emit(NetworkEvent::PeerConnected {
            username: username.clone(),
            conn_type,
            conn_id,
            ip,
        });
        if let Some(init_id) = init_id {
            self.flush_init_queue(init_id);
        }
        if pierced && conn_type == ConnectionType::Distributed {
            self.accept_child_peer(conn_id, &username);
        }
    }

    pub(super) fn handle_peer_message(
        &mut self,
        conn_id: ConnId,
        message: PeerMessage,
        received_through: u64,
    ) {
        let Some(conn) = self.peers.get(conn_id) else {
            return;
        };
        let Some(identity) = &conn.identity else {
            warn!(conn_id, "peer message on unidentified connection, closing");
            self.close_conn(conn_id);
            return;
        };
        let username = identity.username.clone();
        let close_after = matches!(message, PeerMessage::FileSearchResponse { .. })
            && self.server.username() != Some(username.as_str())
            && conn.is_quiescent(received_through);
        self.emit(NetworkEvent::PeerMessage { username, message });
        if close_after {
            self.close_conn(conn_id);
        }
    }

    pub(super) fn handle_conn_closed(&mut self, conn_id: ConnId, error: Option<String>) {
        let Some(conn) = self.peers.remove(conn_id) else {
            return;
        };
        self.emit(NetworkEvent::ConnectionCount(self.peers.count()));

        if !conn.established
            && let (Some(token), Some(identity)) = (conn.pierce_token, &conn.identity)
        {
            self.send_to_server(ServerRequest::CantConnectToPeer {
                token,
                user: identity.username.clone(),
            });
        }

        if let Some(init_id) = conn.init_id {
            self.detach_init_conn(init_id, conn_id, error.as_deref());
        }

        let Some(PeerIdentity {
            username,
            conn_type,
        }) = conn.identity
        else {
            return;
        };
        match conn_type {
            ConnectionType::File => {
                self.emit(NetworkEvent::FileConnectionClosed {
                    username,
                    token: conn.file_token,
                    conn_id,
                });
            }
            ConnectionType::Distributed => {
                self.handle_distributed_conn_closed(&username, conn_id);
            }
            ConnectionType::Peer => {}
        }
    }
}
