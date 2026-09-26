use std::io::SeekFrom;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until, timeout};

use super::bandwidth::{Bandwidth, Grant};
use super::{ConnControl, ConnEvent, PEER_IDLE_TIMEOUT, SharedLimits, write_all};
use crate::network::ConnId;
use crate::protocol::{FileOffset, FileTransferInit};

pub(super) async fn run_file_loop(
    conn_id: ConnId,
    events: mpsc::Sender<ConnEvent>,
    mut control: mpsc::Receiver<ConnControl>,
    limits: SharedLimits,
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: BufWriter<OwnedWriteHalf>,
) {
    let mut init_exchanged = false;
    let deadline = Instant::now() + PEER_IDLE_TIMEOUT;
    let error = loop {
        tokio::select! {
            init_result = reader.read_u32_le(), if !init_exchanged => {
                match init_result {
                    Ok(token) => {
                        init_exchanged = true;
                        if events.send(ConnEvent::FileInit { conn_id, token }).await.is_err() {
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
            _ = sleep_until(deadline) => break Some("file init timed out".into()),
        }
    };
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
}

async fn take_turn(
    bandwidth: &Bandwidth,
    control: &mut mpsc::Receiver<ConnControl>,
    max_len: u64,
) -> Option<Grant> {
    loop {
        let changed = bandwidth.changed();
        let Some(wait_until) = bandwidth.wait_until() else {
            return Some(bandwidth.grant(max_len));
        };
        tokio::select! {
            _ = sleep_until(wait_until) => return Some(bandwidth.grant(max_len)),
            _ = changed => {}
            ctrl = control.recv() => match ctrl {
                Some(ConnControl::Close) | None => return None,
                Some(other) => unreachable!("invalid transfer control {other:?}"),
            },
        }
    }
}

struct TransferTask<'a> {
    conn_id: ConnId,
    events: &'a mpsc::Sender<ConnEvent>,
    control: &'a mut mpsc::Receiver<ConnControl>,
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
    let error = loop {
        if bytes_left == 0 {
            if let Err(error) = file.flush().await {
                let _ = events
                    .send(ConnEvent::FileError {
                        conn_id,
                        error: error.to_string(),
                    })
                    .await;
                break None;
            }
            let _ = events
                .send(ConnEvent::DownloadProgress {
                    conn_id,
                    bytes_left: 0,
                })
                .await;
            let _ = events.send(ConnEvent::FileDone { conn_id }).await;
            break None;
        }
        let Some(grant) = take_turn(
            &limits.download,
            control,
            bytes_left.min(buffer.len() as u64),
        )
        .await
        else {
            break None;
        };
        tokio::select! {
            read_result = reader.read(&mut buffer[..grant.len]) => {
                match read_result {
                    Ok(0) => break Some("connection closed".into()),
                    Ok(count) => {
                        if let Err(error) = file.write_all(&buffer[..count]).await {
                            let _ = events.send(ConnEvent::FileError { conn_id, error: error.to_string() }).await;
                            break None;
                        }
                        bytes_left -= count as u64;
                        limits.download.charge(&grant, count as u64);
                        if bytes_left > 0 && last_report.elapsed() >= Duration::from_secs(1) {
                            last_report = Instant::now();
                            let _ = events.send(ConnEvent::DownloadProgress {
                                conn_id,
                                bytes_left,
                            }).await;
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
            _ = sleep(PEER_IDLE_TIMEOUT) => break Some("download stalled".into()),
        }
    };
    let _ = file.flush().await;
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
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
    let offset = match timeout(PEER_IDLE_TIMEOUT, reader.read_u64_le()).await {
        Ok(Ok(offset)) => offset,
        Err(_) => {
            let _ = events
                .send(ConnEvent::Closed {
                    conn_id,
                    error: Some("offset read timed out".into()),
                })
                .await;
            return;
        }
        Ok(Err(error)) => {
            let _ = events
                .send(ConnEvent::Closed {
                    conn_id,
                    error: Some(error.to_string()),
                })
                .await;
            return;
        }
    };
    let _ = events
        .send(ConnEvent::FileOffsetReceived { conn_id, offset })
        .await;

    let mut file = tokio::fs::File::from_std(file);
    if let Err(error) = file.seek(SeekFrom::Start(offset)).await {
        let _ = events
            .send(ConnEvent::FileError {
                conn_id,
                error: error.to_string(),
            })
            .await;
        let _ = events
            .send(ConnEvent::Closed {
                conn_id,
                error: None,
            })
            .await;
        return;
    }

    let mut bytes_sent = 0u64;
    let mut buffer = vec![0u8; 65536];
    let mut last_report = Instant::now();
    let error = loop {
        if offset + bytes_sent >= size {
            let _ = events
                .send(ConnEvent::UploadProgress {
                    conn_id,
                    offset,
                    bytes_sent,
                })
                .await;
            let _ = events.send(ConnEvent::FileDone { conn_id }).await;
            break None;
        }
        let remaining = size - offset - bytes_sent;
        let Some(grant) =
            take_turn(&limits.upload, control, remaining.min(buffer.len() as u64)).await
        else {
            break None;
        };
        let count = match file.read(&mut buffer[..grant.len]).await {
            Ok(0) => {
                let _ = events
                    .send(ConnEvent::FileError {
                        conn_id,
                        error: "file truncated".into(),
                    })
                    .await;
                break None;
            }
            Ok(count) => count,
            Err(error) => {
                let _ = events
                    .send(ConnEvent::FileError {
                        conn_id,
                        error: error.to_string(),
                    })
                    .await;
                break None;
            }
        };
        let written = tokio::select! {
            written = timeout(PEER_IDLE_TIMEOUT, write_all(writer, &buffer[..count])) => written,
            ctrl = control.recv() => match ctrl {
                Some(ConnControl::Close) | None => break None,
                Some(other) => unreachable!("invalid upload control {other:?}"),
            },
        };
        match written {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break Some("write failed".into()),
            Err(_) => break Some("upload stalled".into()),
        }
        bytes_sent += count as u64;
        limits.upload.charge(&grant, count as u64);
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            let _ = events
                .send(ConnEvent::UploadProgress {
                    conn_id,
                    offset,
                    bytes_sent,
                })
                .await;
        }
    };
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use tokio::net::{TcpListener, TcpStream};

    use super::*;
    use crate::network::conn::TransferLimits;

    #[tokio::test]
    async fn upload_stops_at_the_advertised_size_when_the_file_grew() {
        let path = std::env::temp_dir().join(format!(
            "newkitine-upload-cap-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&vec![7u8; 200_000])
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = stream.into_split();
        let (events_tx, mut events) = mpsc::channel(64);
        let (control_tx, control) = mpsc::channel(8);
        let conn_task = tokio::spawn(run_file_loop(
            1,
            events_tx,
            control,
            Arc::new(TransferLimits::default()),
            BufReader::new(read_half),
            BufWriter::new(write_half),
        ));

        control_tx.send(ConnControl::SendFileInit(9)).await.unwrap();
        control_tx
            .send(ConnControl::Upload {
                file,
                size: 100_000,
            })
            .await
            .unwrap();
        assert_eq!(peer.read_u32_le().await.unwrap(), 9);
        peer.write_u64_le(1_000).await.unwrap();
        let mut received = Vec::new();
        peer.read_to_end(&mut received).await.unwrap();
        conn_task.await.unwrap();

        assert_eq!(received.len(), 99_000);
        let mut saw_done = false;
        while let Ok(event) = events.try_recv() {
            match event {
                ConnEvent::FileDone { .. } => saw_done = true,
                ConnEvent::FileError { error, .. } => panic!("unexpected file error {error}"),
                _ => {}
            }
        }
        assert!(saw_done);
    }
}
