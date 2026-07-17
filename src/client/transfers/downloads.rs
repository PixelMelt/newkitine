use std::collections::HashSet;
use std::path::PathBuf;
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
    FileAttributes, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const QUEUE_POSITION_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug)]
struct Transfer {
    id: TransferId,
    username: String,
    virtual_path: String,
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

    fn basename(&self) -> String {
        self.virtual_path
            .rsplit('\\')
            .next()
            .unwrap_or(&self.virtual_path)
            .to_owned()
    }

    fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            id: self.id,
            direction: TransferDirection::Download,
            username: self.username.clone(),
            virtual_path: self.virtual_path.clone(),
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
    pending_queue_requests: Vec<TransferKey>,
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
            pending_queue_requests: Vec::new(),
            queue_positions_at: Instant::now(),
        }
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
    }

    pub fn seed(&mut self, seed: TransferSnapshot) {
        let phase = TransferPhase::from_seed(&seed);
        let key = (seed.username.clone(), seed.virtual_path.clone());
        self.transfers.insert(
            seed.id,
            key,
            Transfer {
                id: seed.id,
                username: seed.username,
                virtual_path: seed.virtual_path,
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
        virtual_path: String,
        size: u64,
        attributes: FileAttributes,
        defer_requests: bool,
    ) -> (EnqueueResult, Vec<TransferWork>) {
        let key = (username.clone(), virtual_path.clone());
        let id = match self.transfers.get(&key) {
            Some(existing) if existing.phase.is_active() => {
                debug!(username, virtual_path, "download already in progress");
                return (EnqueueResult::AlreadyActive, Vec::new());
            }
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        let transfer = Transfer {
            id,
            username: username.clone(),
            virtual_path: virtual_path.clone(),
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
        let queued = TransferWork::Update(transfer.snapshot());
        self.transfers.insert(id, key.clone(), transfer);
        self.net.server(ServerRequest::WatchUser {
            user: username.clone(),
        });
        self.send_queue_request(key, defer_requests);
        (EnqueueResult::Enqueued, vec![queued])
    }

    pub fn retry(
        &mut self,
        ids: &mut TransferIds,
        id: TransferId,
        defer_requests: bool,
    ) -> (RetryResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (RetryResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get(&key).unwrap();
        if transfer.phase.is_active() {
            return (RetryResult::AlreadyActive, Vec::new());
        }
        let (username, virtual_path, size, attributes) = (
            transfer.username.clone(),
            transfer.virtual_path.clone(),
            transfer.size,
            transfer.attributes.clone(),
        );
        let (result, work) = self.enqueue(
            ids,
            username,
            virtual_path,
            size,
            attributes,
            defer_requests,
        );
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
        let detached = self.transfers.detach(&key);
        if let Some(conn_id) = detached.conn_id {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
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

    pub fn request_queued(&mut self, defer_requests: bool) {
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
            self.send_queue_request(key, defer_requests);
        }
    }

    fn send_queue_request(&mut self, key: TransferKey, defer_requests: bool) {
        if defer_requests {
            if !self.pending_queue_requests.contains(&key) {
                self.pending_queue_requests.push(key);
            }
            return;
        }
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

    pub fn flush_queued_requests(&mut self) {
        for key in std::mem::take(&mut self.pending_queue_requests) {
            let Some(transfer) = self.transfers.get(&key) else {
                continue;
            };
            if transfer.phase != TransferPhase::Queued {
                continue;
            }
            self.net.peer(
                key.0,
                PeerMessage::QueueUpload {
                    file: key.1,
                    legacy_client: transfer.legacy_attempt,
                },
            );
        }
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

    pub fn retry_offline(&mut self, username: &str, defer_requests: bool) {
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
        for key in keys {
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Queued;
            transfer.retry_attempt = false;
            self.send_queue_request(key, defer_requests);
        }
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

    pub fn owns_token(&self, username: &str, token: u32) -> bool {
        self.transfers.owns_token(username, token)
    }

    pub fn handle_transfer_request(
        &mut self,
        username: &str,
        token: u32,
        file: &str,
        filesize: Option<u64>,
    ) {
        let key = (username.to_owned(), file.to_owned());
        let mut accepted = false;
        let response = match self.transfers.get_mut(&key) {
            Some(transfer)
                if matches!(
                    transfer.phase,
                    TransferPhase::Queued | TransferPhase::GettingStatus | TransferPhase::Failed(_)
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
                accepted = true;
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
        if accepted {
            self.transfers.attach_token(&key, token);
        }
        self.net.peer(username, response);
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
        let incomplete_path = files::incomplete_file_path(&self.incomplete_dir, username, &key.1);
        let incomplete_dir = self.incomplete_dir.clone();
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.activated_at = None;
        transfer.incomplete_path = Some(incomplete_path.clone());

        let size_changed = transfer.size_changed;
        match files::open_incomplete(&incomplete_dir, &incomplete_path, size_changed) {
            Ok((file, offset)) => {
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
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
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

    pub fn handle_transfer_error(
        &mut self,
        username: &str,
        token: u32,
        error: &str,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            return Vec::new();
        };
        self.fail(&key, error.to_owned())
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: Option<u32>,
        conn_id: ConnId,
    ) -> Vec<TransferWork> {
        let key = match self.transfers.key_by_conn(conn_id).cloned().or_else(|| {
            token.and_then(|token| self.transfers.key_by_token(username, token).cloned())
        }) {
            Some(key) => key,
            None => return Vec::new(),
        };
        self.transfers.detach_conn(&key);
        let transfer = self.transfers.get(&key).unwrap();
        match transfer.phase {
            TransferPhase::Transferring => self.fail(&key, "connection closed".into()),
            _ => {
                self.transfers.detach_token(&key);
                Vec::new()
            }
        }
    }

    pub fn handle_upload_denied(
        &mut self,
        username: &str,
        file: &str,
        reason: &str,
        defer_requests: bool,
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        if transfer.phase == TransferPhase::Finished {
            return Vec::new();
        }
        if reason == TransferRejectReason::FILE_NOT_SHARED
            && transfer.phase == TransferPhase::Queued
            && !transfer.legacy_attempt
        {
            info!(
                username,
                virtual_path = file,
                "file not shared, retrying with latin-1 encoded path"
            );
            self.transfers.detach(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.legacy_attempt = true;
            transfer.activated_at = None;
            self.send_queue_request(key, defer_requests);
            return Vec::new();
        }
        self.fail(&key, reason.to_owned())
    }

    pub fn handle_upload_failed(
        &mut self,
        username: &str,
        file: &str,
        defer_requests: bool,
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Finished | TransferPhase::Aborted
        ) {
            return Vec::new();
        }
        if !transfer.retry_attempt {
            self.transfers.detach(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.retry_attempt = true;
            transfer.legacy_attempt = true;
            transfer.phase = TransferPhase::Queued;
            transfer.activated_at = None;
            self.send_queue_request(key, defer_requests);
            return Vec::new();
        }
        self.fail(&key, "upload failed".into())
    }

    pub fn handle_peer_connection_error(
        &mut self,
        username: &str,
        unsent: &[PeerMessage],
        is_offline: bool,
    ) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        for message in unsent {
            if let PeerMessage::QueueUpload { file, .. } = message {
                let key = (username.to_owned(), file.clone());
                let reason = if is_offline {
                    "user is offline"
                } else {
                    "connection timeout"
                };
                updates.extend(self.fail(&key, reason.into()));
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
            updates.push(TransferWork::Update(transfer.snapshot()));
        }
        updates
    }

    fn finish(&mut self, key: &TransferKey) -> Vec<TransferWork> {
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Placing;
        transfer.bytes_done = transfer.size;
        transfer.speed_bps = 0;
        let update = TransferWork::Update(transfer.snapshot());
        let username = transfer.username.clone();
        let basename = transfer.basename();
        let incomplete_path = transfer
            .incomplete_path
            .clone()
            .expect("finishing a download that never opened its incomplete file");
        let download_dir = if self.username_subfolders {
            self.download_dir.join(files::clean_file_name(&username))
        } else {
            self.download_dir.clone()
        };
        let placements = self.placements.clone();
        let key = key.clone();
        let task = tokio::task::spawn_blocking(move || {
            files::place_download(&download_dir, &incomplete_path, &basename)
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
                self.transfers.detach_token(key);
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
                }]
            }
            Err(error) => self.fail(key, format!("cannot place finished download: {error}")),
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
        self.transfers.detach(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(reason);
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
            "Music\\song.mp3".into(),
            100,
            FileAttributes::default(),
            false,
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        ("uploader".into(), "Music\\song.mp3".into())
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
            false,
        );
        assert!(updates.is_empty());
        let transfer = downloads.transfers.get(&key).unwrap();
        assert_eq!(transfer.phase, TransferPhase::Aborted);
        assert!(!transfer.legacy_attempt);
    }

    #[tokio::test]
    async fn legacy_retry_defers_while_awaiting_share_index() {
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
        let updates = downloads.handle_upload_denied(
            "uploader",
            "Music\\song.mp3",
            TransferRejectReason::FILE_NOT_SHARED,
            true,
        );
        assert!(updates.is_empty());
        assert!(downloads.pending_queue_requests.contains(&key));
        let transfer = downloads.transfers.get(&key).unwrap();
        assert!(transfer.legacy_attempt);
        assert_eq!(transfer.phase, TransferPhase::Queued);
    }

    #[tokio::test]
    async fn flush_skips_transfers_no_longer_queued() {
        let (net, mut commands) = crate::network::test_channel();
        let mut downloads = Downloads::new(
            net,
            "/tmp".into(),
            "/tmp".into(),
            false,
            mpsc::unbounded_channel().0,
        );
        let mut ids = TransferIds::new(&[]);
        let (result, _) = downloads.enqueue(
            &mut ids,
            "uploader".into(),
            "Music\\song.mp3".into(),
            100,
            FileAttributes::default(),
            true,
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        let key = ("uploader".to_owned(), "Music\\song.mp3".to_owned());
        assert!(downloads.pending_queue_requests.contains(&key));
        let id = downloads.transfers.get(&key).unwrap().id;
        let (aborted, _) = downloads.abort(id);
        assert_eq!(aborted, AbortResult::Aborted);
        downloads.flush_queued_requests();
        assert!(downloads.pending_queue_requests.is_empty());
        let transfer = downloads.transfers.get(&key).unwrap();
        assert_eq!(transfer.phase, TransferPhase::Aborted);
        while let Ok(command) = commands.try_recv() {
            assert!(
                !matches!(
                    command,
                    NetworkCommand::SendPeerMessage {
                        message: PeerMessage::QueueUpload { .. },
                        ..
                    }
                ),
                "flush must not send a queue request for a non-queued transfer"
            );
        }
    }
}
