use std::collections::HashMap;
use std::fs;
use std::io::Seek;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::network::{ConnId, NetworkCommand, NetworkHandle};
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::{FileAttributes, TransferRejectReason};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    GettingStatus,
    Transferring,
    Finished,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Transfer {
    pub username: String,
    pub virtual_path: String,
    pub size: u64,
    pub attributes: FileAttributes,
    pub status: TransferStatus,
    pub token: Option<u32>,
    pub conn_id: Option<ConnId>,
    pub bytes_done: u64,
    pub retry_attempt: bool,
    pub size_changed: bool,
    pub activated_at: Option<Instant>,
}

impl Transfer {
    pub fn key(&self) -> (String, String) {
        (self.username.clone(), self.virtual_path.clone())
    }

    pub fn basename(&self) -> String {
        self.virtual_path.rsplit('\\').next().unwrap_or(&self.virtual_path).to_owned()
    }
}

#[derive(Debug)]
pub enum DownloadUpdate {
    Started { username: String, virtual_path: String, incomplete_path: PathBuf },
    Progress { username: String, virtual_path: String, bytes_done: u64, size: u64 },
    Finished { username: String, virtual_path: String, file_path: PathBuf },
    Failed { username: String, virtual_path: String, reason: String },
}

pub struct Downloads {
    net: NetworkHandle,
    download_dir: PathBuf,
    incomplete_dir: PathBuf,
    transfers: HashMap<(String, String), Transfer>,
    active_tokens: HashMap<(String, u32), String>,
    conn_tokens: HashMap<ConnId, (String, u32)>,
}

impl Downloads {
    pub fn new(net: NetworkHandle, download_dir: PathBuf, incomplete_dir: PathBuf) -> Self {
        Self {
            net,
            download_dir,
            incomplete_dir,
            transfers: HashMap::new(),
            active_tokens: HashMap::new(),
            conn_tokens: HashMap::new(),
        }
    }

    pub fn set_dirs(&mut self, download_dir: PathBuf, incomplete_dir: PathBuf) {
        self.download_dir = download_dir;
        self.incomplete_dir = incomplete_dir;
    }

    pub fn enqueue(
        &mut self,
        username: String,
        virtual_path: String,
        size: u64,
        attributes: FileAttributes,
    ) {
        let key = (username.clone(), virtual_path.clone());
        if let Some(existing) = self.transfers.get(&key)
            && matches!(
                existing.status,
                TransferStatus::Queued
                    | TransferStatus::GettingStatus
                    | TransferStatus::Transferring
            )
        {
            debug!(username, virtual_path, "download already in progress");
            return;
        }
        self.transfers.insert(
            key,
            Transfer {
                username: username.clone(),
                virtual_path: virtual_path.clone(),
                size,
                attributes,
                status: TransferStatus::Queued,
                token: None,
                conn_id: None,
                bytes_done: 0,
                retry_attempt: false,
                size_changed: false,
                activated_at: None,
            },
        );
        self.net.server(ServerRequest::WatchUser { user: username.clone() });
        self.net.peer(username, PeerMessage::QueueUpload {
            file: virtual_path,
            legacy_client: false,
        });
    }

    pub fn abort(&mut self, username: &str, virtual_path: &str) {
        let key = (username.to_owned(), virtual_path.to_owned());
        let Some(mut transfer) = self.transfers.remove(&key) else { return };
        if let Some(token) = transfer.token.take() {
            self.active_tokens.remove(&(transfer.username.clone(), token));
        }
        if let Some(conn_id) = transfer.conn_id.take() {
            self.conn_tokens.remove(&conn_id);
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
    }

    pub fn retry_offline(&mut self, username: &str) {
        let keys: Vec<(String, String)> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.username == username
                    && matches!(
                        &transfer.status,
                        TransferStatus::Failed(reason) if reason == "user is offline"
                    )
            })
            .map(Transfer::key)
            .collect();
        for key in keys {
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.status = TransferStatus::Queued;
            transfer.retry_attempt = false;
            self.net.peer(key.0, PeerMessage::QueueUpload {
                file: key.1,
                legacy_client: false,
            });
        }
    }

    pub fn owns_token(&self, username: &str, token: u32) -> bool {
        self.active_tokens.contains_key(&(username.to_owned(), token))
    }

    pub fn handle_transfer_request(
        &mut self,
        username: &str,
        token: u32,
        file: &str,
        filesize: Option<u64>,
    ) {
        let key = (username.to_owned(), file.to_owned());
        let response = match self.transfers.get_mut(&key) {
            Some(transfer)
                if matches!(
                    transfer.status,
                    TransferStatus::Queued
                        | TransferStatus::GettingStatus
                        | TransferStatus::Failed(_)
                ) =>
            {
                if let Some(size) = filesize
                    && size > 0
                {
                    if transfer.size != size && transfer.size != 0 {
                        transfer.size_changed = true;
                    }
                    transfer.size = size;
                }
                transfer.status = TransferStatus::GettingStatus;
                transfer.token = Some(token);
                transfer.activated_at = Some(Instant::now());
                self.active_tokens.insert((username.to_owned(), token), file.to_owned());
                PeerMessage::TransferResponse { token, allowed: true, reason: None, filesize: None }
            }
            Some(transfer) if transfer.status == TransferStatus::Finished => {
                PeerMessage::TransferResponse {
                    token,
                    allowed: false,
                    reason: Some(TransferRejectReason::COMPLETE.into()),
                    filesize: None,
                }
            }
            _ => PeerMessage::TransferResponse {
                token,
                allowed: false,
                reason: Some(TransferRejectReason::CANCELLED.into()),
                filesize: None,
            },
        };
        self.net.peer(username, response);
    }

    pub fn handle_file_transfer_init(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
    ) -> Vec<DownloadUpdate> {
        let Some(virtual_path) = self.active_tokens.get(&(username.to_owned(), token)).cloned()
        else {
            debug!(username, token, "file transfer init with unknown token, closing");
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        let incomplete_path = self.incomplete_file_path(username, &virtual_path);
        let incomplete_dir = self.incomplete_dir.clone();
        let transfer = self.transfers.get_mut(&key).unwrap();
        if transfer.conn_id.is_some() {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        }
        transfer.conn_id = Some(conn_id);
        transfer.activated_at = None;
        self.conn_tokens.insert(conn_id, (username.to_owned(), token));

        let size_changed = transfer.size_changed;
        let open_result = (|| {
            fs::create_dir_all(&incomplete_dir)?;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(&incomplete_path)?;
            if size_changed {
                file.set_len(0)?;
            }
            let offset = file.seek(std::io::SeekFrom::End(0))?;
            Ok::<_, std::io::Error>((file, offset))
        })();

        match open_result {
            Ok((file, offset)) => {
                if transfer.size > offset {
                    transfer.status = TransferStatus::Transferring;
                    transfer.bytes_done = offset;
                    let size = transfer.size;
                    info!(username, virtual_path, offset, size, "download started");
                    self.net.send(NetworkCommand::DownloadFile {
                        conn_id,
                        file,
                        offset,
                        bytes_left: transfer.size - offset,
                    });
                    vec![DownloadUpdate::Started {
                        username: username.to_owned(),
                        virtual_path,
                        incomplete_path,
                    }]
                } else {
                    self.net.send(NetworkCommand::CloseConnection(conn_id));
                    self.finish(&key)
                }
            }
            Err(error) => {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
                self.fail(&key, format!("local file error: {error}"))
            }
        }
    }

    pub fn handle_download_progress(
        &mut self,
        username: &str,
        token: u32,
        bytes_left: u64,
    ) -> Vec<DownloadUpdate> {
        let Some(virtual_path) = self.active_tokens.get(&(username.to_owned(), token)).cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        let Some(transfer) = self.transfers.get_mut(&key) else { return Vec::new() };
        transfer.bytes_done = transfer.size.saturating_sub(bytes_left);
        if bytes_left == 0 {
            return self.finish(&key);
        }
        vec![DownloadUpdate::Progress {
            username: username.to_owned(),
            virtual_path,
            bytes_done: transfer.bytes_done,
            size: transfer.size,
        }]
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: Option<u32>,
        conn_id: ConnId,
    ) -> Vec<DownloadUpdate> {
        let token = match token.or_else(|| {
            self.conn_tokens.get(&conn_id).map(|(_, token)| *token)
        }) {
            Some(token) => token,
            None => return Vec::new(),
        };
        self.conn_tokens.remove(&conn_id);
        let Some(virtual_path) = self.active_tokens.get(&(username.to_owned(), token)).cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path);
        let Some(transfer) = self.transfers.get(&key) else { return Vec::new() };
        match transfer.status {
            TransferStatus::Transferring => self.fail(&key, "connection closed".into()),
            _ => {
                self.active_tokens.remove(&(username.to_owned(), token));
                Vec::new()
            }
        }
    }

    pub fn handle_upload_denied(
        &mut self,
        username: &str,
        file: &str,
        reason: &str,
    ) -> Vec<DownloadUpdate> {
        let key = (username.to_owned(), file.to_owned());
        match self.transfers.get(&key) {
            Some(transfer) if transfer.status != TransferStatus::Finished => {
                self.fail(&key, reason.to_owned())
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_upload_failed(&mut self, username: &str, file: &str) -> Vec<DownloadUpdate> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get_mut(&key) else { return Vec::new() };
        if transfer.status == TransferStatus::Finished {
            return Vec::new();
        }
        if !transfer.retry_attempt {
            transfer.retry_attempt = true;
            transfer.status = TransferStatus::Queued;
            transfer.token = None;
            transfer.conn_id = None;
            transfer.activated_at = None;
            let username = transfer.username.clone();
            let file = transfer.virtual_path.clone();
            self.net.peer(username, PeerMessage::QueueUpload { file, legacy_client: false });
            return Vec::new();
        }
        self.fail(&key, "upload failed".into())
    }

    pub fn handle_peer_connection_error(
        &mut self,
        username: &str,
        unsent: &[PeerMessage],
        is_offline: bool,
    ) -> Vec<DownloadUpdate> {
        let mut updates = Vec::new();
        for message in unsent {
            if let PeerMessage::QueueUpload { file, .. } = message {
                let key = (username.to_owned(), file.clone());
                let reason =
                    if is_offline { "user is offline" } else { "connection timeout" };
                updates.extend(self.fail(&key, reason.into()));
            }
        }
        updates
    }

    pub fn sweep_request_timeouts(&mut self) -> Vec<DownloadUpdate> {
        let expired: Vec<(String, String)> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.status == TransferStatus::GettingStatus
                    && transfer
                        .activated_at
                        .is_some_and(|at| at.elapsed() > TRANSFER_REQUEST_TIMEOUT)
            })
            .map(Transfer::key)
            .collect();
        let mut updates = Vec::new();
        for key in expired {
            updates.extend(self.fail(&key, "request timed out".into()));
        }
        updates
    }

    pub fn reset(&mut self) {
        self.active_tokens.clear();
        self.conn_tokens.clear();
        for transfer in self.transfers.values_mut() {
            if matches!(
                transfer.status,
                TransferStatus::GettingStatus | TransferStatus::Transferring
            ) {
                transfer.status = TransferStatus::Queued;
                transfer.token = None;
                transfer.conn_id = None;
                transfer.activated_at = None;
            }
        }
    }

    fn finish(&mut self, key: &(String, String)) -> Vec<DownloadUpdate> {
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.status = TransferStatus::Finished;
        if let Some(token) = transfer.token.take() {
            self.active_tokens.remove(&(transfer.username.clone(), token));
        }
        let username = transfer.username.clone();
        let virtual_path = transfer.virtual_path.clone();
        let size = transfer.size;
        let basename = transfer.basename();

        let incomplete_path = self.incomplete_file_path(&username, &virtual_path);
        let destination = self.complete_file_path(&basename, size);
        let move_result = (|| {
            fs::create_dir_all(&self.download_dir)?;
            fs::rename(&incomplete_path, &destination)
                .or_else(|_| fs::copy(&incomplete_path, &destination).map(|_| ()).and_then(|_| fs::remove_file(&incomplete_path)))
        })();
        match move_result {
            Ok(()) => {
                info!(username, virtual_path, ?destination, "download finished");
                vec![DownloadUpdate::Finished {
                    username,
                    virtual_path,
                    file_path: destination,
                }]
            }
            Err(error) => {
                let key = key.clone();
                self.fail(&key, format!("cannot move finished download: {error}"))
            }
        }
    }

    fn fail(&mut self, key: &(String, String), reason: String) -> Vec<DownloadUpdate> {
        let Some(transfer) = self.transfers.get_mut(key) else { return Vec::new() };
        if matches!(transfer.status, TransferStatus::Finished | TransferStatus::Failed(_)) {
            return Vec::new();
        }
        transfer.status = TransferStatus::Failed(reason.clone());
        if let Some(token) = transfer.token.take() {
            self.active_tokens.remove(&(transfer.username.clone(), token));
        }
        if let Some(conn_id) = transfer.conn_id.take() {
            self.conn_tokens.remove(&conn_id);
        }
        vec![DownloadUpdate::Failed {
            username: key.0.clone(),
            virtual_path: key.1.clone(),
            reason,
        }]
    }

    fn incomplete_file_path(&self, username: &str, virtual_path: &str) -> PathBuf {
        let digest = md5::compute(format!("{virtual_path}{username}").as_bytes());
        let basename = clean_file_name(
            virtual_path.rsplit('\\').next().unwrap_or(virtual_path),
        );
        self.incomplete_dir.join(format!("INCOMPLETE{digest:x}{basename}"))
    }

    fn complete_file_path(&self, basename: &str, size: u64) -> PathBuf {
        let basename = clean_file_name(basename);
        let (stem, extension) = match basename.rfind('.') {
            Some(index) if index > 0 => basename.split_at(index),
            _ => (basename.as_str(), ""),
        };
        let mut candidate = self.download_dir.join(&basename);
        let mut counter = 1;
        while let Ok(metadata) = fs::metadata(&candidate) {
            if metadata.len() == size {
                break;
            }
            candidate = self.download_dir.join(format!("{stem} ({counter}){extension}"));
            counter += 1;
        }
        candidate
    }
}

fn clean_file_name(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect()
}
