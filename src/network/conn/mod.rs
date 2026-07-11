mod file;
mod peer;
mod server;

pub use peer::{run_incoming_peer, run_outgoing_peer};
pub use server::run_server_conn;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::protocol::{DistributedMessage, PeerInitMessage, PeerMessage, ServerResponse};
use crate::types::{ConnId, ConnectionType};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_QUEUE_CAPACITY: usize = 256;

#[derive(Default)]
pub struct AllowedResponses {
    pub search_tokens: HashSet<u32>,
    pub shared_list_users: HashSet<String>,
    pub user_info_users: HashSet<String>,
}

pub type SharedAllowed = Arc<RwLock<AllowedResponses>>;

#[derive(Default)]
pub struct TransferLimits {
    pub upload_bps: AtomicU64,
    pub download_bps: AtomicU64,
}

pub type SharedLimits = Arc<TransferLimits>;

#[derive(Debug)]
pub enum ConnControl {
    Send(Vec<u8>),
    SendFileInit(u32),
    AssumeIdentity {
        username: String,
        conn_type: ConnectionType,
    },
    Download {
        file: std::fs::File,
        offset: u64,
        bytes_left: u64,
    },
    Upload {
        file: std::fs::File,
        size: u64,
    },
    Close,
}

#[derive(Debug)]
pub enum ConnEvent {
    ServerMessage(ServerResponse),
    ServerClosed {
        error: Option<String>,
    },
    OutgoingEstablished {
        conn_id: ConnId,
    },
    IncomingInit {
        conn_id: ConnId,
        init: PeerInitMessage,
        addr: SocketAddr,
    },
    Peer {
        conn_id: ConnId,
        message: PeerMessage,
    },
    Distrib {
        conn_id: ConnId,
        message: DistributedMessage,
    },
    FileInit {
        conn_id: ConnId,
        token: u32,
    },
    FileOffsetReceived {
        conn_id: ConnId,
        offset: u64,
    },
    DownloadProgress {
        conn_id: ConnId,
        bytes_left: u64,
    },
    UploadProgress {
        conn_id: ConnId,
        offset: u64,
        bytes_sent: u64,
    },
    FileDone {
        conn_id: ConnId,
    },
    FileError {
        conn_id: ConnId,
        error: String,
    },
    Closed {
        conn_id: ConnId,
        error: Option<String>,
    },
}

pub struct PeerTask {
    pub conn_id: ConnId,
    pub events: mpsc::Sender<ConnEvent>,
    pub control: mpsc::Receiver<ConnControl>,
    pub allowed: SharedAllowed,
    pub limits: SharedLimits,
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            if let Err(error) = stream.set_nodelay(true) {
                tracing::warn!(%error, %addr, "cannot set nodelay, continuing without it");
            }
            Ok(stream)
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

async fn write_all(writer: &mut BufWriter<OwnedWriteHalf>, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes).await?;
    writer.flush().await
}
