use std::net::Ipv4Addr;

use crate::protocol::{PeerMessage, ServerResponse};
use crate::types::ConnectionType;

pub type ConnId = u64;

#[derive(Debug)]
pub enum NetworkEvent {
    ServerConnected,
    LoggedIn {
        username: String,
        banner: String,
        ip_address: Ipv4Addr,
        is_supporter: bool,
    },
    LoginRejected {
        reason: String,
        detail: Option<String>,
    },
    ServerDisconnected {
        manual: bool,
    },
    ServerMessage(ServerResponse),
    PeerMessage {
        username: String,
        conn_id: ConnId,
        message: PeerMessage,
    },
    PeerConnected {
        username: String,
        conn_type: ConnectionType,
        conn_id: ConnId,
    },
    PeerConnectionClosed {
        username: String,
        conn_type: ConnectionType,
        conn_id: ConnId,
    },
    PeerConnectionError {
        username: String,
        conn_type: ConnectionType,
        unsent: Vec<PeerMessage>,
        is_offline: bool,
    },
    DistributedSearch {
        username: String,
        token: u32,
        search_term: String,
    },
    FileTransferInit {
        username: String,
        token: u32,
        conn_id: ConnId,
    },
    FileDownloadProgress {
        username: String,
        token: u32,
        bytes_left: u64,
        speed: u64,
    },
    FileUploadProgress {
        username: String,
        token: u32,
        offset: u64,
        bytes_sent: u64,
        speed: u64,
    },
    FileTransferError {
        username: String,
        token: u32,
        error: String,
    },
    FileConnectionClosed {
        username: String,
        token: Option<u32>,
        conn_id: ConnId,
    },
}
