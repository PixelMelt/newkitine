use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::files;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use crate::network::NetworkHandle;
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::{
    AbortResult, ConnId, EnqueueResult, FileAttributes, NetworkCommand, RetryResult,
    TransferDirection, TransferId, TransferRejectReason, TransferSnapshot, TransferStatus,
    TransferWork,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

pub struct TransferIds {
    next: u64,
}

impl TransferIds {
    pub fn new(seeds: &[TransferSnapshot]) -> Self {
        Self {
            next: seeds.iter().map(|seed| seed.id.0).max().unwrap_or(0) + 1,
        }
    }

    pub fn mint(&mut self) -> TransferId {
        let id = TransferId(self.next);
        self.next += 1;
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferPhase {
    Queued,
    GettingStatus,
    Transferring,
    Finished,
    Aborted,
    Failed(String),
}

impl TransferPhase {
    pub fn status(&self) -> TransferStatus {
        match self {
            Self::Queued | Self::GettingStatus => TransferStatus::Queued,
            Self::Transferring => TransferStatus::Transferring,
            Self::Finished => TransferStatus::Finished,
            Self::Aborted => TransferStatus::Aborted,
            Self::Failed(_) => TransferStatus::Failed,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Queued | Self::GettingStatus | Self::Transferring
        )
    }

    pub fn from_seed(seed: &TransferSnapshot) -> Self {
        match seed.status {
            TransferStatus::Queued => Self::Queued,
            TransferStatus::Transferring => {
                unreachable!("bootstrap normalizes transferring transfers before seeding")
            }
            TransferStatus::Finished => Self::Finished,
            TransferStatus::Aborted => Self::Aborted,
            TransferStatus::Failed => Self::Failed(seed.failure_reason.clone().unwrap_or_default()),
        }
    }
}

#[derive(Debug)]
pub struct Transfer {
    pub id: TransferId,
    pub username: String,
    pub virtual_path: String,
    pub size: u64,
    pub attributes: FileAttributes,
    pub phase: TransferPhase,
    pub bytes_done: u64,
    pub file_path: Option<String>,
    pub queue_place: u32,
    pub speed_bps: u32,
    pub retry_attempt: bool,
    pub size_changed: bool,
    pub activated_at: Option<Instant>,
    pub(crate) speed: SpeedMeter,
}

impl Transfer {
    pub fn key(&self) -> TransferKey {
        (self.username.clone(), self.virtual_path.clone())
    }

    pub fn basename(&self) -> String {
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

pub struct Downloads {
    net: NetworkHandle,
    download_dir: PathBuf,
    incomplete_dir: PathBuf,
    transfers: Registry<Transfer>,
}

impl Downloads {
    pub fn new(net: NetworkHandle, download_dir: PathBuf, incomplete_dir: PathBuf) -> Self {
        Self {
            net,
            download_dir,
            incomplete_dir,
            transfers: Registry::default(),
        }
    }

    pub fn set_dirs(&mut self, download_dir: PathBuf, incomplete_dir: PathBuf) {
        self.download_dir = download_dir;
        self.incomplete_dir = incomplete_dir;
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
                size_changed: false,
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
            size_changed: false,
            activated_at: None,
            speed: SpeedMeter::default(),
        };
        let queued = TransferWork::Update(transfer.snapshot());
        self.transfers.insert(id, key, transfer);
        self.net.server(ServerRequest::WatchUser {
            user: username.clone(),
        });
        self.net.peer(
            username,
            PeerMessage::QueueUpload {
                file: virtual_path,
                legacy_client: false,
            },
        );
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
        let (username, virtual_path, size, attributes) = (
            transfer.username.clone(),
            transfer.virtual_path.clone(),
            transfer.size,
            transfer.attributes.clone(),
        );
        let (result, work) = self.enqueue(ids, username, virtual_path, size, attributes);
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

    pub fn request_queued(&self) {
        let mut watched = HashSet::new();
        for transfer in self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
        {
            if watched.insert(transfer.username.clone()) {
                self.net.server(ServerRequest::WatchUser {
                    user: transfer.username.clone(),
                });
            }
            self.net.peer(
                transfer.username.clone(),
                PeerMessage::QueueUpload {
                    file: transfer.virtual_path.clone(),
                    legacy_client: false,
                },
            );
        }
    }

    pub fn retry_offline(&mut self, username: &str) {
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
            self.net.peer(
                key.0,
                PeerMessage::QueueUpload {
                    file: key.1,
                    legacy_client: false,
                },
            );
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
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        match self.transfers.get(&key) {
            Some(transfer) if transfer.phase != TransferPhase::Finished => {
                self.fail(&key, reason.to_owned())
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_upload_failed(&mut self, username: &str, file: &str) -> Vec<TransferWork> {
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
            transfer.phase = TransferPhase::Queued;
            transfer.activated_at = None;
            let username = transfer.username.clone();
            let file = transfer.virtual_path.clone();
            self.net.peer(
                username,
                PeerMessage::QueueUpload {
                    file,
                    legacy_client: false,
                },
            );
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
        let transfer = self.transfers.get(key).unwrap();
        let username = transfer.username.clone();
        let virtual_path = transfer.virtual_path.clone();
        let basename = transfer.basename();
        let incomplete_path =
            files::incomplete_file_path(&self.incomplete_dir, &username, &virtual_path);

        match files::place_download(&self.download_dir, &incomplete_path, &basename) {
            Ok(destination) => {
                self.transfers.detach_token(key);
                let transfer = self.transfers.get_mut(key).unwrap();
                transfer.phase = TransferPhase::Finished;
                transfer.bytes_done = transfer.size;
                transfer.file_path = Some(destination.display().to_string());
                info!(username, virtual_path, ?destination, "download finished");
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
