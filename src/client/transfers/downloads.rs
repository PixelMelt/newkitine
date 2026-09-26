use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info};

use super::files;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use super::{TransferIds, TransferPhase, TransferRejectReason};
use crate::client::{AbortResult, EnqueueResult, RetryResult, TransferWork};
use crate::network::ConnId;
use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::{
    FileAttributes, FileInfo, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const QUEUE_POSITION_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug)]
struct Transfer {
    id: TransferId,
    username: String,
    virtual_path: String,
    folder_path: PathBuf,
    size: u64,
    attributes: FileAttributes,
    phase: TransferPhase,
    bytes_done: u64,
    file_path: Option<String>,
    queue_place: u32,
    speed_bps: u32,
    retry_attempt: bool,
    legacy_attempt: bool,
    size_changed: bool,
    incomplete_path: Option<PathBuf>,
    activated_at: Option<Instant>,
    speed: SpeedMeter,
}

impl Transfer {
    fn key(&self) -> TransferKey {
        (self.username.clone(), self.virtual_path.clone())
    }

    fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            id: self.id,
            direction: TransferDirection::Download,
            username: self.username.clone(),
            virtual_path: self.virtual_path.clone(),
            folder_path: Some(self.folder_path.display().to_string()),
            size: self.size,
            bytes_done: self.bytes_done,
            status: self.phase.status(),
            failure_reason: match &self.phase {
                TransferPhase::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
            file_path: self.file_path.clone(),
            queue_place: self.queue_place,
            speed_bps: if self.phase == TransferPhase::Transferring {
                self.speed_bps
            } else {
                0
            },
            attributes: self.attributes.clone(),
        }
    }
}

pub(super) type PlacementDone = (TransferKey, std::io::Result<PathBuf>);

pub(in crate::client) struct Downloads {
    net: NetworkHandle,
    download_dir: PathBuf,
    incomplete_dir: PathBuf,
    username_subfolders: bool,
    placements: mpsc::UnboundedSender<PlacementDone>,
    transfers: Registry<Transfer>,
    basename_limits: HashMap<PathBuf, usize>,
    queue_positions_at: Instant,
}

impl Downloads {
    pub fn new(
        net: NetworkHandle,
        download_dir: PathBuf,
        incomplete_dir: PathBuf,
        username_subfolders: bool,
        placements: mpsc::UnboundedSender<PlacementDone>,
    ) -> Self {
        Self {
            net,
            download_dir,
            incomplete_dir,
            username_subfolders,
            placements,
            transfers: Registry::default(),
            basename_limits: HashMap::new(),
            queue_positions_at: Instant::now(),
        }
    }

    fn basename_limit(&mut self, dir: &Path) -> usize {
        if let Some(limit) = self.basename_limits.get(dir) {
            return *limit;
        }
        let limit = files::basename_byte_limit(dir);
        self.basename_limits.insert(dir.to_path_buf(), limit);
        limit
    }

    fn destination(&self, username: &str, virtual_path: &str, root: Option<&str>) -> PathBuf {
        files::folder_destination(
            &self.download_dir,
            self.username_subfolders,
            username,
            virtual_path,
            root,
        )
    }

    pub fn set_dirs(
        &mut self,
        download_dir: PathBuf,
        incomplete_dir: PathBuf,
        username_subfolders: bool,
    ) {
        self.download_dir = download_dir;
        self.incomplete_dir = incomplete_dir;
        self.username_subfolders = username_subfolders;
        self.basename_limits.clear();
    }

    pub fn seed(&mut self, seed: TransferSnapshot) {
        let phase = TransferPhase::from_seed(&seed);
        let key = (seed.username.clone(), seed.virtual_path.clone());
        let folder_path = seed.folder_path.map_or_else(
            || self.destination(&seed.username, &seed.virtual_path, None),
            PathBuf::from,
        );
        self.transfers.insert(
            seed.id,
            key,
            Transfer {
                id: seed.id,
                username: seed.username,
                virtual_path: seed.virtual_path,
                folder_path,
                size: seed.size,
                attributes: seed.attributes,
                phase,
                bytes_done: seed.bytes_done,
                file_path: seed.file_path,
                queue_place: 0,
                speed_bps: 0,
                retry_attempt: false,
                legacy_attempt: false,
                size_changed: false,
                incomplete_path: None,
                activated_at: None,
                speed: SpeedMeter::default(),
            },
        );
    }

    pub fn enqueue(
        &mut self,
        ids: &mut TransferIds,
        username: String,
        file: FileInfo,
        root: Option<&str>,
    ) -> (EnqueueResult, Vec<TransferWork>) {
        let folder_path = self.destination(&username, &file.name, root);
        self.start(ids, username, file, folder_path)
    }

    fn start(
        &mut self,
        ids: &mut TransferIds,
        username: String,
        file: FileInfo,
        folder_path: PathBuf,
    ) -> (EnqueueResult, Vec<TransferWork>) {
        let FileInfo {
            name: virtual_path,
            size,
            attributes,
        } = file;
        let key = (username.clone(), virtual_path.clone());
        let id = match self.transfers.get(&key) {
            Some(existing) if existing.phase.is_active() => {
                debug!(username, virtual_path, "download already in progress");
                return (EnqueueResult::AlreadyActive, Vec::new());
            }
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        let limit = self.basename_limit(&folder_path);
        let basename = files::download_basename(&virtual_path, limit);
        let downloaded = files::complete_file_path(&folder_path, &basename, size);
        let mut transfer = Transfer {
            id,
            username: username.clone(),
            virtual_path: virtual_path.clone(),
            folder_path,
            size,
            attributes,
            phase: TransferPhase::Queued,
            bytes_done: 0,
            file_path: None,
            queue_place: 0,
            speed_bps: 0,
            retry_attempt: false,
            legacy_attempt: false,
            size_changed: false,
            incomplete_path: None,
            activated_at: None,
            speed: SpeedMeter::default(),
        };
        if let Some(destination) = downloaded {
            info!(
                username,
                virtual_path,
                ?destination,
                "file is already downloaded"
            );
            transfer.phase = TransferPhase::Finished;
            transfer.bytes_done = size;
            transfer.file_path = Some(destination.display().to_string());
            let finished = TransferWork::Finished {
                snapshot: transfer.snapshot(),
                avg_speed_bps: None,
                delivered_bytes: 0,
            };
            self.transfers.insert(id, key, transfer);
            return (EnqueueResult::Enqueued, vec![finished]);
        }
        let queued = TransferWork::Update(transfer.snapshot());
        self.transfers.insert(id, key.clone(), transfer);
        self.net.server(ServerRequest::WatchUser {
            user: username.clone(),
        });
        self.send_queue_request(key);
        (EnqueueResult::Enqueued, vec![queued])
    }

    pub fn retry(
        &mut self,
        ids: &mut TransferIds,
        id: TransferId,
    ) -> (RetryResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (RetryResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get(&key).unwrap();
        if transfer.phase.is_active() {
            return (RetryResult::AlreadyActive, Vec::new());
        }
        let (username, folder_path, file) = (
            transfer.username.clone(),
            transfer.folder_path.clone(),
            FileInfo {
                name: transfer.virtual_path.clone(),
                size: transfer.size,
                attributes: transfer.attributes.clone(),
            },
        );
        let (result, work) = self.start(ids, username, file, folder_path);
        match result {
            EnqueueResult::Enqueued => (RetryResult::Requeued, work),
            EnqueueResult::AlreadyActive => {
                unreachable!("inactive transfer cannot collide with an active one")
            }
        }
    }

    pub fn abort(&mut self, id: TransferId) -> (AbortResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (AbortResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get_mut(&key).unwrap();
        if !transfer.phase.is_active() {
            return (AbortResult::Aborted, Vec::new());
        }
        transfer.phase = TransferPhase::Aborted;
        let aborted = TransferWork::Update(transfer.snapshot());
        self.release(&key);
        (AbortResult::Aborted, vec![aborted])
    }

    pub fn clear(&mut self, statuses: &[TransferStatus]) -> Vec<TransferId> {
        let removed: Vec<TransferId> = self
            .transfers
            .values()
            .filter(|transfer| statuses.contains(&transfer.phase.status()))
            .map(|transfer| transfer.id)
            .collect();
        for id in &removed {
            let (_, detached) = self.transfers.remove(*id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
        }
        removed
    }

    pub fn clear_all(&mut self) -> Vec<TransferId> {
        let removed: Vec<TransferId> = self
            .transfers
            .values()
            .map(|transfer| transfer.id)
            .collect();
        for id in &removed {
            let (_, detached) = self.transfers.remove(*id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
        }
        removed
    }

    pub fn request_queued(&mut self) {
        let mut watched = HashSet::new();
        let keys: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
            .map(Transfer::key)
            .collect();
        for key in keys {
            if watched.insert(key.0.clone()) {
                self.net.server(ServerRequest::WatchUser {
                    user: key.0.clone(),
                });
            }
            self.send_queue_request(key);
        }
    }

    fn send_queue_request(&mut self, key: TransferKey) {
        let transfer = self.transfers.get(&key).unwrap();
        let legacy_client = transfer.legacy_attempt;
        self.net.peer(
            key.0,
            PeerMessage::QueueUpload {
                file: key.1,
                legacy_client,
            },
        );
    }

    pub fn request_queue_positions(&mut self) {
        if self.queue_positions_at.elapsed() < QUEUE_POSITION_INTERVAL {
            return;
        }
        self.queue_positions_at = Instant::now();
        for transfer in self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
        {
            self.net.peer(
                transfer.username.clone(),
                PeerMessage::PlaceInQueueRequest {
                    file: transfer.virtual_path.clone(),
                    legacy_client: transfer.legacy_attempt,
                },
            );
        }
    }

    pub fn retry_offline(&mut self, username: &str) -> Vec<TransferWork> {
        let keys: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.username == username
                    && matches!(
                        &transfer.phase,
                        TransferPhase::Failed(reason) if reason == "user is offline"
                    )
            })
            .map(Transfer::key)
            .collect();
        let mut updates = Vec::with_capacity(keys.len());
        for key in keys {
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Queued;
            transfer.retry_attempt = false;
            updates.push(TransferWork::Update(transfer.snapshot()));
            self.send_queue_request(key);
        }
        updates
    }

    pub fn queue_place(
        &mut self,
        username: &str,
        virtual_path: &str,
        place: u32,
    ) -> Option<TransferWork> {
        let transfer = self
            .transfers
            .get_mut(&(username.to_owned(), virtual_path.to_owned()))?;
        transfer.queue_place = place;
        Some(TransferWork::Progress(transfer.snapshot()))
    }

    pub fn handle_transfer_request(
        &mut self,
        username: &str,
        token: u32,
        file: &str,
        filesize: Option<u64>,
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let token_taken = self
            .transfers
            .key_by_token(username, token)
            .is_some_and(|holder| *holder != key);
        if token_taken {
            debug!(
                username,
                token,
                virtual_path = file,
                "transfer request reuses a token held by another download"
            );
        }
        let mut updates = Vec::new();
        let response = match self.transfers.get_mut(&key) {
            Some(transfer)
                if !token_taken
                    && matches!(
                        transfer.phase,
                        TransferPhase::Queued
                            | TransferPhase::GettingStatus
                            | TransferPhase::Failed(_)
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
                transfer.phase = TransferPhase::GettingStatus;
                transfer.activated_at = Some(Instant::now());
                transfer.queue_place = 0;
                updates.push(TransferWork::Update(transfer.snapshot()));
                PeerMessage::TransferResponse {
                    token,
                    allowed: true,
                    reason: None,
                    filesize: None,
                }
            }
            Some(transfer) if transfer.phase == TransferPhase::Finished => {
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
        if !updates.is_empty() {
            self.transfers.attach_token(&key, token);
        }
        self.net.peer(username, response);
        updates
    }

    pub fn handle_file_transfer_init(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            debug!(
                username,
                token, "file transfer init with unknown token, closing"
            );
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        };
        if self.transfers.conn_of(&key).is_some() {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        }
        self.transfers.attach_conn(&key, conn_id);
        let incomplete_dir = self.incomplete_dir.clone();
        let limit = self.basename_limit(&incomplete_dir);
        let incomplete_path = files::incomplete_file_path(&incomplete_dir, username, &key.1, limit);
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.activated_at = None;
        transfer.incomplete_path = Some(incomplete_path.clone());

        let size_changed = transfer.size_changed;
        match files::open_incomplete(&incomplete_dir, &incomplete_path, size_changed) {
            Ok((file, offset)) => {
                transfer.retry_attempt = false;
                if transfer.size > offset {
                    transfer.phase = TransferPhase::Transferring;
                    transfer.bytes_done = offset;
                    transfer.queue_place = 0;
                    transfer.speed_bps = 0;
                    transfer.speed.reset();
                    let size = transfer.size;
                    let started = TransferWork::Update(transfer.snapshot());
                    info!(
                        username,
                        virtual_path = key.1,
                        offset,
                        size,
                        "download started"
                    );
                    self.net.send(NetworkCommand::DownloadFile {
                        conn_id,
                        file,
                        offset,
                        bytes_left: size - offset,
                    });
                    vec![started]
                } else {
                    self.net.send(NetworkCommand::CloseConnection(conn_id));
                    self.finish(&key)
                }
            }
            Err(error) => self.fail(&key, format!("local file error: {error}")),
        }
    }

    pub fn handle_download_progress(
        &mut self,
        conn_id: ConnId,
        bytes_left: u64,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned() else {
            return Vec::new();
        };
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.bytes_done = transfer.size.saturating_sub(bytes_left);
        if bytes_left == 0 {
            return self.finish(&key);
        }
        transfer.speed_bps = transfer.speed.sample(transfer.bytes_done);
        vec![TransferWork::Progress(transfer.snapshot())]
    }

    pub fn handle_transfer_error(&mut self, conn_id: ConnId, error: &str) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned() else {
            return Vec::new();
        };
        self.fail(&key, error.to_owned())
    }

    pub fn handle_file_connection_closed(&mut self, conn_id: ConnId) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned() else {
            return Vec::new();
        };
        self.transfers.detach_conn(&key);
        match self.transfers.get(&key).unwrap().phase {
            TransferPhase::Transferring => self.fail(&key, "connection closed".into()),
            _ => Vec::new(),
        }
    }

    pub fn handle_upload_denied(
        &mut self,
        username: &str,
        file: &str,
        reason: &str,
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        if transfer.phase != TransferPhase::Queued {
            return Vec::new();
        }
        if reason == TransferRejectReason::FILE_NOT_SHARED && !transfer.legacy_attempt {
            info!(
                username,
                virtual_path = file,
                "file not shared, retrying with latin-1 encoded path"
            );
            self.transfers.get_mut(&key).unwrap().legacy_attempt = true;
            self.send_queue_request(key);
            return Vec::new();
        }
        self.fail(&key, reason.to_owned())
    }

    pub fn handle_upload_failed(&mut self, username: &str, file: &str) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Placing | TransferPhase::Finished | TransferPhase::Aborted
        ) {
            return Vec::new();
        }
        if !transfer.retry_attempt {
            self.release(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.retry_attempt = true;
            transfer.legacy_attempt = true;
            transfer.size_changed = false;
            transfer.phase = TransferPhase::Queued;
            transfer.activated_at = None;
            let requeued = TransferWork::Update(transfer.snapshot());
            self.send_queue_request(key);
            return vec![requeued];
        }
        let updates = self.fail(&key, "upload failed".into());
        self.transfers.get_mut(&key).unwrap().retry_attempt = false;
        updates
    }

    pub fn handle_peer_connection_error(
        &mut self,
        username: &str,
        unsent: &[PeerMessage],
        is_offline: bool,
    ) -> Vec<TransferWork> {
        let reason = if is_offline {
            "user is offline"
        } else {
            "connection timeout"
        };
        let mut updates = Vec::new();
        for message in unsent {
            if let PeerMessage::QueueUpload { file, .. } = message {
                let key = (username.to_owned(), file.clone());
                if self
                    .transfers
                    .get(&key)
                    .is_some_and(|transfer| transfer.phase == TransferPhase::Queued)
                {
                    updates.extend(self.fail(&key, reason.into()));
                }
            }
        }
        updates
    }

    pub fn sweep_request_timeouts(&mut self) -> Vec<TransferWork> {
        let expired: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.phase == TransferPhase::GettingStatus
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

    pub fn reset(&mut self) -> Vec<TransferWork> {
        let active: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| {
                matches!(
                    transfer.phase,
                    TransferPhase::GettingStatus | TransferPhase::Transferring
                )
            })
            .map(Transfer::key)
            .collect();
        let mut updates = Vec::new();
        for key in active {
            self.transfers.detach(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Queued;
            transfer.activated_at = None;
            transfer.legacy_attempt = false;
            transfer.size_changed = false;
            updates.push(TransferWork::Update(transfer.snapshot()));
        }
        updates
    }

    fn finish(&mut self, key: &TransferKey) -> Vec<TransferWork> {
        let transfer = self.transfers.get(key).unwrap();
        let destination_dir = transfer.folder_path.clone();
        let virtual_path = transfer.virtual_path.clone();
        let limit = self.basename_limit(&destination_dir);
        let basename = files::download_basename(&virtual_path, limit);
        self.transfers.detach_token(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Placing;
        transfer.bytes_done = transfer.size;
        transfer.speed_bps = 0;
        let update = TransferWork::Update(transfer.snapshot());
        let incomplete_path = transfer
            .incomplete_path
            .clone()
            .expect("finishing a download that never opened its incomplete file");
        let placements = self.placements.clone();
        let key = key.clone();
        let task = tokio::task::spawn_blocking(move || {
            files::place_download(&destination_dir, &incomplete_path, &basename)
        });
        tokio::spawn(async move {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(std::io::Error::other(format!(
                    "placement task panicked: {error}"
                ))),
            };
            let _ = placements.send((key, result));
        });
        vec![update]
    }

    pub fn handle_placement_done(
        &mut self,
        key: &TransferKey,
        result: std::io::Result<PathBuf>,
    ) -> Vec<TransferWork> {
        let Some(transfer) = self.transfers.get(key) else {
            return Vec::new();
        };
        if transfer.phase != TransferPhase::Placing {
            debug!(
                username = key.0,
                virtual_path = key.1,
                phase = ?transfer.phase,
                "placement completed for a transfer that moved on"
            );
            return Vec::new();
        }
        match result {
            Ok(destination) => {
                let transfer = self.transfers.get_mut(key).unwrap();
                transfer.phase = TransferPhase::Finished;
                transfer.file_path = Some(destination.display().to_string());
                info!(
                    username = key.0,
                    virtual_path = key.1,
                    ?destination,
                    "download finished"
                );
                vec![TransferWork::Finished {
                    snapshot: transfer.snapshot(),
                    avg_speed_bps: None,
                    delivered_bytes: transfer.size,
                }]
            }
            Err(error) => self.fail(key, format!("cannot place finished download: {error}")),
        }
    }

    fn release(&mut self, key: &TransferKey) {
        if let Some(conn_id) = self.transfers.detach(key).conn_id {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
    }

    fn fail(&mut self, key: &TransferKey, reason: String) -> Vec<TransferWork> {
        let Some(transfer) = self.transfers.get(key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Finished | TransferPhase::Aborted | TransferPhase::Failed(_)
        ) {
            return Vec::new();
        }
        self.release(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(reason);
        transfer.legacy_attempt = false;
        transfer.size_changed = false;
        vec![TransferWork::Update(transfer.snapshot())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::spawn as spawn_network;

    fn queued_download(downloads: &mut Downloads, ids: &mut TransferIds) -> TransferKey {
        let (result, _) = downloads.enqueue(
            ids,
            "uploader".into(),
            FileInfo {
                name: "Music\\song.mp3".into(),
                size: 100,
                attributes: FileAttributes::default(),
            },
            None,
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        ("uploader".into(), "Music\\song.mp3".into())
    }

    #[tokio::test]
    async fn folder_downloads_keep_the_remote_hierarchy() {
        let (net, _events) = spawn_network();
        let mut downloads = Downloads::new(
            net,
            "/downloads".into(),
            "/incomplete".into(),
            true,
            mpsc::unbounded_channel().0,
        );
        let mut ids = TransferIds::new(&[]);
        let (result, _) = downloads.enqueue(
            &mut ids,
            "uploader".into(),
            FileInfo {
                name: "share\\Soulseek\\folder1\\sub1\\file4.mp3".into(),
                size: 100,
                attributes: FileAttributes::default(),
            },
            Some("share\\Soulseek"),
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        let key = (
            "uploader".to_owned(),
            "share\\Soulseek\\folder1\\sub1\\file4.mp3".to_owned(),
        );
        assert_eq!(
            downloads.transfers.get(&key).unwrap().folder_path,
            PathBuf::from("/downloads/uploader/Soulseek/folder1/sub1")
        );
    }

    #[tokio::test]
    async fn an_already_downloaded_file_finishes_without_asking_the_peer() {
        let dir = std::env::temp_dir().join(format!("newkitine-skip-{}", std::process::id()));
        let destination = dir.join("Album");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("song.mp3"), vec![0u8; 100]).unwrap();

        let (net, mut commands) = crate::network::test_channel();
        let mut downloads = Downloads::new(
            net,
            dir.clone(),
            "/incomplete".into(),
            false,
            mpsc::unbounded_channel().0,
        );
        let mut ids = TransferIds::new(&[]);
        let (result, work) = downloads.enqueue(
            &mut ids,
            "uploader".into(),
            FileInfo {
                name: "share\\Album\\song.mp3".into(),
                size: 100,
                attributes: FileAttributes::default(),
            },
            Some("share\\Album"),
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        assert!(matches!(
            work.as_slice(),
            [TransferWork::Finished {
                delivered_bytes: 0,
                ..
            }]
        ));
        let key = ("uploader".to_owned(), "share\\Album\\song.mp3".to_owned());
        let transfer = downloads.transfers.get(&key).unwrap();
        assert_eq!(transfer.phase, TransferPhase::Finished);
        assert_eq!(
            transfer.file_path.as_deref(),
            Some(destination.join("song.mp3").to_str().unwrap())
        );
        assert!(commands.try_recv().is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_size_mismatch_still_downloads() {
        let dir = std::env::temp_dir().join(format!("newkitine-mismatch-{}", std::process::id()));
        let destination = dir.join("Album");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("song.mp3"), vec![0u8; 64]).unwrap();

        let (net, _events) = spawn_network();
        let mut downloads = Downloads::new(
            net,
            dir.clone(),
            "/incomplete".into(),
            false,
            mpsc::unbounded_channel().0,
        );
        let mut ids = TransferIds::new(&[]);
        let (_, work) = downloads.enqueue(
            &mut ids,
            "uploader".into(),
            FileInfo {
                name: "share\\Album\\song.mp3".into(),
                size: 100,
                attributes: FileAttributes::default(),
            },
            Some("share\\Album"),
        );
        assert!(matches!(work.as_slice(), [TransferWork::Update(_)]));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn late_denial_does_not_revive_aborted_transfer() {
        let (net, _events) = spawn_network();
        let mut downloads = Downloads::new(
            net,
            "/tmp".into(),
            "/tmp".into(),
            false,
            mpsc::unbounded_channel().0,
        );
        let mut ids = TransferIds::new(&[]);
        let key = queued_download(&mut downloads, &mut ids);
        let id = downloads.transfers.get(&key).unwrap().id;
        let (aborted, _) = downloads.abort(id);
        assert_eq!(aborted, AbortResult::Aborted);
        let updates = downloads.handle_upload_denied(
            "uploader",
            "Music\\song.mp3",
            TransferRejectReason::FILE_NOT_SHARED,
        );
        assert!(updates.is_empty());
        let transfer = downloads.transfers.get(&key).unwrap();
        assert_eq!(transfer.phase, TransferPhase::Aborted);
        assert!(!transfer.legacy_attempt);
    }

    struct Harness {
        downloads: Downloads,
        commands: mpsc::Receiver<NetworkCommand>,
        ids: TransferIds,
        dir: PathBuf,
    }

    impl Harness {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("newkitine-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (net, commands) = crate::network::test_channel();
            Self {
                downloads: Downloads::new(
                    net,
                    dir.join("complete"),
                    dir.join("incomplete"),
                    false,
                    mpsc::unbounded_channel().0,
                ),
                commands,
                ids: TransferIds::new(&[]),
                dir,
            }
        }

        fn queue(&mut self, file: &str) -> TransferKey {
            let (result, _) = self.downloads.enqueue(
                &mut self.ids,
                "uploader".into(),
                FileInfo {
                    name: file.into(),
                    size: 100,
                    attributes: FileAttributes::default(),
                },
                None,
            );
            assert_eq!(result, EnqueueResult::Enqueued);
            ("uploader".into(), file.into())
        }

        fn start(&mut self, file: &str, token: u32, conn_id: ConnId) -> TransferKey {
            let key = (String::from("uploader"), String::from(file));
            let requested =
                self.downloads
                    .handle_transfer_request("uploader", token, file, Some(100));
            assert_eq!(requested.len(), 1);
            let started = self
                .downloads
                .handle_file_transfer_init("uploader", token, conn_id);
            assert_eq!(started.len(), 1);
            assert_eq!(self.phase(&key), TransferPhase::Transferring);
            key
        }

        fn phase(&self, key: &TransferKey) -> TransferPhase {
            self.downloads.transfers.get(key).unwrap().phase.clone()
        }

        fn drain(&mut self) -> Vec<NetworkCommand> {
            let mut commands = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                commands.push(command);
            }
            commands
        }

        fn closed(&mut self, conn_id: ConnId) -> bool {
            self.drain().iter().any(
                |command| matches!(command, NetworkCommand::CloseConnection(id) if *id == conn_id),
            )
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn statuses(work: &[TransferWork]) -> Vec<TransferStatus> {
        work.iter()
            .map(|item| match item {
                TransferWork::Update(snapshot) => snapshot.status,
                other => panic!("expected an update, got {other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_token_held_by_another_download_is_refused() {
        let mut harness = Harness::new("token-reuse");
        let first = harness.queue("Music\\a.mp3");
        let second = harness.queue("Music\\b.mp3");
        harness.start("Music\\a.mp3", 7, 1);
        harness.drain();

        let work =
            harness
                .downloads
                .handle_transfer_request("uploader", 7, "Music\\b.mp3", Some(100));
        assert!(work.is_empty());
        assert_eq!(harness.phase(&second), TransferPhase::Queued);
        assert!(harness.drain().iter().any(|command| matches!(
            command,
            NetworkCommand::SendPeerMessage {
                message: PeerMessage::TransferResponse {
                    token: 7,
                    allowed: false,
                    ..
                },
                ..
            }
        )));

        let work = harness.downloads.handle_download_progress(1, 0);
        assert!(matches!(work.as_slice(), [TransferWork::Update(_)]));
        assert_eq!(harness.phase(&first), TransferPhase::Placing);
    }

    #[tokio::test]
    async fn progress_from_a_superseded_connection_is_ignored() {
        let mut harness = Harness::new("stale-conn");
        let key = harness.queue("Music\\a.mp3");
        harness.start("Music\\a.mp3", 7, 1);
        let work = harness
            .downloads
            .handle_upload_failed("uploader", "Music\\a.mp3");
        assert_eq!(statuses(&work), vec![TransferStatus::Queued]);
        assert!(harness.closed(1));

        harness.start("Music\\a.mp3", 7, 2);
        assert!(harness.downloads.handle_download_progress(1, 0).is_empty());
        assert!(
            harness
                .downloads
                .handle_transfer_error(1, "stale")
                .is_empty()
        );
        assert!(
            harness
                .downloads
                .handle_file_connection_closed(1)
                .is_empty()
        );
        assert_eq!(harness.phase(&key), TransferPhase::Transferring);
    }

    #[tokio::test]
    async fn failing_a_transferring_download_closes_its_connection() {
        let mut harness = Harness::new("fail-close");
        let key = harness.queue("Music\\a.mp3");
        harness.start("Music\\a.mp3", 7, 1);
        harness.drain();

        let work = harness.downloads.handle_transfer_error(1, "disk full");
        assert_eq!(statuses(&work), vec![TransferStatus::Failed]);
        assert!(harness.closed(1));
        assert!(harness.downloads.transfers.key_by_conn(1).is_none());
        assert_eq!(
            harness.phase(&key),
            TransferPhase::Failed("disk full".into())
        );
    }

    #[tokio::test]
    async fn denials_and_connection_errors_only_touch_queued_downloads() {
        let mut harness = Harness::new("queued-only");
        let key = harness.queue("Music\\a.mp3");
        let requested =
            harness
                .downloads
                .handle_transfer_request("uploader", 7, "Music\\a.mp3", Some(100));
        assert_eq!(statuses(&requested), vec![TransferStatus::Queued]);

        let denied = harness.downloads.handle_upload_denied(
            "uploader",
            "Music\\a.mp3",
            TransferRejectReason::TOO_MANY_FILES,
        );
        assert!(denied.is_empty());
        let errored = harness.downloads.handle_peer_connection_error(
            "uploader",
            &[PeerMessage::QueueUpload {
                file: "Music\\a.mp3".into(),
                legacy_client: false,
            }],
            true,
        );
        assert!(errored.is_empty());
        assert_eq!(harness.phase(&key), TransferPhase::GettingStatus);

        harness.start("Music\\a.mp3", 7, 1);
        let placing = harness.downloads.handle_download_progress(1, 0);
        assert_eq!(placing.len(), 1);
        assert!(
            harness
                .downloads
                .handle_upload_failed("uploader", "Music\\a.mp3")
                .is_empty()
        );
        assert_eq!(harness.phase(&key), TransferPhase::Placing);
    }

    #[tokio::test]
    async fn every_requeue_is_published() {
        let mut harness = Harness::new("requeue-updates");
        let key = harness.queue("Music\\a.mp3");
        let failed = harness.downloads.handle_peer_connection_error(
            "uploader",
            &[PeerMessage::QueueUpload {
                file: "Music\\a.mp3".into(),
                legacy_client: false,
            }],
            true,
        );
        assert_eq!(statuses(&failed), vec![TransferStatus::Failed]);

        let requeued = harness.downloads.retry_offline("uploader");
        assert_eq!(statuses(&requeued), vec![TransferStatus::Queued]);

        let failed = harness.downloads.handle_upload_denied(
            "uploader",
            "Music\\a.mp3",
            TransferRejectReason::CANCELLED,
        );
        assert_eq!(statuses(&failed), vec![TransferStatus::Failed]);
        let revived =
            harness
                .downloads
                .handle_transfer_request("uploader", 9, "Music\\a.mp3", Some(100));
        assert_eq!(statuses(&revived), vec![TransferStatus::Queued]);
        assert_eq!(harness.phase(&key), TransferPhase::GettingStatus);
    }

    #[tokio::test]
    async fn aborting_a_session_clears_size_changed_and_starting_clears_retry() {
        let mut harness = Harness::new("flag-resets");
        let key = harness.queue("Music\\a.mp3");
        let _ = harness
            .downloads
            .handle_transfer_request("uploader", 7, "Music\\a.mp3", Some(250));
        assert!(harness.downloads.transfers.get(&key).unwrap().size_changed);
        let _ = harness
            .downloads
            .handle_file_transfer_init("uploader", 7, 1);
        let _ = harness.downloads.handle_transfer_error(1, "reset by peer");
        assert!(!harness.downloads.transfers.get(&key).unwrap().size_changed);

        let _ = harness
            .downloads
            .handle_transfer_request("uploader", 6, "Music\\a.mp3", Some(400));
        assert!(harness.downloads.transfers.get(&key).unwrap().size_changed);
        let reset = harness.downloads.reset();
        assert_eq!(statuses(&reset), vec![TransferStatus::Queued]);
        assert!(!harness.downloads.transfers.get(&key).unwrap().size_changed);

        let _ = harness
            .downloads
            .handle_upload_failed("uploader", "Music\\a.mp3");
        let transfer = harness.downloads.transfers.get(&key).unwrap();
        assert!(transfer.retry_attempt);
        assert_eq!(transfer.phase, TransferPhase::Queued);
        let _ = harness
            .downloads
            .handle_transfer_request("uploader", 8, "Music\\a.mp3", Some(250));
        let _ = harness
            .downloads
            .handle_file_transfer_init("uploader", 8, 2);
        assert!(!harness.downloads.transfers.get(&key).unwrap().retry_attempt);
    }
}
