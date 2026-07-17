use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr};

use crate::protocol::{PeerMessage, ServerRequest, ServerResponse};
use crate::types::ConnectionType;

pub type ConnId = u64;

#[derive(Debug)]
pub enum NetworkCommand {
    ServerConnect {
        address: SocketAddr,
        username: String,
        password: String,
        listen_port: u16,
    },
    ServerDisconnect,
    SendServerMessage(ServerRequest),
    SendPeerMessage {
        username: String,
        message: PeerMessage,
    },
    SendPeerFrame {
        username: String,
        bytes: Vec<u8>,
    },
    RequestFileConnection {
        username: String,
        token: u32,
    },
    DownloadFile {
        conn_id: ConnId,
        file: File,
        offset: u64,
        bytes_left: u64,
    },
    UploadFile {
        conn_id: ConnId,
        file: File,
        size: u64,
    },
    SetTransferLimits {
        upload_bps: u64,
        download_bps: u64,
    },
    AllowSearchToken(u32),
    DisallowSearchToken(u32),
    AllowSharedListUser(String),
    DisallowSharedListUser(String),
    AllowUserInfoUser(String),
    DisallowUserInfoUser(String),
    CloseConnection(ConnId),
    Quit,
}

#[derive(Debug)]
pub enum NetworkEvent {
    LoggedIn {
        username: String,
        banner: String,
    },
    LoginRejected {
        reason: String,
        detail: Option<String>,
    },
    ListenFailed {
        port: u16,
        error: String,
    },
    ServerDisconnected {
        manual: bool,
    },
    ServerMessage(ServerResponse),
    PeerMessage {
        username: String,
        message: PeerMessage,
    },
    PeerConnected {
        username: String,
        conn_type: ConnectionType,
        conn_id: ConnId,
        ip: Option<Ipv4Addr>,
    },
    ConnectionCount(usize),
    PeerConnectionError {
        username: String,
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
    },
    FileUploadProgress {
        username: String,
        token: u32,
        offset: u64,
        bytes_sent: u64,
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
