use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::queue::UploadQueue;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use super::{TransferIds, TransferPhase, TransferRejectReason};
use crate::client::shares::SharesIndex;
use crate::client::users::Users;
use crate::client::{AbortResult, TransferWork};
use crate::network::ConnId;
use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::{PeerMessage, ServerRequest, increment_token, initial_token};
use crate::types::{
    FileAttributes, Restriction, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const TIMED_OUT_RETRY_INTERVAL: Duration = Duration::from_secs(180);
const CONNECTION_TIMEOUT: &str = "connection timeout";
const FILE_READ_ERROR: &str = "File read error.";

enum Admission {
    Accept(PathBuf, u64, FileAttributes),
    AlreadyQueued,
}

#[derive(Debug)]
struct UploadTransfer {
    id: TransferId,
    username: String,
    virtual_path: String,
    real_path: Option<PathBuf>,
    size: u64,
    attributes: FileAttributes,
    phase: TransferPhase,
    bytes_done: u64,
    speed_bps: u32,
    started_offset: u64,
    activated_at: Option<Instant>,
    started_at: Option<Instant>,
    speed: SpeedMeter,
}

impl UploadTransfer {
    fn key(&self) -> TransferKey {
        (self.username.clone(), self.virtual_path.clone())
    }

    fn is_timed_out(&self) -> bool {
        matches!(&self.phase, TransferPhase::Failed(reason) if reason == CONNECTION_TIMEOUT)
    }

    fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            id: self.id,
            direction: TransferDirection::Upload,
            username: self.username.clone(),
            virtual_path: self.virtual_path.clone(),
            folder_path: None,
            size: self.size,
            bytes_done: self.bytes_done,
            status: self.phase.status(),
            failure_reason: match &self.phase {
                TransferPhase::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
            file_path: self
                .real_path
                .as_ref()
                .map(|path| path.display().to_string()),
            queue_place: 0,
            speed_bps: if self.phase == TransferPhase::Transferring {
                self.speed_bps
            } else {
                0
            },
            attributes: self.attributes.clone(),
        }
    }
}

pub(in crate::client) struct Uploads {
    net: NetworkHandle,
    upload_slots: usize,
    queue_file_limit: usize,
    queue_size_limit_mb: u64,
    banned_message: String,
    transfers: Registry<UploadTransfer>,
    queue: UploadQueue,
    token: u32,
    retried_at: Instant,
    pub upload_speed: u32,
}

impl Uploads {
    pub fn new(
        net: NetworkHandle,
        upload_slots: usize,
        queue_file_limit: usize,
        queue_size_limit_mb: u64,
        banned_message: String,
    ) -> Self {
        Self {
            net,
            upload_slots,
            queue_file_limit,
            queue_size_limit_mb,
            banned_message,
            transfers: Registry::default(),
            queue: UploadQueue::new(),
            token: initial_token(),
            retried_at: Instant::now(),
            upload_speed: 0,
        }
    }

    pub fn total_slots(&self) -> u32 {
        self.upload_slots as u32
    }

    pub fn set_limits(
        &mut self,
        upload_slots: usize,
        queue_file_limit: usize,
        queue_size_limit_mb: u64,
        banned_message: String,
    ) {
        self.upload_slots = upload_slots;
        self.queue_file_limit = queue_file_limit;
        self.queue_size_limit_mb = queue_size_limit_mb;
        self.banned_message = banned_message;
    }

    pub fn is_new_upload_accepted(&self) -> bool {
        self.free_slots() > 0
    }

    pub fn free_slots(&self) -> u32 {
        self.upload_slots
            .saturating_sub(self.queue.active_user_count()) as u32
    }

    pub fn queue_size(&self) -> u32 {
        self.queue.len() as u32
    }

    pub fn queued_for(&self, username: &str) -> u32 {
        self.queue.queued_for(username) as u32
    }

    pub fn active_uploads(&self, username: &str) -> u32 {
        u32::from(self.queue.is_active(username))
    }

    pub fn owns_token(&self, username: &str, token: u32) -> bool {
        self.transfers.owns_token(username, token)
    }

    pub fn seed(&mut self, seed: TransferSnapshot) {
        let phase = TransferPhase::from_seed(&seed);
        let key = (seed.username.clone(), seed.virtual_path.clone());
        self.transfers.insert(
            seed.id,
            key,
            UploadTransfer {
                id: seed.id,
                username: seed.username,
                virtual_path: seed.virtual_path,
                real_path: seed.file_path.map(PathBuf::from),
                size: seed.size,
                attributes: seed.attributes,
                phase,
                bytes_done: seed.bytes_done,
                speed_bps: 0,
                started_offset: 0,
                activated_at: None,
                started_at: None,
                speed: SpeedMeter::default(),
            },
        );
    }

    pub fn handle_queue_upload(
        &mut self,
        ids: &mut TransferIds,
        username: &str,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> (Vec<TransferWork>, bool) {
        match self.admit(username, virtual_path, shares, users) {
            Ok(Admission::Accept(real_path, size, attributes)) => {
                let mut updates =
                    self.enqueue(ids, username, virtual_path, real_path, size, attributes);
                updates.extend(self.check_queue(users));
                (updates, true)
            }
            Ok(Admission::AlreadyQueued) => (Vec::new(), true),
            Err(reason) => {
                self.net.peer(
                    username,
                    PeerMessage::UploadDenied {
                        file: virtual_path.to_owned(),
                        reason,
                    },
                );
                (Vec::new(), false)
            }
        }
    }

    pub fn handle_legacy_transfer_request(
        &mut self,
        ids: &mut TransferIds,
        username: &str,
        token: u32,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> (Vec<TransferWork>, bool) {
        let admission = self.admit(username, virtual_path, shares, users);
        let reason = match &admission {
            Ok(_) => TransferRejectReason::QUEUED.to_owned(),
            Err(reason) => reason.clone(),
        };
        self.net.peer(
            username,
            PeerMessage::TransferResponse {
                token,
                allowed: false,
                reason: Some(reason),
                filesize: None,
            },
        );
        match admission {
            Ok(Admission::Accept(real_path, size, attributes)) => {
                let mut updates =
                    self.enqueue(ids, username, virtual_path, real_path, size, attributes);
                updates.extend(self.check_queue(users));
                (updates, true)
            }
            Ok(Admission::AlreadyQueued) => (Vec::new(), true),
            Err(_) => (Vec::new(), false),
        }
    }

    fn refusal(&self, username: &str, virtual_path: &str, users: &Users) -> Option<String> {
        if users.is_banned(username) {
            return Some(self.banned_message.clone());
        }
        if let Some(Restriction::Denied { reason }) = users.restriction(username) {
            return Some(reason.clone());
        }
        if users.is_file_denied(username, virtual_path) {
            return Some(TransferRejectReason::REPEATED.into());
        }
        None
    }

    fn admit(
        &self,
        username: &str,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> Result<Admission, String> {
        if let Some(reason) = self.refusal(username, virtual_path, users) {
            return Err(reason);
        }
        let key = (username.to_owned(), virtual_path.to_owned());
        if self
            .transfers
            .get(&key)
            .is_some_and(|transfer| transfer.phase.is_active())
        {
            debug!(username, virtual_path, "upload already queued");
            return Ok(Admission::AlreadyQueued);
        }
        let (real_path, size, attributes) = shares
            .and_then(|shares| shares.resolve(virtual_path, users.is_buddy(username)))
            .ok_or(TransferRejectReason::FILE_NOT_SHARED)?;
        if self.queue.queued_for(username) >= self.queue_file_limit {
            return Err(TransferRejectReason::TOO_MANY_FILES.into());
        }
        if self.queue_size_limit_mb > 0
            && self.queued_bytes_for(username) >= self.queue_size_limit_mb * 1024 * 1024
        {
            return Err(TransferRejectReason::TOO_MANY_MEGABYTES.into());
        }
        Ok(Admission::Accept(real_path, size, attributes.clone()))
    }

    fn queued_bytes_for(&self, username: &str) -> u64 {
        self.transfers
            .values()
            .filter(|transfer| {
                transfer.username == username && transfer.phase == TransferPhase::Queued
            })
            .map(|transfer| transfer.size)
            .sum()
    }

    fn enqueue(
        &mut self,
        ids: &mut TransferIds,
        username: &str,
        virtual_path: &str,
        real_path: PathBuf,
        size: u64,
        attributes: FileAttributes,
    ) -> Vec<TransferWork> {
        let key = (username.to_owned(), virtual_path.to_owned());
        let id = match self.transfers.get(&key) {
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        let transfer = UploadTransfer {
            id,
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
            real_path: Some(real_path),
            size,
            attributes,
            phase: TransferPhase::Queued,
            bytes_done: 0,
            speed_bps: 0,
            started_offset: 0,
            activated_at: None,
            started_at: None,
            speed: SpeedMeter::default(),
        };
        let queued = TransferWork::Update(transfer.snapshot());
        self.transfers.insert(id, key.clone(), transfer);
        self.queue.push(key);
        vec![queued]
    }

    pub fn check_queue(&mut self, users: &Users) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        while self.is_new_upload_accepted() {
            let Some(key) = self.queue.select_next(users) else {
                break;
            };
            updates.extend(self.activate(&key));
        }
        updates
    }

    pub fn ban(&mut self, username: &str, users: &Users) -> Vec<TransferWork> {
        let reason = self.banned_message.clone();
        let mut updates = self.deny_all(username, &reason, users);
        let retryable: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| transfer.username == username && transfer.is_timed_out())
            .map(UploadTransfer::key)
            .collect();
        for key in &retryable {
            let transfer = self.transfers.get_mut(key).unwrap();
            transfer.phase = TransferPhase::Failed(reason.clone());
            updates.push(TransferWork::Update(transfer.snapshot()));
        }
        updates
    }

    pub fn deny_all(&mut self, username: &str, reason: &str, users: &Users) -> Vec<TransferWork> {
        let pending: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| transfer.username == username && transfer.phase.is_active())
            .map(UploadTransfer::key)
            .collect();
        self.deny_each(&pending, reason, users)
    }

    pub fn deny_file(&mut self, key: &TransferKey, users: &Users) -> Vec<TransferWork> {
        let active = self
            .transfers
            .get(key)
            .is_some_and(|transfer| transfer.phase.is_active());
        if !active {
            return Vec::new();
        }
        self.deny_each(
            std::slice::from_ref(key),
            TransferRejectReason::REPEATED,
            users,
        )
    }

    pub fn revalidate(&mut self, shares: &SharesIndex, users: &Users) -> Vec<TransferWork> {
        let unshared: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.phase.is_active()
                    && shares
                        .resolve(&transfer.virtual_path, users.is_buddy(&transfer.username))
                        .is_none_or(|(real_path, _, _)| {
                            transfer.real_path.as_ref() != Some(&real_path)
                        })
            })
            .map(UploadTransfer::key)
            .collect();
        self.deny_each(&unshared, TransferRejectReason::FILE_NOT_SHARED, users)
    }

    fn deny_each(
        &mut self,
        keys: &[TransferKey],
        reason: &str,
        users: &Users,
    ) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        for key in keys {
            self.net.peer(
                key.0.clone(),
                PeerMessage::UploadDenied {
                    file: key.1.clone(),
                    reason: reason.to_owned(),
                },
            );
            updates.extend(self.fail(key, reason.to_owned()));
        }
        if !keys.is_empty() {
            updates.extend(self.check_queue(users));
        }
        updates
    }

    fn activate(&mut self, key: &TransferKey) -> Vec<TransferWork> {
        let real_path = self
            .transfers
            .get(key)
            .unwrap()
            .real_path
            .as_ref()
            .expect("upload queued without a resolved path");
        let size = match fs::metadata(real_path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                self.net.peer(
                    key.0.clone(),
                    PeerMessage::UploadDenied {
                        file: key.1.clone(),
                        reason: FILE_READ_ERROR.into(),
                    },
                );
                return self.fail(key, format!("local file error: {error}"));
            }
        };
        self.token = increment_token(self.token);
        let token = self.token;
        self.queue.mark_active(key, token);
        self.transfers.attach_token(key, token);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.size = size;
        transfer.phase = TransferPhase::GettingStatus;
        transfer.activated_at = Some(Instant::now());
        let activated = TransferWork::Update(transfer.snapshot());
        info!(
            username = key.0,
            virtual_path = key.1,
            token,
            "requesting upload"
        );
        self.net.peer(
            key.0.clone(),
            PeerMessage::TransferRequest {
                direction: TransferDirection::Upload,
                token,
                file: key.1.clone(),
                filesize: Some(size),
            },
        );
        vec![activated]
    }

    pub fn handle_transfer_response(
        &mut self,
        username: &str,
        token: u32,
        reason: Option<&str>,
        users: &Users,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            debug!(username, token, "transfer response for unknown upload");
            return Vec::new();
        };
        if self.transfers.conn_of(&key).is_some() {
            debug!(
                username,
                token, "transfer response for upload with a file connection"
            );
            return Vec::new();
        }
        if let Some(reason) = reason {
            let mut updates = match reason {
                TransferRejectReason::COMPLETE => self.finish(&key, 0),
                CONNECTION_TIMEOUT => self.fail(&key, TransferRejectReason::CANCELLED.into()),
                reason => self.fail(&key, reason.to_owned()),
            };
            updates.extend(self.check_queue(users));
            return updates;
        }
        self.net.send(NetworkCommand::RequestFileConnection {
            username: username.to_owned(),
            token,
        });
        Vec::new()
    }

    pub fn handle_file_transfer_init(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
        users: &Users,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            return Vec::new();
        };
        if self.transfers.conn_of(&key).is_some() {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        }
        self.transfers.attach_conn(&key, conn_id);
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.activated_at = None;

        let real_path = transfer
            .real_path
            .as_ref()
            .expect("upload activated without a resolved path");
        let size = transfer.size;
        let opened = fs::File::open(real_path).and_then(|file| {
            let current = file.metadata()?.len();
            if current == size {
                Ok(file)
            } else {
                Err(io::Error::other(format!(
                    "file changed size from {size} to {current} bytes"
                )))
            }
        });
        match opened {
            Ok(file) => {
                transfer.phase = TransferPhase::Transferring;
                transfer.started_at = Some(Instant::now());
                transfer.speed_bps = 0;
                transfer.speed.reset();
                let started = TransferWork::Update(transfer.snapshot());
                info!(username, virtual_path = key.1, size, "upload started");
                self.net.send(NetworkCommand::UploadFile {
                    conn_id,
                    file,
                    size,
                });
                vec![started]
            }
            Err(error) => {
                self.net.peer(
                    username,
                    PeerMessage::UploadFailed {
                        file: key.1.clone(),
                    },
                );
                let mut updates = self.fail(&key, format!("local file error: {error}"));
                updates.extend(self.check_queue(users));
                updates
            }
        }
    }

    pub fn handle_upload_progress(
        &mut self,
        username: &str,
        token: u32,
        offset: u64,
        bytes_sent: u64,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            return Vec::new();
        };
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.started_offset = offset;
        transfer.bytes_done = offset + bytes_sent;
        transfer.speed_bps = transfer.speed.sample(transfer.bytes_done);
        vec![TransferWork::Progress(transfer.snapshot())]
    }

    pub fn handle_transfer_error(
        &mut self,
        username: &str,
        token: u32,
        error: &str,
        users: &Users,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            return Vec::new();
        };
        self.net.peer(
            username,
            PeerMessage::UploadFailed {
                file: key.1.clone(),
            },
        );
        let mut updates = self.fail(&key, error.to_owned());
        updates.extend(self.check_queue(users));
        updates
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: Option<u32>,
        conn_id: ConnId,
        users: &Users,
    ) -> Vec<TransferWork> {
        let key = match self.transfers.key_by_conn(conn_id).cloned().or_else(|| {
            token
                .and_then(|token| self.transfers.key_by_token(username, token).cloned())
                .filter(|key| self.transfers.conn_of(key).is_none())
        }) {
            Some(key) => key,
            None => return Vec::new(),
        };
        self.transfers.detach_conn(&key);
        let transfer = self.transfers.get(&key).unwrap();
        let mut updates = match transfer.phase {
            TransferPhase::Transferring if transfer.bytes_done >= transfer.size => {
                let delivered_bytes = transfer.bytes_done.saturating_sub(transfer.started_offset);
                self.finish(&key, delivered_bytes)
            }
            TransferPhase::Transferring => {
                self.net.peer(
                    key.0.clone(),
                    PeerMessage::UploadFailed {
                        file: key.1.clone(),
                    },
                );
                self.fail(&key, "connection closed".into())
            }
            _ => {
                self.deactivate(&key);
                Vec::new()
            }
        };
        updates.extend(self.check_queue(users));
        updates
    }

    pub fn handle_peer_connection_error(
        &mut self,
        username: &str,
        unsent: &[PeerMessage],
        is_offline: bool,
        users: &Users,
    ) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        for message in unsent {
            if let PeerMessage::TransferRequest {
                direction: TransferDirection::Upload,
                file,
                ..
            } = message
            {
                let key = (username.to_owned(), file.clone());
                let reason = if is_offline {
                    "user is offline"
                } else {
                    CONNECTION_TIMEOUT
                };
                updates.extend(self.fail(&key, reason.into()));
            }
        }
        if !updates.is_empty() {
            updates.extend(self.check_queue(users));
        }
        updates
    }

    pub fn abort(&mut self, id: TransferId, users: &Users) -> (AbortResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (AbortResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get_mut(&key).unwrap();
        if transfer.is_timed_out() {
            transfer.phase = TransferPhase::Aborted;
            return (
                AbortResult::Aborted,
                vec![TransferWork::Update(transfer.snapshot())],
            );
        }
        if !transfer.phase.is_active() {
            return (AbortResult::Aborted, Vec::new());
        }
        self.deactivate(&key);
        self.close_conn(&key);
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.phase = TransferPhase::Aborted;
        let aborted = TransferWork::Update(transfer.snapshot());
        self.net.peer(
            key.0,
            PeerMessage::UploadDenied {
                file: key.1,
                reason: TransferRejectReason::CANCELLED.into(),
            },
        );
        let mut updates = vec![aborted];
        updates.extend(self.check_queue(users));
        (AbortResult::Aborted, updates)
    }

    pub fn clear(
        &mut self,
        statuses: &[TransferStatus],
        users: &Users,
    ) -> (Vec<TransferId>, Vec<TransferWork>) {
        let removed: Vec<TransferId> = self
            .transfers
            .values()
            .filter(|transfer| statuses.contains(&transfer.phase.status()))
            .map(|transfer| transfer.id)
            .collect();
        for id in &removed {
            let (key, detached) = self.transfers.remove(*id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
            self.queue.release(&key, detached.token);
        }
        let updates = self.check_queue(users);
        (removed, updates)
    }

    pub fn clear_all(&mut self) -> Vec<TransferId> {
        let entries: Vec<(TransferId, bool)> = self
            .transfers
            .values()
            .map(|transfer| (transfer.id, transfer.phase.is_active()))
            .collect();
        let mut removed = Vec::new();
        for (id, active) in entries {
            let (key, detached) = self.transfers.remove(id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
            if active {
                self.net.peer(
                    key.0,
                    PeerMessage::UploadDenied {
                        file: key.1,
                        reason: TransferRejectReason::CANCELLED.into(),
                    },
                );
            }
            removed.push(id);
        }
        self.queue.clear();
        removed
    }

    pub fn handle_place_in_queue_request(
        &mut self,
        username: &str,
        virtual_path: &str,
        users: &Users,
    ) {
        let Some(place) = self.queue.place_of(username, virtual_path, users) else {
            return;
        };
        self.net.peer(
            username,
            PeerMessage::PlaceInQueueResponse {
                filename: virtual_path.to_owned(),
                place,
            },
        );
    }

    pub fn sweep_request_timeouts(&mut self, users: &Users) -> Vec<TransferWork> {
        let expired: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.phase == TransferPhase::GettingStatus
                    && transfer
                        .activated_at
                        .is_some_and(|at| at.elapsed() > TRANSFER_REQUEST_TIMEOUT)
            })
            .map(UploadTransfer::key)
            .collect();
        let mut updates = Vec::new();
        for key in &expired {
            updates.extend(self.fail(key, CONNECTION_TIMEOUT.into()));
        }
        if !expired.is_empty() {
            updates.extend(self.check_queue(users));
        }
        updates
    }

    pub fn sweep_queue(
        &mut self,
        ids: &mut TransferIds,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        if self.retried_at.elapsed() >= TIMED_OUT_RETRY_INTERVAL {
            self.retried_at = Instant::now();
            updates.extend(self.retry_timed_out(ids, shares, users));
        }
        updates.extend(self.check_queue(users));
        updates
    }

    fn retry_timed_out(
        &mut self,
        ids: &mut TransferIds,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> Vec<TransferWork> {
        let timed_out: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| transfer.is_timed_out())
            .map(UploadTransfer::key)
            .collect();
        let mut updates = Vec::new();
        for (username, virtual_path) in timed_out {
            if self.refusal(&username, &virtual_path, users).is_some() {
                continue;
            }
            let Some((real_path, size, attributes)) =
                shares.and_then(|shares| shares.resolve(&virtual_path, users.is_buddy(&username)))
            else {
                continue;
            };
            let attributes = attributes.clone();
            updates.extend(self.enqueue(
                ids,
                &username,
                &virtual_path,
                real_path,
                size,
                attributes,
            ));
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
            .map(UploadTransfer::key)
            .collect();
        let mut updates = Vec::new();
        for key in &active {
            updates.extend(self.fail(key, "server disconnected".into()));
        }
        updates
    }

    fn finish(&mut self, key: &TransferKey, delivered_bytes: u64) -> Vec<TransferWork> {
        self.deactivate(key);
        self.close_conn(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Finished;
        transfer.bytes_done = transfer.size;
        let mut avg_speed_bps = None;
        if delivered_bytes > 0
            && let Some(started_at) = transfer.started_at
        {
            let elapsed = started_at.elapsed().as_secs_f64();
            if elapsed >= 1.0 {
                self.upload_speed = (delivered_bytes as f64 / elapsed) as u32;
                avg_speed_bps = Some(self.upload_speed);
            }
        }
        let snapshot = self.transfers.get(key).unwrap().snapshot();
        if let Some(speed) = avg_speed_bps {
            self.net.server(ServerRequest::SendUploadSpeed { speed });
        }
        info!(username = key.0, virtual_path = key.1, "upload finished");
        vec![TransferWork::Finished {
            snapshot,
            avg_speed_bps,
            delivered_bytes,
        }]
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
        self.deactivate(key);
        self.close_conn(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(reason);
        vec![TransferWork::Update(transfer.snapshot())]
    }

    fn close_conn(&mut self, key: &TransferKey) {
        if let Some(conn_id) = self.transfers.detach_conn(key) {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
    }

    fn deactivate(&mut self, key: &TransferKey) {
        let token = self.transfers.detach_token(key);
        self.queue.release(key, token);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::client::shares::{self, ShareCatalog};
    use crate::network::spawn as spawn_network;
    use crate::types::SharedFolder;

    const TRACKS: [&str; 3] = ["Music\\a.mp3", "Music\\b.mp3", "Music\\c.mp3"];

    fn shares_of(tag: &str, names: &[&str]) -> SharesIndex {
        let dir = std::env::temp_dir().join(format!("newkitine-{tag}-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), b"payload").unwrap();
        }
        let catalog = shares::walk(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &[],
            &ShareCatalog::empty(),
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .expect("scan test shares");
        SharesIndex::from_catalog(catalog)
    }

    fn three_track_shares(tag: &str) -> SharesIndex {
        shares_of(tag, &["a.mp3", "b.mp3", "c.mp3"])
    }

    #[tokio::test]
    async fn a_new_index_denies_uploads_it_no_longer_shares() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());

        let shares = three_track_shares("revalidate");
        for path in [TRACKS[0], TRACKS[2]] {
            let (_, accepted) =
                uploads.handle_queue_upload(&mut ids, "peer", path, Some(&shares), &users);
            assert!(accepted);
        }

        let shrunk = shares_of("revalidate", &["a.mp3", "b.mp3"]);
        let updates = uploads.revalidate(&shrunk, &users);
        assert_eq!(updates.len(), 1);
        let kept = ("peer".to_owned(), TRACKS[0].to_owned());
        let dropped = ("peer".to_owned(), TRACKS[2].to_owned());
        assert_eq!(
            uploads.transfers.get(&kept).unwrap().phase,
            TransferPhase::Queued
        );
        assert_eq!(
            uploads.transfers.get(&dropped).unwrap().phase,
            TransferPhase::Failed(TransferRejectReason::FILE_NOT_SHARED.into())
        );
        assert!(uploads.revalidate(&shrunk, &users).is_empty());
    }

    #[tokio::test]
    async fn held_users_are_skipped_until_the_hold_lifts() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);

        let shares = three_track_shares("slots");

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());

        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "leech", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "human", TRACKS[0], Some(&shares), &users);
        assert!(accepted);

        users.set_restriction("leech".into(), Restriction::Hold);
        uploads.set_limits(2, 500, 0, "Banned".into());
        uploads.check_queue(&users);
        assert!(uploads.queue.is_active("human"));
        assert!(!uploads.queue.is_active("leech"));

        users.set_restriction("leech".into(), Restriction::None);
        uploads.check_queue(&users);
        assert!(uploads.queue.is_active("leech"));
    }

    #[tokio::test]
    async fn denied_restriction_rejects_queue_requests() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 2, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);

        let shares = three_track_shares("deny");

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_restriction(
            "leech".into(),
            Restriction::Denied {
                reason: "not welcome".into(),
            },
        );
        let (updates, accepted) =
            uploads.handle_queue_upload(&mut ids, "leech", TRACKS[0], Some(&shares), &users);
        assert!(!accepted);
        assert!(updates.is_empty());
    }

    #[tokio::test]
    async fn one_upload_per_peer_and_no_peer_waits_behind_another() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("nowait");
        let users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());

        for peer in ["one", "two", "three"] {
            for path in TRACKS {
                let (_, accepted) =
                    uploads.handle_queue_upload(&mut ids, peer, path, Some(&shares), &users);
                assert!(accepted);
            }
        }
        uploads.check_queue(&users);
        uploads.check_queue(&users);

        for peer in ["one", "two", "three"] {
            assert!(uploads.queue.is_active(peer), "{peer} is transferring");
        }
        assert_eq!(uploads.queue.active_user_count(), 3);
        assert_eq!(uploads.queue.len(), 6);
    }

    #[tokio::test]
    async fn slot_ceiling_counts_peers_not_transfers() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 2, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("ceiling");
        let users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());

        for peer in ["one", "two", "three"] {
            for path in TRACKS {
                uploads.handle_queue_upload(&mut ids, peer, path, Some(&shares), &users);
            }
        }
        uploads.check_queue(&users);

        assert_eq!(uploads.queue.active_user_count(), 2);
        assert!(!uploads.is_new_upload_accepted());
    }

    fn transferring(tag: &str) -> (Uploads, Users, u32, u64, u64) {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares(tag);
        let users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());

        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        let token = uploads.token;
        let conn_id: ConnId = 7;
        uploads.handle_file_transfer_init("peer", token, conn_id, &users);
        let size = uploads
            .transfers
            .get(&("peer".to_owned(), TRACKS[0].to_owned()))
            .unwrap()
            .size;
        (uploads, users, token, conn_id, size)
    }

    fn delivered_of(work: &[TransferWork]) -> Option<u64> {
        work.iter().find_map(|item| match item {
            TransferWork::Finished {
                delivered_bytes, ..
            } => Some(*delivered_bytes),
            _ => None,
        })
    }

    #[tokio::test]
    async fn a_capped_file_is_refused_while_other_files_still_queue() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("capped");
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.deny_file(
            "peer".into(),
            TRACKS[0].into(),
            Instant::now() + Duration::from_secs(60),
        );

        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        assert!(!accepted);
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[1], Some(&shares), &users);
        assert!(accepted);
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "other", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
    }

    #[tokio::test]
    async fn a_peer_resuming_at_the_end_of_the_file_is_not_a_delivery() {
        let (mut uploads, users, token, conn_id, size) = transferring("resume-end");
        uploads.handle_upload_progress("peer", token, size, 0);
        let work = uploads.handle_file_connection_closed("peer", Some(token), conn_id, &users);
        assert_eq!(delivered_of(&work), Some(0));
        let key = ("peer".to_owned(), TRACKS[0].to_owned());
        assert_eq!(
            uploads.transfers.get(&key).unwrap().phase,
            TransferPhase::Finished
        );
    }

    #[tokio::test]
    async fn a_peer_resuming_mid_file_is_a_delivery() {
        let (mut uploads, users, token, conn_id, size) = transferring("resume-mid");
        uploads.handle_upload_progress("peer", token, size / 2, size - size / 2);
        let work = uploads.handle_file_connection_closed("peer", Some(token), conn_id, &users);
        assert_eq!(delivered_of(&work), Some(size - size / 2));
    }

    #[tokio::test]
    async fn an_offset_past_the_end_of_the_file_is_not_a_delivery() {
        let (mut uploads, users, token, conn_id, size) = transferring("resume-past");
        uploads.handle_upload_progress("peer", token, size + 1, 0);
        let work = uploads.handle_file_connection_closed("peer", Some(token), conn_id, &users);
        assert_eq!(delivered_of(&work), Some(0));
        assert_eq!(uploads.upload_speed, 0);
    }

    #[tokio::test]
    async fn a_duplicate_file_connection_does_not_finish_the_primary_transfer() {
        let (mut uploads, users, token, conn_id, size) = transferring("dup-conn");
        uploads.handle_upload_progress("peer", token, 0, size);
        let work = uploads.handle_file_connection_closed("peer", Some(token), conn_id + 1, &users);
        assert!(work.is_empty());
        let key = ("peer".to_owned(), TRACKS[0].to_owned());
        assert_eq!(uploads.transfers.conn_of(&key), Some(conn_id));
    }

    fn users() -> Users {
        Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new())
    }

    fn phase_of(uploads: &Uploads, username: &str, path: &str) -> TransferPhase {
        uploads
            .transfers
            .get(&(username.to_owned(), path.to_owned()))
            .unwrap()
            .phase
            .clone()
    }

    #[tokio::test]
    async fn banning_denies_queued_and_active_uploads() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 1, 500, 0, "Go away".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("ban");
        let mut users = users();
        for path in [TRACKS[0], TRACKS[1]] {
            let (_, accepted) =
                uploads.handle_queue_upload(&mut ids, "peer", path, Some(&shares), &users);
            assert!(accepted);
        }
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "other", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        assert!(uploads.queue.is_active("peer"));

        users.banned.insert("peer".into());
        let updates = uploads.ban("peer", &users);
        assert!(updates.len() >= 2);
        for path in [TRACKS[0], TRACKS[1]] {
            assert_eq!(
                phase_of(&uploads, "peer", path),
                TransferPhase::Failed("Go away".into())
            );
        }
        assert!(!uploads.queue.is_active("peer"));
        assert!(uploads.queue.is_active("other"));
        assert_eq!(uploads.queue.queued_for("peer"), 0);
    }

    #[tokio::test]
    async fn activation_advertises_the_current_file_size() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("restat");
        let users = users();
        for path in [TRACKS[0], TRACKS[1]] {
            let (_, accepted) =
                uploads.handle_queue_upload(&mut ids, "peer", path, Some(&shares), &users);
            assert!(accepted);
        }
        let grown = ("peer".to_owned(), TRACKS[0].to_owned());
        let gone = ("peer".to_owned(), TRACKS[1].to_owned());
        let grown_path = uploads
            .transfers
            .get(&grown)
            .unwrap()
            .real_path
            .clone()
            .unwrap();
        let gone_path = uploads
            .transfers
            .get(&gone)
            .unwrap()
            .real_path
            .clone()
            .unwrap();
        std::fs::write(&grown_path, b"a longer payload").unwrap();
        std::fs::remove_file(&gone_path).unwrap();

        uploads.set_limits(1, 500, 0, "Banned".into());
        let updates = uploads.check_queue(&users);
        assert!(!updates.is_empty());
        assert_eq!(uploads.transfers.get(&grown).unwrap().size, 16);
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::GettingStatus
        );

        let id = uploads.transfers.get(&grown).unwrap().id;
        let _ = uploads.abort(id, &users);
        assert!(matches!(
            phase_of(&uploads, "peer", TRACKS[1]),
            TransferPhase::Failed(reason) if reason.starts_with("local file error")
        ));
    }

    #[tokio::test]
    async fn a_file_changed_after_the_request_is_not_streamed() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("changed");
        let users = users();
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        let key = ("peer".to_owned(), TRACKS[0].to_owned());
        let path = uploads
            .transfers
            .get(&key)
            .unwrap()
            .real_path
            .clone()
            .unwrap();
        std::fs::write(&path, b"x").unwrap();
        let token = uploads.token;
        uploads.handle_file_transfer_init("peer", token, 7, &users);
        assert!(matches!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::Failed(reason) if reason.contains("changed size")
        ));
        assert_eq!(uploads.transfers.conn_of(&key), None);
    }

    #[tokio::test]
    async fn a_transfer_response_after_the_file_connection_is_ignored() {
        let (mut uploads, users, token, conn_id, _) = transferring("late-response");
        let work = uploads.handle_transfer_response(
            "peer",
            token,
            Some(TransferRejectReason::CANCELLED),
            &users,
        );
        assert!(work.is_empty());
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::Transferring
        );
        assert!(uploads.queue.is_active("peer"));
        let key = ("peer".to_owned(), TRACKS[0].to_owned());
        assert_eq!(uploads.transfers.conn_of(&key), Some(conn_id));
    }

    #[tokio::test]
    async fn a_duplicate_request_at_the_queue_limit_is_silently_accepted() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 1, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("dup-request");
        let users = users();
        let (updates, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        assert_eq!(updates.len(), 1);
        let (updates, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        assert!(accepted);
        assert!(updates.is_empty());
        assert!(matches!(
            uploads.admit("peer", TRACKS[1], Some(&shares), &users),
            Err(reason) if reason == TransferRejectReason::TOO_MANY_FILES
        ));
    }

    #[tokio::test]
    async fn queueing_more_files_keeps_a_users_turn() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("fairness");
        let users = users();
        let _ = uploads.handle_queue_upload(&mut ids, "first", TRACKS[0], Some(&shares), &users);
        let _ = uploads.handle_queue_upload(&mut ids, "second", TRACKS[0], Some(&shares), &users);
        let _ = uploads.handle_queue_upload(&mut ids, "first", TRACKS[1], Some(&shares), &users);
        assert_eq!(
            uploads.queue.select_next(&users).map(|key| key.0),
            Some("first".to_owned())
        );

        uploads.set_limits(1, 500, 0, "Banned".into());
        let _ = uploads.check_queue(&users);
        assert!(uploads.queue.is_active("first"));
        let id = uploads
            .transfers
            .get(&("first".to_owned(), TRACKS[0].to_owned()))
            .unwrap()
            .id;
        let _ = uploads.abort(id, &users);
        assert!(uploads.queue.is_active("second"));
    }

    #[tokio::test]
    async fn queue_place_follows_round_robin_order() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("place");
        let mut users = users();
        for path in TRACKS {
            let _ = uploads.handle_queue_upload(&mut ids, "many", path, Some(&shares), &users);
        }
        let _ = uploads.handle_queue_upload(&mut ids, "late", TRACKS[0], Some(&shares), &users);
        assert_eq!(uploads.queue.place_of("late", TRACKS[0], &users), Some(3));
        assert_eq!(uploads.queue.place_of("many", TRACKS[2], &users), Some(5));
        assert_eq!(uploads.queue.place_of("late", TRACKS[1], &users), None);

        users.handle_privileged_users(vec!["late".into()]);
        assert_eq!(uploads.queue.place_of("late", TRACKS[0], &users), Some(2));
        assert_eq!(uploads.queue.place_of("many", TRACKS[0], &users), Some(4));
    }

    #[tokio::test]
    async fn timed_out_uploads_are_requeued_periodically() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("retry");
        let users = users();
        let _ = uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        let _ = uploads.handle_queue_upload(&mut ids, "gone", TRACKS[0], Some(&shares), &users);
        let request = |file: &str| PeerMessage::TransferRequest {
            direction: TransferDirection::Upload,
            token: 0,
            file: file.to_owned(),
            filesize: None,
        };
        let _ = uploads.handle_peer_connection_error("peer", &[request(TRACKS[0])], false, &users);
        let _ = uploads.handle_peer_connection_error("gone", &[request(TRACKS[0])], true, &users);
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::Failed(CONNECTION_TIMEOUT.into())
        );

        assert!(
            uploads
                .sweep_queue(&mut ids, Some(&shares), &users)
                .is_empty()
        );
        uploads.retried_at -= TIMED_OUT_RETRY_INTERVAL;
        let updates = uploads.sweep_queue(&mut ids, Some(&shares), &users);
        assert!(!updates.is_empty());
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::GettingStatus
        );
        assert_eq!(
            phase_of(&uploads, "gone", TRACKS[0]),
            TransferPhase::Failed("user is offline".into())
        );

        let _ = uploads.handle_peer_connection_error("peer", &[request(TRACKS[0])], false, &users);
        let id = uploads
            .transfers
            .get(&("peer".to_owned(), TRACKS[0].to_owned()))
            .unwrap()
            .id;
        let (_, updates) = uploads.abort(id, &users);
        assert_eq!(updates.len(), 1);
        uploads.retried_at -= TIMED_OUT_RETRY_INTERVAL;
        let _ = uploads.sweep_queue(&mut ids, Some(&shares), &users);
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::Aborted
        );
    }

    #[tokio::test]
    async fn only_local_timeouts_are_retried_and_a_ban_retires_them() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 999, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let shares = three_track_shares("retry-ban");
        let mut users = users();
        let _ = uploads.handle_queue_upload(&mut ids, "peer", TRACKS[0], Some(&shares), &users);
        let _ = uploads.handle_queue_upload(&mut ids, "liar", TRACKS[0], Some(&shares), &users);
        let token = uploads.token;
        let _ = uploads.handle_transfer_response("liar", token, Some(CONNECTION_TIMEOUT), &users);
        assert_eq!(
            phase_of(&uploads, "liar", TRACKS[0]),
            TransferPhase::Failed(TransferRejectReason::CANCELLED.into())
        );
        let request = PeerMessage::TransferRequest {
            direction: TransferDirection::Upload,
            token: 0,
            file: TRACKS[0].to_owned(),
            filesize: None,
        };
        let _ = uploads.handle_peer_connection_error("peer", &[request], false, &users);

        users.banned.insert("peer".into());
        let updates = uploads.ban("peer", &users);
        assert_eq!(updates.len(), 1);
        users.banned.remove("peer");
        uploads.retried_at -= TIMED_OUT_RETRY_INTERVAL;
        let _ = uploads.sweep_queue(&mut ids, Some(&shares), &users);
        assert_eq!(
            phase_of(&uploads, "peer", TRACKS[0]),
            TransferPhase::Failed("Banned".into())
        );
        assert_eq!(
            phase_of(&uploads, "liar", TRACKS[0]),
            TransferPhase::Failed(TransferRejectReason::CANCELLED.into())
        );
    }

    #[tokio::test]
    async fn a_new_index_denies_uploads_whose_path_now_resolves_elsewhere() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500, 0, "Banned".into());
        let mut ids = TransferIds::new(&[]);
        let users = users();
        let path = "Music\\foo.mp3";
        let shares = shares_of("case", &["foo.mp3", "FOO.mp3"]);
        let (_, accepted) =
            uploads.handle_queue_upload(&mut ids, "peer", path, Some(&shares), &users);
        assert!(accepted);
        assert!(uploads.revalidate(&shares, &users).is_empty());

        let filtered = shares_of("case", &["FOO.mp3"]);
        assert!(filtered.resolve(path, false).is_some());
        let updates = uploads.revalidate(&filtered, &users);
        assert_eq!(updates.len(), 1);
        assert_eq!(
            phase_of(&uploads, "peer", path),
            TransferPhase::Failed(TransferRejectReason::FILE_NOT_SHARED.into())
        );
    }
}
