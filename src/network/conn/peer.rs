use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{sleep_until, timeout};
use tracing::debug;

use super::file::run_file_loop;
use super::{
    ConnControl, ConnEvent, FRAME_QUEUE_CAPACITY, OUTGOING_QUEUE_CAPACITY, PEER_IDLE_TIMEOUT,
    PeerTask, SharedAllowed, SharedTraffic, SocketReader, connect, consumed, split_tracked,
    write_all,
};
use crate::network::codec::{
    MAX_CONTROL_MESSAGE_SIZE, MAX_LARGE_RESPONSE_SIZE, MAX_PEER_MESSAGE_SIZE, read_frame_u8,
    read_payload,
};
use crate::network::{AllowedResponse, ConnId};
use crate::protocol::{DistributedMessage, PeerInitMessage, PeerMessage, ResponseHeader};
use crate::types::ConnectionType;

const GHOST_TIMEOUT: Duration = Duration::from_secs(10);

async fn closed(task: &PeerTask, error: Option<String>) {
    let _ = task
        .events
        .send(ConnEvent::Closed {
            conn_id: task.conn_id,
            error,
        })
        .await;
}

pub async fn run_outgoing_peer(
    mut task: PeerTask,
    addr: SocketAddr,
    username: String,
    init: PeerInitMessage,
    conn_type: ConnectionType,
) {
    let connected = tokio::select! {
        biased;
        ctrl = task.control.recv() => match ctrl {
            Some(ConnControl::Close) | None => return closed(&task, None).await,
            Some(other) => unreachable!("invalid pre-connect control {other:?}"),
        },
        connected = connect(addr) => connected,
    };
    let stream = match connected {
        Ok(stream) => stream,
        Err(error) => return closed(&task, Some(error)).await,
    };
    let (reader, mut writer) = split_tracked(stream, task.traffic.clone());

    if write_all(&mut writer, &init.to_bytes()).await.is_err() {
        return closed(&task, Some("write failed".into())).await;
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

    let (mut reader, writer) = split_tracked(stream, task.traffic.clone());

    let init = match timeout(
        GHOST_TIMEOUT,
        read_frame_u8(&mut reader, MAX_CONTROL_MESSAGE_SIZE),
    )
    .await
    {
        Ok(Ok((code, payload))) => {
            PeerInitMessage::parse(code, &payload).map_err(|error| error.to_string())
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("ghost connection".into()),
    };
    let init = match init {
        Ok(init) => init,
        Err(error) => return closed(&task, Some(error)).await,
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
            Some(ConnControl::Close) | None => return closed(&task, None).await,
            Some(other) => unreachable!("invalid pre-init control {other:?}"),
        },
    };

    run_typed_loop(task, reader, writer, username, conn_type).await;
}

type Frame = Result<ConnEvent, Option<String>>;

async fn run_typed_loop(
    task: PeerTask,
    reader: SocketReader,
    writer: BufWriter<OwnedWriteHalf>,
    username: String,
    conn_type: ConnectionType,
) {
    let PeerTask {
        conn_id,
        events,
        mut control,
        allowed,
        limits,
        traffic,
    } = task;
    let (frames_tx, mut frames) = mpsc::channel(FRAME_QUEUE_CAPACITY);
    let reader_task = match conn_type {
        ConnectionType::Peer => tokio::spawn(read_peer_frames(
            conn_id,
            reader,
            frames_tx,
            allowed,
            username.clone(),
        )),
        ConnectionType::Distributed => {
            tokio::spawn(read_distrib_frames(conn_id, reader, frames_tx))
        }
        ConnectionType::File => {
            run_file_loop(conn_id, events, control, limits, reader, writer).await;
            return;
        }
    };
    let (outgoing, outgoing_rx) = mpsc::channel(OUTGOING_QUEUE_CAPACITY);
    let mut writer_task = tokio::spawn(write_frames(
        writer.into_inner(),
        outgoing_rx,
        traffic.clone(),
    ));
    traffic.touch();
    let mut deadline = traffic.last_active() + PEER_IDLE_TIMEOUT;
    let mut unsent = Vec::new();
    let error = loop {
        tokio::select! {
            frame = frames.recv() => {
                let event = match frame {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => break error,
                    None => break Some("connection closed".into()),
                };
                if events.send(event).await.is_err() {
                    break None;
                }
            }
            ctrl = control.recv() => {
                let (bytes, messages) = match ctrl {
                    Some(ConnControl::Send(bytes)) => (bytes, Vec::new()),
                    Some(ConnControl::SendPeer(messages)) => (
                        messages.iter().flat_map(PeerMessage::to_bytes).collect(),
                        messages,
                    ),
                    Some(ConnControl::Close) | None => break None,
                    Some(other) => unreachable!("invalid message-loop control {other:?}"),
                };
                if let Err(error) = outgoing.try_send(bytes) {
                    unsent.extend(messages);
                    break match error {
                        TrySendError::Full(_) => Some("outbound queue overflowed".into()),
                        TrySendError::Closed(_) => {
                            Some((&mut writer_task).await.expect("peer writer task panicked"))
                        }
                    };
                }
            }
            written = &mut writer_task => {
                break Some(written.expect("peer writer task panicked"));
            }
            _ = sleep_until(deadline) => {
                let idle_deadline = traffic.last_active() + PEER_IDLE_TIMEOUT;
                if idle_deadline <= deadline {
                    break None;
                }
                deadline = idle_deadline;
            }
        }
    };
    reader_task.abort();
    writer_task.abort();
    control.close();
    while let Ok(control) = control.try_recv() {
        if let ConnControl::SendPeer(messages) = control {
            unsent.extend(messages);
        }
    }
    if !unsent.is_empty() {
        let _ = events
            .send(ConnEvent::Unsent {
                username,
                messages: unsent,
            })
            .await;
    }
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
}

const OFFLOADED_PARSE_THRESHOLD: usize = 1048576;

async fn read_peer_frames(
    conn_id: ConnId,
    mut reader: SocketReader,
    frames: mpsc::Sender<Frame>,
    allowed: SharedAllowed,
    username: String,
) {
    let mut received_through = consumed(&reader);
    let fatal = loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(error) => break Some(error.to_string()),
        };
        if size < 4 {
            break Some("truncated peer frame".into());
        }
        let code = match reader.read_u32_le().await {
            Ok(code) => code,
            Err(error) => break Some(error.to_string()),
        };

        let is_large_response = matches!(code, 5 | 16);
        let limit = if is_large_response {
            MAX_LARGE_RESPONSE_SIZE
        } else {
            MAX_PEER_MESSAGE_SIZE
        };
        if size - 4 > limit {
            break Some(format!("peer message size {size} exceeds limit {limit}"));
        }
        if is_large_response && !is_large_response_allowed(&allowed, &username, code) {
            break None;
        }

        let payload = match read_payload(&mut reader, size - 4).await {
            Ok(payload) => payload,
            Err(error) => break Some(error.to_string()),
        };
        received_through += 4 + size as u64;

        match PeerMessage::response_header(code, &payload) {
            Ok(Some(header)) if !is_response_allowed(&allowed, &username, &header) => {
                debug!(code, username, "dropping unsolicited peer response");
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                debug!(code, %error, "dropping unparsable peer response header");
                continue;
            }
        }

        let parsed = if payload.len() >= OFFLOADED_PARSE_THRESHOLD || is_compressed(code) {
            tokio::task::spawn_blocking(move || PeerMessage::parse(code, &payload))
                .await
                .expect("peer parse task panicked")
        } else {
            PeerMessage::parse(code, &payload)
        };
        match parsed {
            Ok(message) => {
                let event = ConnEvent::Peer {
                    conn_id,
                    message,
                    received_through,
                };
                if frames.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            Err(error) => debug!(code, %error, "dropping unparsable peer message"),
        }
    };
    let _ = frames.send(Err(fatal)).await;
}

fn is_compressed(code: u32) -> bool {
    matches!(code, 5 | 9 | 37)
}

fn is_large_response_allowed(allowed: &SharedAllowed, username: &str, code: u32) -> bool {
    let response = match code {
        5 => AllowedResponse::SharedList(username.to_owned()),
        16 => AllowedResponse::UserInfo(username.to_owned()),
        _ => unreachable!("peer code {code} is not a large response"),
    };
    allowed.read().unwrap().contains(&response)
}

fn is_response_allowed(allowed: &SharedAllowed, username: &str, header: &ResponseHeader) -> bool {
    let response = match header {
        ResponseHeader::Search { token } => AllowedResponse::Search(*token),
        ResponseHeader::FolderContents { directory } => AllowedResponse::FolderContents {
            username: username.to_owned(),
            directory: directory.clone(),
        },
    };
    allowed.read().unwrap().contains(&response)
}

async fn read_distrib_frames(
    conn_id: ConnId,
    mut reader: SocketReader,
    frames: mpsc::Sender<Frame>,
) {
    loop {
        match read_frame_u8(&mut reader, MAX_CONTROL_MESSAGE_SIZE).await {
            Ok((code, payload)) => match DistributedMessage::parse(code, &payload) {
                Ok(message) => {
                    let event = ConnEvent::Distrib { conn_id, message };
                    if frames.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                Err(error) => debug!(code, %error, "dropping unparsable distributed message"),
            },
            Err(error) => {
                let _ = frames.send(Err(Some(error.to_string()))).await;
                return;
            }
        }
    }
}

async fn write_frames(
    mut writer: OwnedWriteHalf,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    traffic: SharedTraffic,
) -> String {
    while let Some(bytes) = outgoing.recv().await {
        let mut written = 0;
        while written < bytes.len() {
            match timeout(PEER_IDLE_TIMEOUT, writer.write(&bytes[written..])).await {
                Ok(Ok(0)) => return "write failed: connection closed".into(),
                Ok(Ok(count)) => {
                    written += count;
                    traffic.touch();
                }
                Ok(Err(error)) => return format!("write failed: {error}"),
                Err(_) => return "write stalled".into(),
            }
        }
        traffic.record_send_complete();
    }
    "outgoing queue closed".into()
}
