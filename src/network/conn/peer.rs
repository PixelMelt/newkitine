use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use tracing::debug;

use super::file::run_file_loop;
use super::{
    ConnControl, ConnEvent, FRAME_QUEUE_CAPACITY, PeerTask, SharedAllowed, connect, write_all,
};
use crate::network::ConnId;
use crate::network::codec::{
    FrameError, MAX_CONTROL_MESSAGE_SIZE, MAX_LARGE_RESPONSE_SIZE, MAX_PEER_MESSAGE_SIZE,
    read_frame_u8, read_payload,
};
use crate::protocol::{DistributedMessage, PeerInitMessage, PeerMessage};
use crate::types::ConnectionType;

const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const GHOST_TIMEOUT: Duration = Duration::from_secs(10);

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
            let _ = task
                .events
                .send(ConnEvent::Closed {
                    conn_id: task.conn_id,
                    error: Some(error),
                })
                .await;
            return;
        }
    };
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    if write_all(&mut writer, &init.to_bytes()).await.is_err() {
        let _ = task
            .events
            .send(ConnEvent::Closed {
                conn_id: task.conn_id,
                error: Some("write failed".into()),
            })
            .await;
        return;
    }
    if task
        .events
        .send(ConnEvent::OutgoingEstablished {
            conn_id: task.conn_id,
        })
        .await
        .is_err()
    {
        return;
    }
    run_typed_loop(task, reader, writer, username, conn_type).await;
}

pub async fn run_incoming_peer(mut task: PeerTask, stream: TcpStream, addr: SocketAddr) {
    if let Err(error) = stream.set_nodelay(true) {
        tracing::warn!(%error, %addr, "cannot set nodelay, continuing without it");
    }

    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let writer = BufWriter::new(write_half);

    let init = match timeout(
        GHOST_TIMEOUT,
        read_frame_u8(&mut reader, MAX_CONTROL_MESSAGE_SIZE),
    )
    .await
    {
        Ok(Ok((code, payload))) => match PeerInitMessage::parse(code, &payload) {
            Ok(init) => init,
            Err(error) => {
                let _ = task
                    .events
                    .send(ConnEvent::Closed {
                        conn_id: task.conn_id,
                        error: Some(error.to_string()),
                    })
                    .await;
                return;
            }
        },
        Ok(Err(error)) => {
            let _ = task
                .events
                .send(ConnEvent::Closed {
                    conn_id: task.conn_id,
                    error: Some(error.to_string()),
                })
                .await;
            return;
        }
        Err(_) => {
            let _ = task
                .events
                .send(ConnEvent::Closed {
                    conn_id: task.conn_id,
                    error: Some("ghost connection".into()),
                })
                .await;
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
        .await
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
                let _ = task
                    .events
                    .send(ConnEvent::Closed {
                        conn_id: task.conn_id,
                        error: None,
                    })
                    .await;
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
            let (frames_tx, frames) = mpsc::channel(FRAME_QUEUE_CAPACITY);
            let reader_task = tokio::spawn(read_peer_frames(reader, frames_tx, allowed, username));
            run_message_loop(conn_id, events, control, frames, writer, reader_task, true).await;
        }
        ConnectionType::Distributed => {
            let (frames_tx, frames) = mpsc::channel(FRAME_QUEUE_CAPACITY);
            let reader_task = tokio::spawn(read_distrib_frames(reader, frames_tx));
            run_message_loop(conn_id, events, control, frames, writer, reader_task, true).await;
        }
        ConnectionType::File => {
            run_file_loop(conn_id, events, control, limits, reader, writer).await;
        }
    }
}

const OFFLOADED_PARSE_THRESHOLD: usize = 1048576;

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
    frames: mpsc::Sender<PeerFrame>,
    allowed: SharedAllowed,
    username: String,
) {
    loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(error) => {
                let _ = frames.send(PeerFrame::Fatal(Some(error.to_string()))).await;
                return;
            }
        };
        if size < 4 {
            let _ = frames
                .send(PeerFrame::Fatal(Some("truncated peer frame".into())))
                .await;
            return;
        }
        let mut frame = vec![0u8; 4];
        if let Err(error) = reader.read_exact(&mut frame).await {
            let _ = frames.send(PeerFrame::Fatal(Some(error.to_string()))).await;
            return;
        }
        let code = u32::from_le_bytes(frame[..4].try_into().unwrap());

        let is_large_response = matches!(code, 5 | 16);
        let limit = if code == 5 {
            MAX_LARGE_RESPONSE_SIZE
        } else {
            MAX_PEER_MESSAGE_SIZE
        };
        if size - 4 > limit {
            let _ = frames
                .send(PeerFrame::Fatal(Some(format!(
                    "peer message size {size} exceeds limit {limit}"
                ))))
                .await;
            return;
        }
        if is_large_response && !is_large_response_allowed(&allowed, &username, code) {
            let _ = frames.send(PeerFrame::Fatal(None)).await;
            return;
        }

        let payload = match read_payload(&mut reader, size - 4).await {
            Ok(payload) => payload,
            Err(error) => {
                let _ = frames.send(PeerFrame::Fatal(Some(error.to_string()))).await;
                return;
            }
        };
        let parsed = if payload.len() >= OFFLOADED_PARSE_THRESHOLD {
            tokio::task::spawn_blocking(move || PeerMessage::parse(code, &payload))
                .await
                .expect("peer parse task panicked")
        } else {
            PeerMessage::parse(code, &payload)
        };
        match parsed {
            Ok(message) => {
                if !is_parsed_message_allowed(&allowed, &message) {
                    debug!(code, username, "dropping disallowed peer response");
                    continue;
                }
                if frames
                    .send(PeerFrame::Message(ConnEventPayload::Peer(message)))
                    .await
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

fn is_parsed_message_allowed(allowed: &SharedAllowed, message: &PeerMessage) -> bool {
    let allowed = allowed.read().unwrap();
    match message {
        PeerMessage::FileSearchResponse { token, .. } => allowed.search_tokens.contains(token),
        PeerMessage::FolderContentsResponse { .. } => false,
        _ => true,
    }
}

async fn read_distrib_frames(
    mut reader: BufReader<OwnedReadHalf>,
    frames: mpsc::Sender<PeerFrame>,
) {
    loop {
        match read_frame_u8(&mut reader, MAX_CONTROL_MESSAGE_SIZE).await {
            Ok((code, payload)) => match DistributedMessage::parse(code, &payload) {
                Ok(message) => {
                    if frames
                        .send(PeerFrame::Message(ConnEventPayload::Distrib(message)))
                        .await
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
                let _ = frames.send(PeerFrame::Fatal(Some(error.to_string()))).await;
                return;
            }
        }
    }
}

async fn run_message_loop(
    conn_id: ConnId,
    events: mpsc::Sender<ConnEvent>,
    mut control: mpsc::Receiver<ConnControl>,
    mut frames: mpsc::Receiver<PeerFrame>,
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
                        if events.send(event).await.is_err() {
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
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
}
