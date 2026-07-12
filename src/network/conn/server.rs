use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, BufReader, BufWriter};
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc;
use tracing::debug;

use super::{ConnControl, ConnEvent, FRAME_QUEUE_CAPACITY, connect, write_all};
use crate::network::codec::{MAX_SERVER_MESSAGE_SIZE, read_payload};
use crate::protocol::{ProtocolError, ServerResponse};

pub async fn run_server_conn(
    address: SocketAddr,
    events: mpsc::Sender<ConnEvent>,
    mut control: mpsc::Receiver<ConnControl>,
) {
    let stream = match connect(address).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events
                .send(ConnEvent::ServerClosed { error: Some(error) })
                .await;
            return;
        }
    };
    let (read_half, write_half) = stream.into_split();
    let mut writer = BufWriter::new(write_half);

    let (frames_tx, mut frames) = mpsc::channel(FRAME_QUEUE_CAPACITY);
    let reader_task = tokio::spawn(read_server_frames(BufReader::new(read_half), frames_tx));

    let error = loop {
        tokio::select! {
            frame = frames.recv() => {
                match frame {
                    Some(Ok(message)) => {
                        if events.send(ConnEvent::ServerMessage(message)).await.is_err() {
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
    let _ = events.send(ConnEvent::ServerClosed { error }).await;
}

async fn read_server_frames(
    mut reader: BufReader<OwnedReadHalf>,
    frames: mpsc::Sender<Result<ServerResponse, String>>,
) {
    loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(error) => {
                let _ = frames.send(Err(error.to_string())).await;
                return;
            }
        };
        if !(4..=MAX_SERVER_MESSAGE_SIZE).contains(&size) {
            let _ = frames
                .send(Err(format!("invalid server message size {size}")))
                .await;
            return;
        }
        let code = match reader.read_u32_le().await {
            Ok(code) => code,
            Err(error) => {
                let _ = frames.send(Err(error.to_string())).await;
                return;
            }
        };
        let payload = match read_payload(&mut reader, size - 4).await {
            Ok(payload) => payload,
            Err(error) => {
                let _ = frames.send(Err(error.to_string())).await;
                return;
            }
        };
        match ServerResponse::parse(code, &payload) {
            Ok(message) => {
                if frames.send(Ok(message)).await.is_err() {
                    return;
                }
            }
            Err(ProtocolError::UnknownMessageCode { .. }) => {
                debug!(code, "ignoring unsupported server message");
            }
            Err(error) => {
                let _ = frames
                    .send(Err(format!("malformed server message {code}: {error}")))
                    .await;
                return;
            }
        }
    }
}
