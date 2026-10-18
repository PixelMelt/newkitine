use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::queue::UploadQueue;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use super::{
    CONNECTION_CLOSED, CONNECTION_TIMEOUT, LOCAL_FILE_ERROR, TRANSFER_REQUEST_TIMEOUT, TransferIds,
    TransferPhase, TransferRejectReason, USER_OFFLINE, peer_reason,
};
use crate::client::shares::SharesIndex;
use crate::client::users::Users;
use crate::client::{AbortResult, TransferWork};
use crate::network::ConnId;
use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::{PeerMessage, ServerRequest, increment_token, initial_token};
use crate::types::{
    FileAttributes, Restriction, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const TIMED_OUT_RETRY_INTERVAL: Duration = Duration::from_secs(180);

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
    started_offset: u64,
    activated_at: Option<Instant>,
    started_at: Option<Instant>,
    speed: SpeedMeter,
}

impl UploadTransfer {
    fn new(
        id: TransferId,
        key: TransferKey,
        real_path: Option<PathBuf>,
        size: u64,
        attributes: FileAttributes,
    ) -> Self {
        Self {
            id,
            username: key.0,
            virtual_path: key.1,
            real_path,
            size,
            attributes,
            phase: TransferPhase::Queued,
            bytes_done: 0,
            started_offset: 0,
            activated_at: None,
            started_at: None,
            speed: SpeedMeter::default(),
        }
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
                self.speed.bps()
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
    suspended: bool,
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
            queue: UploadQueue::default(),
            token: initial_token(),
            retried_at: Instant::now(),
            upload_speed: 0,
            suspended: true,
        }
    }

    pub fn resume(&mut self, users: &Users) -> Vec<TransferWork> {
        self.suspended = false;
        self.check_queue(users)
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

    pub fn seed(&mut self, seed: TransferSnapshot) {
        let phase = TransferPhase::from_seed(&seed);
        let key = (seed.username, seed.virtual_path);
        let mut transfer = UploadTransfer::new(
            seed.id,
            key.clone(),
            seed.file_path.map(PathBuf::from),
            seed.size,
            seed.attributes,
        );
        transfer.phase = phase;
        transfer.bytes_done = seed.bytes_done;
        self.transfers.insert(seed.id, key, transfer);
    }

    pub fn handle_queue_upload(
        &mut self,
        ids: &mut TransferIds,
        username: &str,
        legacy_token: Option<u32>,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> (Vec<TransferWork>, bool) {
        let key = (username.to_owned(), virtual_path.to_owned());
        let admission = self.admit(username, virtual_path, shares, users);
        let refusal = admission.as_ref().err();
        match legacy_token {
            Some(token) => self.net.peer(
                username,
                PeerMessage::TransferResponse {
                    token,
                    allowed: false,
                    reason: Some(
                        refusal
                            .map_or(TransferRejectReason::QUEUED, String::as_str)
                            .into(),
                    ),
                    filesize: None,
                },
            ),
            None => {
                if let Some(reason) = refusal {
                    self.deny(&key, reason);
                }
            }
        }
        let accepted = admission.is_ok();
        let mut updates = Vec::new();
        if let Ok(Some((real_path, size, attributes))) = admission {
            updates = self.enqueue(ids, key, real_path, size, attributes);
            updates.extend(self.check_queue(users));
        }
        (updates, accepted)
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
    ) -> Result<Option<(PathBuf, u64, FileAttributes)>, String> {
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
            return Ok(None);
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
        Ok(Some((real_path, size, attributes.clone())))
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
        key: TransferKey,
        real_path: PathBuf,
        size: u64,
        attributes: FileAttributes,
    ) -> Vec<TransferWork> {
        let id = match self.transfers.get(&key) {
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        let transfer = UploadTransfer::new(id, key.clone(), Some(real_path), size, attributes);
        let queued = TransferWork::Update(transfer.snapshot());
        self.transfers.insert(id, key.clone(), transfer);
        self.queue.push(key);
        vec![queued]
    }

    pub fn check_queue(&mut self, users: &Users) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        if self.suspended {
            return updates;
        }
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
        let retryable = self
            .transfers
            .keys_where(|transfer| transfer.username == username && transfer.is_timed_out());
        for key in &retryable {
            let transfer = self.transfers.get_mut(key).unwrap();
            transfer.phase = TransferPhase::Failed(reason.clone());
            updates.push(TransferWork::Update(transfer.snapshot()));
        }
        updates
    }

    pub fn deny_all(&mut self, username: &str, reason: &str, users: &Users) -> Vec<TransferWork> {
        let pending = self
            .transfers
            .keys_where(|transfer| transfer.username == username && transfer.phase.is_active());
        self.fail_each(&pending, reason, true, users)
    }

    pub fn deny_file(&mut self, key: &TransferKey, users: &Users) -> Vec<TransferWork> {
        let active = |transfer: &UploadTransfer| transfer.phase.is_active();
        if !self.transfers.get(key).is_some_and(active) {
            return Vec::new();
        }
        let keys = std::slice::from_ref(key);
        self.fail_each(keys, TransferRejectReason::REPEATED, true, users)
    }

    pub fn revalidate(&mut self, shares: &SharesIndex, users: &Users) -> Vec<TransferWork> {
        let unshared = self.transfers.keys_where(|transfer| {
            transfer.phase.is_active()
                && shares
                    .resolve(&transfer.virtual_path, users.is_buddy(&transfer.username))
                    .is_none_or(|(real_path, _, _)| transfer.real_path.as_ref() != Some(&real_path))
        });
        self.fail_each(
            &unshared,
            TransferRejectReason::FILE_NOT_SHARED,
            true,
            users,
        )
    }

    fn deny(&self, key: &TransferKey, reason: &str) {
        self.net.peer(
            key.0.clone(),
            PeerMessage::UploadDenied {
                file: key.1.clone(),
                reason: reason.to_owned(),
            },
        );
    }

    fn fail_each(
        &mut self,
        keys: &[TransferKey],
        reason: &str,
        deny: bool,
        users: &Users,
    ) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        for key in keys {
            if deny {
                self.deny(key, reason);
            }
            updates.extend(self.fail(key, reason.to_owned()));
        }
        if !updates.is_empty() {
            updates.extend(self.check_queue(users));
        }
        updates
    }

    fn fail_notified(&mut self, key: &TransferKey, reason: String) -> Vec<TransferWork> {
        self.net.peer(
            key.0.clone(),
            PeerMessage::UploadFailed {
                file: key.1.clone(),
            },
        );
        self.fail(key, reason)
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
                self.deny(key, TransferRejectReason::FILE_READ_ERROR);
                return self.fail(key, format!("{LOCAL_FILE_ERROR}: {error}"));
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
                reason => self.fail(&key, peer_reason(reason)),
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
            self.net.send(NetworkCommand::CloseConnection(conn_id));
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
                let mut updates = self.fail_notified(&key, format!("{LOCAL_FILE_ERROR}: {error}"));
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
        transfer.speed.sample(transfer.bytes_done);
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
        let mut updates = self.fail_notified(&key, error.to_owned());
        updates.extend(self.check_queue(users));
        updates
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
        users: &Users,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned().or_else(|| {
            self.transfers
                .key_by_token(username, token)
                .cloned()
                .filter(|key| self.transfers.conn_of(key).is_none())
        }) else {
            return Vec::new();
        };
        self.transfers.detach_conn(&key);
        let transfer = self.transfers.get(&key).unwrap();
        let mut updates = match transfer.phase {
            TransferPhase::Transferring if transfer.bytes_done >= transfer.size => {
                let delivered_bytes = transfer.bytes_done.saturating_sub(transfer.started_offset);
                self.finish(&key, delivered_bytes)
            }
            TransferPhase::Transferring => self.fail_notified(&key, CONNECTION_CLOSED.into()),
            _ => self.fail(&key, CONNECTION_TIMEOUT.into()),
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
        let reason = if is_offline {
            USER_OFFLINE
        } else {
            CONNECTION_TIMEOUT
        };
        let keys: Vec<TransferKey> = unsent
            .iter()
            .filter_map(|message| match message {
                PeerMessage::TransferRequest {
                    direction: TransferDirection::Upload,
                    file,
                    ..
                } => Some((username.to_owned(), file.clone())),
                _ => None,
            })
            .collect();
        self.fail_each(&keys, reason, false, users)
    }

    pub fn abort(&mut self, id: TransferId, users: &Users) -> (AbortResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (AbortResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get(&key).unwrap();
        let active = transfer.phase.is_active();
        if !active && !transfer.is_timed_out() {
            return (AbortResult::Aborted, Vec::new());
        }
        if active {
            self.release(&key);
        }
        let transfer = self.transfers.get_mut(&key).unwrap();
        transfer.phase = TransferPhase::Aborted;
        let mut updates = vec![TransferWork::Update(transfer.snapshot())];
        if active {
            self.deny(&key, TransferRejectReason::CANCELLED);
            updates.extend(self.check_queue(users));
        }
        (AbortResult::Aborted, updates)
    }

    pub fn clear(
        &mut self,
        statuses: Option<&[TransferStatus]>,
        users: &Users,
    ) -> (Vec<TransferId>, Vec<TransferWork>) {
        let entries: Vec<(TransferId, bool)> = self
            .transfers
            .values()
            .filter(|transfer| {
                statuses.is_none_or(|statuses| statuses.contains(&transfer.phase.status()))
            })
            .map(|transfer| (transfer.id, transfer.phase.is_active()))
            .collect();
        let mut removed = Vec::with_capacity(entries.len());
        for (id, active) in entries {
            let (key, detached) = self.transfers.remove(id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
            self.queue.release(&key, detached.token);
            if active {
                self.deny(&key, TransferRejectReason::CANCELLED);
            }
            removed.push(id);
        }
        (removed, self.check_queue(users))
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

    pub fn user_offline(&mut self, username: &str, users: &Users) -> Vec<TransferWork> {
        let pending = self.transfers.keys_where(|transfer| {
            transfer.username == username && transfer.phase == TransferPhase::GettingStatus
        });
        self.fail_each(&pending, USER_OFFLINE, false, users)
    }

    pub fn sweep_request_timeouts(&mut self, users: &Users) -> Vec<TransferWork> {
        let expired = self.transfers.keys_where(|transfer| {
            transfer.phase == TransferPhase::GettingStatus
                && transfer
                    .activated_at
                    .is_some_and(|at| at.elapsed() > TRANSFER_REQUEST_TIMEOUT)
        });
        self.fail_each(&expired, CONNECTION_TIMEOUT, false, users)
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
        let timed_out = self.transfers.keys_where(UploadTransfer::is_timed_out);
        let mut updates = Vec::new();
        for key in timed_out {
            if self.refusal(&key.0, &key.1, users).is_some() {
                continue;
            }
            let Some((real_path, size, attributes)) =
                shares.and_then(|shares| shares.resolve(&key.1, users.is_buddy(&key.0)))
            else {
                continue;
            };
            let attributes = attributes.clone();
            updates.extend(self.enqueue(ids, key, real_path, size, attributes));
        }
        updates
    }

    pub fn reset(&mut self) -> Vec<TransferWork> {
        self.suspended = true;
        self.upload_speed = 0;
        let active = self.transfers.keys_where(|transfer| {
            matches!(
                transfer.phase,
                TransferPhase::GettingStatus | TransferPhase::Transferring
            )
        });
        active
            .iter()
            .flat_map(|key| self.fail(key, "server disconnected".into()))
            .collect()
    }

    fn finish(&mut self, key: &TransferKey, delivered_bytes: u64) -> Vec<TransferWork> {
        self.release(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Finished;
        transfer.bytes_done = transfer.size;
        let mut avg_speed_bps = None;
        if delivered_bytes > 0
            && let Some(started_at) = transfer.started_at
        {
            let elapsed = started_at.elapsed().as_secs_f64();
            if elapsed >= 1.0 {
                avg_speed_bps = Some((delivered_bytes as f64 / elapsed) as u32);
            }
        }
        let snapshot = transfer.snapshot();
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
        let active = |transfer: &UploadTransfer| transfer.phase.is_active();
        if !self.transfers.get(key).is_some_and(active) {
            return Vec::new();
        }
        self.release(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(reason);
        vec![TransferWork::Update(transfer.snapshot())]
    }

    fn release(&mut self, key: &TransferKey) {
        let detached = self.transfers.detach(key);
        self.queue.release(key, detached.token);
        if let Some(conn_id) = detached.conn_id {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::client::shares::{self, ShareCatalog};
    use crate::network::{NetworkEvent, spawn as spawn_network};
    use crate::types::SharedFolder;
    use tokio::sync::mpsc;

    const TRACKS: [&str; 3] = ["Music\\a.mp3", "Music\\b.mp3", "Music\\c.mp3"];
    const CONN: ConnId = 7;

    fn shares_of(tag: &str, names: &[&str]) -> SharesIndex {
        let dir = std::env::temp_dir().join(format!("soulseek-{tag}-test-{}", std::process::id()));
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

    struct Harness {
        uploads: Uploads,
        ids: TransferIds,
        shares: SharesIndex,
        users: Users,
        _events: mpsc::Receiver<NetworkEvent>,
    }

    impl Harness {
        fn new(tag: &str, slots: usize) -> Self {
            let (net, _events) = spawn_network();
            let mut uploads = Uploads::new(net, slots, 500, 0, "Banned".into());
            uploads.suspended = false;
            Self {
                uploads,
                ids: TransferIds::new(&[]),
                shares: shares_of(tag, &["a.mp3", "b.mp3", "c.mp3"]),
                users: Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new()),
                _events,
            }
        }

        fn transferring(tag: &str) -> (Self, u32, u64) {
            let mut harness = Self::new(tag, 999);
            harness.queue("peer", TRACKS[0]);
            let token = harness.uploads.token;
            harness
                .uploads
                .handle_file_transfer_init("peer", token, CONN, &harness.users);
            let size = harness.transfer("peer", TRACKS[0]).size;
            (harness, token, size)
        }

        fn request(&mut self, username: &str, path: &str) -> (Vec<TransferWork>, bool) {
            self.uploads.handle_queue_upload(
                &mut self.ids,
                username,
                None,
                path,
                Some(&self.shares),
                &self.users,
            )
        }

        fn queue(&mut self, username: &str, path: &str) {
            assert!(self.request(username, path).1);
        }

        fn transfer(&self, username: &str, path: &str) -> &UploadTransfer {
            let key = (username.to_owned(), path.to_owned());
            self.uploads.transfers.get(&key).unwrap()
        }

        fn phase(&self, username: &str, path: &str) -> TransferPhase {
            self.transfer(username, path).phase.clone()
        }

        fn conn(&self, username: &str, path: &str) -> Option<ConnId> {
            let key = (username.to_owned(), path.to_owned());
            self.uploads.transfers.conn_of(&key)
        }

        fn set_slots(&mut self, slots: usize) -> Vec<TransferWork> {
            self.uploads.set_limits(slots, 500, 0, "Banned".into());
            self.uploads.check_queue(&self.users)
        }

        fn abort(&mut self, username: &str, path: &str) -> Vec<TransferWork> {
            let id = self.transfer(username, path).id;
            self.uploads.abort(id, &self.users).1
        }

        fn close(&mut self, token: u32, conn_id: ConnId) -> Vec<TransferWork> {
            self.uploads
                .handle_file_connection_closed("peer", token, conn_id, &self.users)
        }

        fn drop_request(&mut self, username: &str, is_offline: bool) {
            let request = PeerMessage::TransferRequest {
                direction: TransferDirection::Upload,
                token: 0,
                file: TRACKS[0].to_owned(),
                filesize: None,
            };
            let _ = self.uploads.handle_peer_connection_error(
                username,
                &[request],
                is_offline,
                &self.users,
            );
        }

        fn retry_due(&mut self) -> Vec<TransferWork> {
            self.uploads.retried_at -= TIMED_OUT_RETRY_INTERVAL;
            self.sweep()
        }

        fn sweep(&mut self) -> Vec<TransferWork> {
            self.uploads
                .sweep_queue(&mut self.ids, Some(&self.shares), &self.users)
        }
    }

    fn not_shared() -> TransferPhase {
        TransferPhase::Failed(TransferRejectReason::FILE_NOT_SHARED.into())
    }

    #[tokio::test]
    async fn a_new_index_denies_uploads_it_no_longer_shares() {
        let mut harness = Harness::new("revalidate", 0);
        for path in [TRACKS[0], TRACKS[2]] {
            harness.queue("peer", path);
        }

        let shrunk = shares_of("revalidate", &["a.mp3", "b.mp3"]);
        let updates = harness.uploads.revalidate(&shrunk, &harness.users);
        assert_eq!(updates.len(), 1);
        assert_eq!(harness.phase("peer", TRACKS[0]), TransferPhase::Queued);
        assert_eq!(harness.phase("peer", TRACKS[2]), not_shared());
        assert!(
            harness
                .uploads
                .revalidate(&shrunk, &harness.users)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn held_users_are_skipped_until_the_hold_lifts() {
        let mut harness = Harness::new("slots", 0);
        harness.queue("leech", TRACKS[0]);
        harness.queue("human", TRACKS[0]);

        harness
            .users
            .set_restriction("leech".into(), Restriction::Hold);
        harness.set_slots(2);
        assert!(harness.uploads.queue.is_active("human"));
        assert!(!harness.uploads.queue.is_active("leech"));

        harness
            .users
            .set_restriction("leech".into(), Restriction::None);
        harness.uploads.check_queue(&harness.users);
        assert!(harness.uploads.queue.is_active("leech"));
    }

    #[tokio::test]
    async fn denied_restriction_rejects_queue_requests() {
        let mut harness = Harness::new("deny", 2);
        harness.users.set_restriction(
            "leech".into(),
            Restriction::Denied {
                reason: "not welcome".into(),
            },
        );
        let (updates, accepted) = harness.request("leech", TRACKS[0]);
        assert!(!accepted);
        assert!(updates.is_empty());
    }

    fn three_peers_queue_every_track(tag: &str, slots: usize) -> Harness {
        let mut harness = Harness::new(tag, slots);
        for peer in ["one", "two", "three"] {
            for path in TRACKS {
                harness.queue(peer, path);
            }
        }
        harness.uploads.check_queue(&harness.users);
        harness
    }

    #[tokio::test]
    async fn one_upload_per_peer_and_no_peer_waits_behind_another() {
        let harness = three_peers_queue_every_track("nowait", 999);
        for peer in ["one", "two", "three"] {
            assert!(
                harness.uploads.queue.is_active(peer),
                "{peer} is transferring"
            );
        }
        assert_eq!(harness.uploads.queue.active_user_count(), 3);
        assert_eq!(harness.uploads.queue.len(), 6);
    }

    #[tokio::test]
    async fn slot_ceiling_counts_peers_not_transfers() {
        let harness = three_peers_queue_every_track("ceiling", 2);
        assert_eq!(harness.uploads.queue.active_user_count(), 2);
        assert!(!harness.uploads.is_new_upload_accepted());
    }

    #[tokio::test]
    async fn a_capped_file_is_refused_while_other_files_still_queue() {
        let mut harness = Harness::new("capped", 999);
        harness.users.deny_file(
            "peer".into(),
            TRACKS[0].into(),
            Instant::now() + Duration::from_secs(60),
        );

        assert!(!harness.request("peer", TRACKS[0]).1);
        harness.queue("peer", TRACKS[1]);
        harness.queue("other", TRACKS[0]);
    }

    #[tokio::test]
    async fn only_bytes_sent_past_the_resume_offset_are_a_delivery() {
        for tag in ["resume-end", "resume-mid", "resume-past"] {
            let (mut harness, token, size) = Harness::transferring(tag);
            let (offset, sent) = match tag {
                "resume-end" => (size, 0),
                "resume-mid" => (size / 2, size - size / 2),
                _ => (size + 1, 0),
            };
            harness
                .uploads
                .handle_upload_progress("peer", token, offset, sent);
            let delivered = harness
                .close(token, CONN)
                .iter()
                .find_map(|item| match item {
                    TransferWork::Finished {
                        delivered_bytes, ..
                    } => Some(*delivered_bytes),
                    _ => None,
                });
            assert_eq!(delivered, Some(sent), "{tag}");
            assert_eq!(harness.phase("peer", TRACKS[0]), TransferPhase::Finished);
            assert_eq!(harness.uploads.upload_speed, 0);
        }
    }

    #[tokio::test]
    async fn a_duplicate_file_connection_does_not_finish_the_primary_transfer() {
        let (mut harness, token, size) = Harness::transferring("dup-conn");
        harness
            .uploads
            .handle_upload_progress("peer", token, 0, size);
        assert!(harness.close(token, CONN + 1).is_empty());
        assert_eq!(harness.conn("peer", TRACKS[0]), Some(CONN));
    }

    #[tokio::test]
    async fn banning_denies_queued_and_active_uploads() {
        let mut harness = Harness::new("ban", 1);
        harness.uploads.banned_message = "Go away".into();
        for path in [TRACKS[0], TRACKS[1]] {
            harness.queue("peer", path);
        }
        harness.queue("other", TRACKS[0]);
        assert!(harness.uploads.queue.is_active("peer"));

        harness.users.banned.insert("peer".into());
        let updates = harness.uploads.ban("peer", &harness.users);
        assert!(updates.len() >= 2);
        for path in [TRACKS[0], TRACKS[1]] {
            assert_eq!(
                harness.phase("peer", path),
                TransferPhase::Failed("Go away".into())
            );
        }
        assert!(!harness.uploads.queue.is_active("peer"));
        assert!(harness.uploads.queue.is_active("other"));
        assert_eq!(harness.uploads.queue.queued_for("peer"), 0);
    }

    #[tokio::test]
    async fn activation_advertises_the_current_file_size() {
        let mut harness = Harness::new("restat", 0);
        for path in [TRACKS[0], TRACKS[1]] {
            harness.queue("peer", path);
        }
        let real_path = |path| harness.transfer("peer", path).real_path.clone().unwrap();
        std::fs::write(real_path(TRACKS[0]), b"a longer payload").unwrap();
        std::fs::remove_file(real_path(TRACKS[1])).unwrap();

        assert!(!harness.set_slots(1).is_empty());
        assert_eq!(harness.transfer("peer", TRACKS[0]).size, 16);
        assert_eq!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::GettingStatus
        );

        harness.abort("peer", TRACKS[0]);
        assert!(matches!(
            harness.phase("peer", TRACKS[1]),
            TransferPhase::Failed(reason) if reason.starts_with(LOCAL_FILE_ERROR)
        ));
    }

    #[tokio::test]
    async fn a_file_changed_after_the_request_is_not_streamed() {
        let mut harness = Harness::new("changed", 999);
        harness.queue("peer", TRACKS[0]);
        let path = harness
            .transfer("peer", TRACKS[0])
            .real_path
            .clone()
            .unwrap();
        std::fs::write(&path, b"x").unwrap();
        let token = harness.uploads.token;
        harness
            .uploads
            .handle_file_transfer_init("peer", token, CONN, &harness.users);
        assert!(matches!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::Failed(reason) if reason.contains("changed size")
        ));
        assert_eq!(harness.conn("peer", TRACKS[0]), None);
    }

    #[tokio::test]
    async fn a_transfer_response_after_the_file_connection_is_ignored() {
        let (mut harness, token, _) = Harness::transferring("late-response");
        let work = harness.uploads.handle_transfer_response(
            "peer",
            token,
            Some(TransferRejectReason::CANCELLED),
            &harness.users,
        );
        assert!(work.is_empty());
        assert_eq!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::Transferring
        );
        assert!(harness.uploads.queue.is_active("peer"));
        assert_eq!(harness.conn("peer", TRACKS[0]), Some(CONN));
    }

    #[tokio::test]
    async fn a_duplicate_request_at_the_queue_limit_is_silently_accepted() {
        let mut harness = Harness::new("dup-request", 0);
        harness.uploads.queue_file_limit = 1;
        let (updates, accepted) = harness.request("peer", TRACKS[0]);
        assert!(accepted);
        assert_eq!(updates.len(), 1);
        let (updates, accepted) = harness.request("peer", TRACKS[0]);
        assert!(accepted);
        assert!(updates.is_empty());
        assert!(matches!(
            harness
                .uploads
                .admit("peer", TRACKS[1], Some(&harness.shares), &harness.users),
            Err(reason) if reason == TransferRejectReason::TOO_MANY_FILES
        ));
    }

    #[tokio::test]
    async fn queueing_more_files_keeps_a_users_turn() {
        let mut harness = Harness::new("fairness", 0);
        harness.queue("first", TRACKS[0]);
        harness.queue("second", TRACKS[0]);
        harness.queue("first", TRACKS[1]);
        assert_eq!(
            harness
                .uploads
                .queue
                .select_next(&harness.users)
                .map(|key| key.0),
            Some("first".to_owned())
        );

        harness.set_slots(1);
        assert!(harness.uploads.queue.is_active("first"));
        harness.abort("first", TRACKS[0]);
        assert!(harness.uploads.queue.is_active("second"));
    }

    #[tokio::test]
    async fn queue_place_follows_round_robin_order() {
        let mut harness = Harness::new("place", 0);
        for path in TRACKS {
            harness.queue("many", path);
        }
        harness.queue("late", TRACKS[0]);
        let place = |harness: &Harness, username, path| {
            harness
                .uploads
                .queue
                .place_of(username, path, &harness.users)
        };
        assert_eq!(place(&harness, "late", TRACKS[0]), Some(3));
        assert_eq!(place(&harness, "many", TRACKS[2]), Some(5));
        assert_eq!(place(&harness, "late", TRACKS[1]), None);

        harness.users.handle_privileged_users(vec!["late".into()]);
        assert_eq!(place(&harness, "late", TRACKS[0]), Some(2));
        assert_eq!(place(&harness, "many", TRACKS[0]), Some(4));
    }

    #[tokio::test]
    async fn timed_out_uploads_are_requeued_periodically() {
        let mut harness = Harness::new("retry", 999);
        harness.queue("peer", TRACKS[0]);
        harness.queue("gone", TRACKS[0]);
        harness.drop_request("peer", false);
        harness.drop_request("gone", true);
        assert_eq!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::Failed(CONNECTION_TIMEOUT.into())
        );

        assert!(harness.sweep().is_empty());
        assert!(!harness.retry_due().is_empty());
        assert_eq!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::GettingStatus
        );
        assert_eq!(
            harness.phase("gone", TRACKS[0]),
            TransferPhase::Failed(USER_OFFLINE.into())
        );

        harness.drop_request("peer", false);
        assert_eq!(harness.abort("peer", TRACKS[0]).len(), 1);
        harness.retry_due();
        assert_eq!(harness.phase("peer", TRACKS[0]), TransferPhase::Aborted);
    }

    #[tokio::test]
    async fn only_local_timeouts_are_retried_and_a_ban_retires_them() {
        let mut harness = Harness::new("retry-ban", 999);
        harness.queue("peer", TRACKS[0]);
        harness.queue("liar", TRACKS[0]);
        let token = harness.uploads.token;
        let _ = harness.uploads.handle_transfer_response(
            "liar",
            token,
            Some(CONNECTION_TIMEOUT),
            &harness.users,
        );
        let cancelled = TransferPhase::Failed(TransferRejectReason::CANCELLED.into());
        assert_eq!(harness.phase("liar", TRACKS[0]), cancelled);
        harness.drop_request("peer", false);

        harness.users.banned.insert("peer".into());
        let updates = harness.uploads.ban("peer", &harness.users);
        assert_eq!(updates.len(), 1);
        harness.users.banned.remove("peer");
        harness.retry_due();
        assert_eq!(
            harness.phase("peer", TRACKS[0]),
            TransferPhase::Failed("Banned".into())
        );
        assert_eq!(harness.phase("liar", TRACKS[0]), cancelled);
    }

    #[tokio::test]
    async fn a_new_index_denies_uploads_whose_path_now_resolves_elsewhere() {
        let mut harness = Harness::new("case", 0);
        let path = "Music\\foo.mp3";
        harness.shares = shares_of("case", &["foo.mp3", "FOO.mp3"]);
        harness.queue("peer", path);
        assert!(
            harness
                .uploads
                .revalidate(&harness.shares, &harness.users)
                .is_empty()
        );

        let filtered = shares_of("case", &["FOO.mp3"]);
        assert!(filtered.resolve(path, false).is_some());
        let updates = harness.uploads.revalidate(&filtered, &harness.users);
        assert_eq!(updates.len(), 1);
        assert_eq!(harness.phase("peer", path), not_shared());
    }
}
