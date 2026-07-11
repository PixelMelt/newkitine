use std::collections::HashSet;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use tracing::debug;

use super::codec::{
    FrameError, MAX_MESSAGE_SIZE_LARGE, MAX_MESSAGE_SIZE_MEDIUM, MAX_MESSAGE_SIZE_SMALL,
    read_frame_u8,
};
use super::event::ConnId;
use crate::protocol::{
    DistributedMessage, FileOffset, FileTransferInit, PeerInitMessage, PeerMessage, ServerResponse,
};
use crate::types::ConnectionType;

const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const GHOST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub upload_bps: AtomicU64,
    pub download_bps: AtomicU64,
}

pub type SharedLimits = Arc<TransferLimits>;

struct Throttle {
    window_start: Instant,
    window_bytes: u64,
}

impl Throttle {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            window_bytes: 0,
        }
    }

    async fn pace(&mut self, count: u64, limit_bps: u64) {
        if limit_bps == 0 {
            self.window_start = Instant::now();
            self.window_bytes = 0;
            return;
        }
        self.window_bytes += count;
        let target = Duration::from_secs_f64(self.window_bytes as f64 / limit_bps as f64);
        let elapsed = self.window_start.elapsed();
        if target > elapsed {
            tokio::time::sleep(target - elapsed).await;
        }
        if self.window_bytes >= limit_bps {
            self.window_start = Instant::now();
            self.window_bytes = 0;
        }
    }
}

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
        bytes_received: u64,
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
    pub events: mpsc::UnboundedSender<ConnEvent>,
    pub control: mpsc::UnboundedReceiver<ConnControl>,
    pub allowed: SharedAllowed,
    pub limits: SharedLimits,
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            stream.set_nodelay(true).ok();
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

pub async fn run_server_conn(
    address: SocketAddr,
    events: mpsc::UnboundedSender<ConnEvent>,
    mut control: mpsc::UnboundedReceiver<ConnControl>,
) {
    let stream = match connect(address).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events.send(ConnEvent::ServerClosed { error: Some(error) });
            return;
        }
    };
    let (read_half, write_half) = stream.into_split();
    let mut writer = BufWriter::new(write_half);

    let (frames_tx, mut frames) = mpsc::unbounded_channel();
    let reader_task = tokio::spawn(read_server_frames(BufReader::new(read_half), frames_tx));

    let error = loop {
        tokio::select! {
            frame = frames.recv() => {
                match frame {
                    Some(Ok(message)) => {
                        if events.send(ConnEvent::ServerMessage(message)).is_err() {
                            break None;
                        }
                    }
                    Some(Err(error)) => break Some(error),
                    None => break Some("connection closed".into()),
                }
            }
            ctrl = control.recv() => {
                match ctrl {
                    Some(ConnControl::Send(bytes)) => {
                        if write_all(&mut writer, &bytes).await.is_err() {
                            break Some("write failed".into());
                        }
                    }
                    Some(ConnControl::Close) | None => break None,
                    Some(other) => unreachable!("invalid server control {other:?}"),
                }
            }
        }
    };
    reader_task.abort();
    let _ = events.send(ConnEvent::ServerClosed { error });
}

async fn read_server_frames(
    mut reader: BufReader<OwnedReadHalf>,
    frames: mpsc::UnboundedSender<Result<ServerResponse, String>>,
) {
    loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(error) => {
                let _ = frames.send(Err(error.to_string()));
                return;
            }
        };
        if !(4..=MAX_MESSAGE_SIZE_LARGE).contains(&size) {
            let _ = frames.send(Err(format!("invalid server message size {size}")));
            return;
        }
        let mut frame = vec![0u8; size];
        if let Err(error) = reader.read_exact(&mut frame).await {
            let _ = frames.send(Err(error.to_string()));
            return;
        }
        let code = u32::from_le_bytes(frame[..4].try_into().unwrap());
        match ServerResponse::parse(code, &frame[4..]) {
            Ok(message) => {
                if frames.send(Ok(message)).is_err() {
                    return;
                }
            }
            Err(error) => debug!(code, %error, "dropping unparsable server message"),
        }
    }
}

pub async fn run_outgoing_peer(
    task: PeerTask,
    addr: SocketAddr,
    username: String,
    init: PeerInitMessage,
    conn_type: ConnectionType,
) {
    let stream = match connect(addr).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = task.events.send(ConnEvent::Closed {
                conn_id: task.conn_id,
                error: Some(error),
            });
            return;
        }
    };
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    if write_all(&mut writer, &init.to_bytes()).await.is_err() {
        let _ = task.events.send(ConnEvent::Closed {
            conn_id: task.conn_id,
            error: Some("write failed".into()),
        });
        return;
    }
    if task
        .events
        .send(ConnEvent::OutgoingEstablished {
            conn_id: task.conn_id,
        })
        .is_err()
    {
        return;
    }
    run_typed_loop(task, reader, writer, username, conn_type).await;
}

pub async fn run_incoming_peer(mut task: PeerTask, stream: TcpStream, addr: SocketAddr) {
    stream.set_nodelay(true).ok();

    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let writer = BufWriter::new(write_half);

    let init = match timeout(
        GHOST_TIMEOUT,
        read_frame_u8(&mut reader, MAX_MESSAGE_SIZE_SMALL),
    )
    .await
    {
        Ok(Ok((code, payload))) => match PeerInitMessage::parse(code, &payload) {
            Ok(init) => init,
            Err(error) => {
                let _ = task.events.send(ConnEvent::Closed {
                    conn_id: task.conn_id,
                    error: Some(error.to_string()),
                });
                return;
            }
        },
        Ok(Err(error)) => {
            let _ = task.events.send(ConnEvent::Closed {
                conn_id: task.conn_id,
                error: Some(error.to_string()),
            });
            return;
        }
        Err(_) => {
            let _ = task.events.send(ConnEvent::Closed {
                conn_id: task.conn_id,
                error: Some("ghost connection".into()),
            });
            return;
        }
    };

    let identity = match &init {
        PeerInitMessage::PeerInit {
            username,
            conn_type,
        } => Some((username.clone(), *conn_type)),
        PeerInitMessage::PierceFireWall { .. } => None,
    };
    if task
        .events
        .send(ConnEvent::IncomingInit {
            conn_id: task.conn_id,
            init,
            addr,
        })
        .is_err()
    {
        return;
    }

    let (username, conn_type) = match identity {
        Some(identity) => identity,
        None => match task.control.recv().await {
            Some(ConnControl::AssumeIdentity {
                username,
                conn_type,
            }) => (username, conn_type),
            Some(ConnControl::Close) | None => {
                let _ = task.events.send(ConnEvent::Closed {
                    conn_id: task.conn_id,
                    error: None,
                });
                return;
            }
            Some(other) => unreachable!("invalid pre-init control {other:?}"),
        },
    };

    run_typed_loop(task, reader, writer, username, conn_type).await;
}

async fn run_typed_loop(
    task: PeerTask,
    reader: BufReader<OwnedReadHalf>,
    writer: BufWriter<OwnedWriteHalf>,
    username: String,
    conn_type: ConnectionType,
) {
    let PeerTask {
        conn_id,
        events,
        control,
        allowed,
        limits,
    } = task;
    match conn_type {
        ConnectionType::Peer => {
            let (frames_tx, frames) = mpsc::unbounded_channel();
            let reader_task = tokio::spawn(read_peer_frames(reader, frames_tx, allowed, username));
            run_message_loop(conn_id, events, control, frames, writer, reader_task, true).await;
        }
        ConnectionType::Distributed => {
            let (frames_tx, frames) = mpsc::unbounded_channel();
            let reader_task = tokio::spawn(read_distrib_frames(reader, frames_tx));
            run_message_loop(conn_id, events, control, frames, writer, reader_task, false).await;
        }
        ConnectionType::File => {
            run_file_loop(conn_id, events, control, limits, reader, writer).await;
        }
    }
}

enum PeerFrame {
    Message(ConnEventPayload),
    Fatal(Option<String>),
}

enum ConnEventPayload {
    Peer(PeerMessage),
    Distrib(DistributedMessage),
}

async fn read_peer_frames(
    mut reader: BufReader<OwnedReadHalf>,
    frames: mpsc::UnboundedSender<PeerFrame>,
    allowed: SharedAllowed,
    username: String,
) {
    loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(error) => {
                let _ = frames.send(PeerFrame::Fatal(Some(error.to_string())));
                return;
            }
        };
        if size < 4 {
            let _ = frames.send(PeerFrame::Fatal(Some("truncated peer frame".into())));
            return;
        }
        let mut frame = vec![0u8; 4];
        if let Err(error) = reader.read_exact(&mut frame).await {
            let _ = frames.send(PeerFrame::Fatal(Some(error.to_string())));
            return;
        }
        let code = u32::from_le_bytes(frame[..4].try_into().unwrap());

        let is_large_response = matches!(code, 5 | 16);
        let limit = if is_large_response {
            MAX_MESSAGE_SIZE_LARGE
        } else {
            MAX_MESSAGE_SIZE_MEDIUM
        };
        if size - 4 > limit {
            let _ = frames.send(PeerFrame::Fatal(Some(format!(
                "peer message size {size} exceeds limit {limit}"
            ))));
            return;
        }
        if is_large_response && !is_large_response_allowed(&allowed, &username, code) {
            let _ = frames.send(PeerFrame::Fatal(None));
            return;
        }

        let mut payload = vec![0u8; size - 4];
        if let Err(error) = reader.read_exact(&mut payload).await {
            let _ = frames.send(PeerFrame::Fatal(Some(error.to_string())));
            return;
        }
        match PeerMessage::parse(code, &payload) {
            Ok(message) => {
                if !is_parsed_message_allowed(&allowed, &username, &message) {
                    debug!(code, username, "dropping disallowed peer response");
                    continue;
                }
                if frames
                    .send(PeerFrame::Message(ConnEventPayload::Peer(message)))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => debug!(code, %error, "dropping unparsable peer message"),
        }
    }
}

fn is_large_response_allowed(allowed: &SharedAllowed, username: &str, code: u32) -> bool {
    let allowed = allowed.read().unwrap();
    match code {
        5 => allowed.shared_list_users.contains(username),
        16 => allowed.user_info_users.contains(username),
        _ => true,
    }
}

fn is_parsed_message_allowed(
    allowed: &SharedAllowed,
    username: &str,
    message: &PeerMessage,
) -> bool {
    let allowed = allowed.read().unwrap();
    match message {
        PeerMessage::FileSearchResponse { token, .. } => allowed.search_tokens.contains(token),
        PeerMessage::FolderContentsResponse { directory, .. } => allowed
            .folder_contents
            .contains(&(username.to_owned(), directory.clone())),
        _ => true,
    }
}

async fn read_distrib_frames(
    mut reader: BufReader<OwnedReadHalf>,
    frames: mpsc::UnboundedSender<PeerFrame>,
) {
    loop {
        match read_frame_u8(&mut reader, MAX_MESSAGE_SIZE_SMALL).await {
            Ok((code, payload)) => match DistributedMessage::parse(code, &payload) {
                Ok(message) => {
                    if frames
                        .send(PeerFrame::Message(ConnEventPayload::Distrib(message)))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => debug!(code, %error, "dropping unparsable distributed message"),
            },
            Err(FrameError::Protocol(error)) => {
                debug!(%error, "dropping unparsable distributed message");
            }
            Err(error) => {
                let _ = frames.send(PeerFrame::Fatal(Some(error.to_string())));
                return;
            }
        }
    }
}

async fn run_message_loop(
    conn_id: ConnId,
    events: mpsc::UnboundedSender<ConnEvent>,
    mut control: mpsc::UnboundedReceiver<ConnControl>,
    mut frames: mpsc::UnboundedReceiver<PeerFrame>,
    mut writer: BufWriter<OwnedWriteHalf>,
    reader_task: JoinHandle<()>,
    idle_timeout: bool,
) {
    let mut deadline = Instant::now() + PEER_IDLE_TIMEOUT;
    let error = loop {
        tokio::select! {
            frame = frames.recv() => {
                deadline = Instant::now() + PEER_IDLE_TIMEOUT;
                match frame {
                    Some(PeerFrame::Message(payload)) => {
                        let event = match payload {
                            ConnEventPayload::Peer(message) => ConnEvent::Peer { conn_id, message },
                            ConnEventPayload::Distrib(message) => ConnEvent::Distrib { conn_id, message },
                        };
                        if events.send(event).is_err() {
                            break None;
                        }
                    }
                    Some(PeerFrame::Fatal(error)) => break error,
                    None => break Some("connection closed".into()),
                }
            }
            ctrl = control.recv() => {
                match ctrl {
                    Some(ConnControl::Send(bytes)) => {
                        deadline = Instant::now() + PEER_IDLE_TIMEOUT;
                        if write_all(&mut writer, &bytes).await.is_err() {
                            break Some("write failed".into());
                        }
                    }
                    Some(ConnControl::Close) | None => break None,
                    Some(other) => unreachable!("invalid message-loop control {other:?}"),
                }
            }
            _ = sleep_until(deadline), if idle_timeout => break None,
        }
    };
    reader_task.abort();
    let _ = events.send(ConnEvent::Closed { conn_id, error });
}

async fn run_file_loop(
    conn_id: ConnId,
    events: mpsc::UnboundedSender<ConnEvent>,
    mut control: mpsc::UnboundedReceiver<ConnControl>,
    limits: SharedLimits,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: BufWriter<OwnedWriteHalf>,
) {
    let mut init_exchanged = false;
    let error = loop {
        tokio::select! {
            init_result = reader.read_u32_le(), if !init_exchanged => {
                match init_result {
                    Ok(token) => {
                        init_exchanged = true;
                        if events.send(ConnEvent::FileInit { conn_id, token }).is_err() {
                            return;
                        }
                    }
                    Err(error) => break Some(error.to_string()),
                }
            }
            ctrl = control.recv() => {
                match ctrl {
                    Some(ConnControl::SendFileInit(token)) => {
                        init_exchanged = true;
                        let bytes = FileTransferInit { token }.to_bytes();
                        if write_all(&mut writer, &bytes).await.is_err() {
                            break Some("write failed".into());
                        }
                    }
                    Some(ConnControl::Download { file, offset, bytes_left }) => {
                        let offset_bytes = FileOffset { offset }.to_bytes();
                        if write_all(&mut writer, &offset_bytes).await.is_err() {
                            break Some("write failed".into());
                        }
                        let task = TransferTask { conn_id, events: &events, control: &mut control, limits: &limits };
                        run_download(task, &mut reader, file, bytes_left).await;
                        return;
                    }
                    Some(ConnControl::Upload { file, size }) => {
                        let task = TransferTask { conn_id, events: &events, control: &mut control, limits: &limits };
                        run_upload(task, &mut writer, &mut reader, file, size).await;
                        return;
                    }
                    Some(ConnControl::Close) | None => break None,
                    Some(other) => unreachable!("invalid file control {other:?}"),
                }
            }
        }
    };
    let _ = events.send(ConnEvent::Closed { conn_id, error });
}

struct TransferTask<'a> {
    conn_id: ConnId,
    events: &'a mpsc::UnboundedSender<ConnEvent>,
    control: &'a mut mpsc::UnboundedReceiver<ConnControl>,
    limits: &'a SharedLimits,
}

async fn run_download(
    task: TransferTask<'_>,
    reader: &mut BufReader<OwnedReadHalf>,
    file: std::fs::File,
    mut bytes_left: u64,
) {
    let TransferTask {
        conn_id,
        events,
        control,
        limits,
    } = task;
    let mut file = tokio::fs::File::from_std(file);
    let mut buffer = vec![0u8; 65536];
    let mut last_report = Instant::now();
    let mut throttle = Throttle::new();
    let error = loop {
        if bytes_left == 0 {
            if let Err(error) = file.flush().await {
                let _ = events.send(ConnEvent::FileError {
                    conn_id,
                    error: error.to_string(),
                });
                break None;
            }
            let _ = events.send(ConnEvent::DownloadProgress {
                conn_id,
                bytes_left: 0,
                bytes_received: 0,
            });
            let _ = events.send(ConnEvent::FileDone { conn_id });
            break None;
        }
        let max_read = buffer.len().min(bytes_left as usize);
        tokio::select! {
            read_result = reader.read(&mut buffer[..max_read]) => {
                match read_result {
                    Ok(0) => break Some("connection closed".into()),
                    Ok(count) => {
                        if let Err(error) = file.write_all(&buffer[..count]).await {
                            let _ = events.send(ConnEvent::FileError { conn_id, error: error.to_string() });
                            break None;
                        }
                        bytes_left -= count as u64;
                        throttle
                            .pace(count as u64, limits.download_bps.load(Ordering::Relaxed))
                            .await;
                        if bytes_left > 0 && last_report.elapsed() >= Duration::from_secs(1) {
                            last_report = Instant::now();
                            let _ = events.send(ConnEvent::DownloadProgress {
                                conn_id,
                                bytes_left,
                                bytes_received: count as u64,
                            });
                        }
                    }
                    Err(error) => break Some(error.to_string()),
                }
            }
            ctrl = control.recv() => {
                match ctrl {
                    Some(ConnControl::Close) | None => break None,
                    Some(other) => unreachable!("invalid download control {other:?}"),
                }
            }
        }
    };
    let _ = file.flush().await;
    let _ = events.send(ConnEvent::Closed { conn_id, error });
}

async fn run_upload(
    task: TransferTask<'_>,
    writer: &mut BufWriter<OwnedWriteHalf>,
    reader: &mut BufReader<OwnedReadHalf>,
    file: std::fs::File,
    size: u64,
) {
    let TransferTask {
        conn_id,
        events,
        control,
        limits,
    } = task;
    let offset = match reader.read_u64_le().await {
        Ok(offset) => offset,
        Err(error) => {
            let _ = events.send(ConnEvent::Closed {
                conn_id,
                error: Some(error.to_string()),
            });
            return;
        }
    };
    let _ = events.send(ConnEvent::FileOffsetReceived { conn_id, offset });

    let mut file = tokio::fs::File::from_std(file);
    if let Err(error) = file.seek(SeekFrom::Start(offset)).await {
        let _ = events.send(ConnEvent::FileError {
            conn_id,
            error: error.to_string(),
        });
        let _ = events.send(ConnEvent::Closed {
            conn_id,
            error: None,
        });
        return;
    }

    let mut bytes_sent = 0u64;
    let mut buffer = vec![0u8; 65536];
    let mut last_report = Instant::now();
    let mut throttle = Throttle::new();
    let error = loop {
        match control.try_recv() {
            Ok(ConnControl::Close) | Err(mpsc::error::TryRecvError::Disconnected) => break None,
            Ok(other) => unreachable!("invalid upload control {other:?}"),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if offset + bytes_sent >= size {
            let _ = events.send(ConnEvent::UploadProgress {
                conn_id,
                offset,
                bytes_sent,
            });
            let _ = events.send(ConnEvent::FileDone { conn_id });
            break None;
        }
        let count = match file.read(&mut buffer).await {
            Ok(0) => {
                let _ = events.send(ConnEvent::FileError {
                    conn_id,
                    error: "file truncated".into(),
                });
                break None;
            }
            Ok(count) => count,
            Err(error) => {
                let _ = events.send(ConnEvent::FileError {
                    conn_id,
                    error: error.to_string(),
                });
                break None;
            }
        };
        if write_all(writer, &buffer[..count]).await.is_err() {
            break Some("write failed".into());
        }
        bytes_sent += count as u64;
        throttle
            .pace(count as u64, limits.upload_bps.load(Ordering::Relaxed))
            .await;
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            let _ = events.send(ConnEvent::UploadProgress {
                conn_id,
                offset,
                bytes_sent,
            });
        }
    };
    let _ = events.send(ConnEvent::Closed { conn_id, error });
}
