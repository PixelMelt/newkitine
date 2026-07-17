use std::io::SeekFrom;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{ConnControl, ConnEvent, SharedLimits, write_all};
use crate::network::ConnId;
use crate::protocol::{FileOffset, FileTransferInit};

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

pub(super) async fn run_file_loop(
    conn_id: ConnId,
    events: mpsc::Sender<ConnEvent>,
    mut control: mpsc::Receiver<ConnControl>,
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
        }
    };
    let _ = events.send(ConnEvent::Closed { conn_id, error }).await;
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
    let mut throttle = Throttle::new();
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
        let max_read = buffer.len().min(bytes_left as usize);
        tokio::select! {
            read_result = reader.read(&mut buffer[..max_read]) => {
                match read_result {
                    Ok(0) => break Some("connection closed".into()),
                    Ok(count) => {
                        if let Err(error) = file.write_all(&buffer[..count]).await {
                            let _ = events.send(ConnEvent::FileError { conn_id, error: error.to_string() }).await;
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
    let offset = match reader.read_u64_le().await {
        Ok(offset) => offset,
        Err(error) => {
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
    let mut throttle = Throttle::new();
    let error = loop {
        match control.try_recv() {
            Ok(ConnControl::Close) | Err(mpsc::error::TryRecvError::Disconnected) => break None,
            Ok(other) => unreachable!("invalid upload control {other:?}"),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
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
        let count = match file.read(&mut buffer).await {
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
        if write_all(writer, &buffer[..count]).await.is_err() {
            break Some("write failed".into());
        }
        bytes_sent += count as u64;
        throttle
            .pace(count as u64, limits.upload_bps.load(Ordering::Relaxed))
            .await;
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
