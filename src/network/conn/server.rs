use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, BufReader, BufWriter};
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc;
use tracing::debug;

use super::{ConnControl, ConnEvent, FRAME_QUEUE_CAPACITY, connect, write_all};
use crate::network::codec::MAX_MESSAGE_SIZE_LARGE;
use crate::protocol::ServerResponse;

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
        if !(4..=MAX_MESSAGE_SIZE_LARGE).contains(&size) {
            let _ = frames
                .send(Err(format!("invalid server message size {size}")))
                .await;
            return;
        }
        let mut frame = vec![0u8; size];
        if let Err(error) = reader.read_exact(&mut frame).await {
            let _ = frames.send(Err(error.to_string())).await;
            return;
        }
        let code = u32::from_le_bytes(frame[..4].try_into().unwrap());
        match ServerResponse::parse(code, &frame[4..]) {
            Ok(message) => {
                if frames.send(Ok(message)).await.is_err() {
                    return;
                }
            }
            Err(error) => debug!(code, %error, "dropping unparsable server message"),
        }
    }
}
