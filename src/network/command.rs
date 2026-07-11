use std::fs::File;
use std::net::SocketAddr;

use super::event::ConnId;
use crate::protocol::{PeerMessage, ServerRequest};

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
    AllowFolderContents {
        username: String,
        directory: String,
    },
    DisallowFolderContents {
        username: String,
        directory: String,
    },
    CloseConnection(ConnId),
    Quit,
}
