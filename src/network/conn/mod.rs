mod bandwidth;
mod file;
mod peer;
mod server;

use bandwidth::Bandwidth;
pub use peer::{run_incoming_peer, run_outgoing_peer};
pub use server::run_server_conn;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWriteExt, BufReader, BufWriter, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

use crate::network::ConnId;
use crate::protocol::{DistributedMessage, PeerInitMessage, PeerMessage, ServerResponse};
use crate::types::ConnectionType;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FRAME_QUEUE_CAPACITY: usize = 256;
const OUTGOING_QUEUE_CAPACITY: usize = 1024;

#[derive(Default)]
pub struct AllowedResponses {
    pub search_tokens: HashSet<u32>,
    pub shared_list_users: HashSet<String>,
    pub user_info_users: HashSet<String>,
    pub folder_contents: HashSet<(String, String)>,
}

pub type SharedAllowed = Arc<RwLock<AllowedResponses>>;

#[derive(Default)]
pub struct TransferLimits {
    pub upload: Bandwidth,
    pub download: Bandwidth,
}

pub type SharedLimits = Arc<TransferLimits>;

pub struct Traffic {
    epoch: Instant,
    last_active_ms: AtomicU64,
    received: AtomicU64,
    sends_written: AtomicU64,
}

impl Default for Traffic {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            last_active_ms: AtomicU64::new(0),
            received: AtomicU64::new(0),
            sends_written: AtomicU64::new(0),
        }
    }
}

impl Traffic {
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Acquire)
    }

    pub fn sends_written(&self) -> u64 {
        self.sends_written.load(Ordering::Acquire)
    }

    fn touch(&self) {
        let elapsed = self.epoch.elapsed().as_millis() as u64;
        self.last_active_ms.fetch_max(elapsed, Ordering::Relaxed);
    }

    fn last_active(&self) -> Instant {
        self.epoch + Duration::from_millis(self.last_active_ms.load(Ordering::Relaxed))
    }

    fn record_received(&self, count: usize) {
        self.received.fetch_add(count as u64, Ordering::Release);
        self.touch();
    }

    fn record_send_complete(&self) {
        self.sends_written.fetch_add(1, Ordering::Release);
    }
}

pub type SharedTraffic = Arc<Traffic>;

pub struct TrackedRead {
    inner: OwnedReadHalf,
    traffic: SharedTraffic,
}

impl AsyncRead for TrackedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let count = buf.filled().len() - before;
        if count > 0 {
            self.traffic.record_received(count);
        }
        polled
    }
}

type SocketReader = BufReader<TrackedRead>;

fn consumed(reader: &SocketReader) -> u64 {
    reader.get_ref().traffic.received() - reader.buffer().len() as u64
}

fn split_tracked(
    stream: TcpStream,
    traffic: SharedTraffic,
) -> (SocketReader, BufWriter<OwnedWriteHalf>) {
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(TrackedRead {
        inner: read_half,
        traffic,
    });
    (reader, BufWriter::new(write_half))
}

#[derive(Debug)]
pub enum ConnControl {
    Send(Vec<u8>),
    SendPeer(PeerMessage),
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
        received_through: u64,
    },
    Unsent {
        username: String,
        messages: Vec<PeerMessage>,
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
    pub traffic: SharedTraffic,
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            stream
                .set_nodelay(true)
                .map_err(|error| format!("cannot set nodelay: {error}"))?;
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
