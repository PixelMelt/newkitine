use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::downloads::{TransferIds, TransferPhase};
use super::queue::UploadQueue;
use super::registry::{Registry, TransferKey};
use super::shares::SharesIndex;
use super::speed::SpeedMeter;
use super::users::Users;
use crate::network::NetworkHandle;
use crate::protocol::{PeerMessage, ServerRequest, increment_token, initial_token};
use crate::types::{
    AbortResult, ConnId, FileAttributes, NetworkCommand, Restriction, TransferDirection,
    TransferId, TransferRejectReason, TransferSnapshot, TransferStatus, TransferWork,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

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

    fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            id: self.id,
            direction: TransferDirection::Upload,
            username: self.username.clone(),
            virtual_path: self.virtual_path.clone(),
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

pub struct Uploads {
    net: NetworkHandle,
    upload_slots: usize,
    queue_file_limit: usize,
    transfers: Registry<UploadTransfer>,
    queue: UploadQueue,
    token: u32,
    pub upload_speed: u32,
}

impl Uploads {
    pub fn new(net: NetworkHandle, upload_slots: usize, queue_file_limit: usize) -> Self {
        Self {
            net,
            upload_slots,
            queue_file_limit,
            transfers: Registry::default(),
            queue: UploadQueue::default(),
            token: initial_token(),
            upload_speed: 0,
        }
    }

    pub fn total_slots(&self) -> u32 {
        self.upload_slots as u32
    }

    pub fn set_limits(&mut self, upload_slots: usize, queue_file_limit: usize) {
        self.upload_slots = upload_slots;
        self.queue_file_limit = queue_file_limit;
    }

    pub fn is_new_upload_accepted(&self) -> bool {
        self.transfers.token_count() < self.upload_slots
    }

    pub fn queue_size(&self) -> u32 {
        self.queue.len() as u32
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
        match self.validate_request(username, virtual_path, shares, users) {
            Ok((real_path, size, attributes)) => {
                let updates =
                    self.enqueue(ids, username, virtual_path, real_path, size, attributes);
                self.check_queue(users);
                (updates, true)
            }
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
        match self.validate_request(username, virtual_path, shares, users) {
            Ok((real_path, size, attributes)) => {
                self.net.peer(
                    username,
                    PeerMessage::TransferResponse {
                        token,
                        allowed: false,
                        reason: Some(TransferRejectReason::QUEUED.into()),
                        filesize: None,
                    },
                );
                let updates =
                    self.enqueue(ids, username, virtual_path, real_path, size, attributes);
                self.check_queue(users);
                (updates, true)
            }
            Err(reason) => {
                self.net.peer(
                    username,
                    PeerMessage::TransferResponse {
                        token,
                        allowed: false,
                        reason: Some(reason),
                        filesize: None,
                    },
                );
                (Vec::new(), false)
            }
        }
    }

    fn validate_request(
        &self,
        username: &str,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> Result<(PathBuf, u64, FileAttributes), String> {
        if users.is_banned(username) {
            return Err(TransferRejectReason::BANNED.into());
        }
        if let Some(Restriction::Denied { reason }) = users.restriction(username) {
            return Err(reason.clone());
        }
        let (real_path, size, attributes) = shares
            .and_then(|shares| shares.resolve(virtual_path, users.is_buddy(username)))
            .ok_or(TransferRejectReason::FILE_NOT_SHARED)?;
        if self.queue.queued_for(username) >= self.queue_file_limit {
            return Err(TransferRejectReason::TOO_MANY_FILES.into());
        }
        Ok((real_path.to_owned(), size, attributes.clone()))
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
            Some(existing) if existing.phase.is_active() => {
                debug!(username, virtual_path, "upload already queued");
                return Vec::new();
            }
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

    pub fn check_queue(&mut self, users: &Users) {
        while self.is_new_upload_accepted() {
            let Some(key) = self.queue.select_next(users) else {
                break;
            };
            self.activate(&key);
        }
    }

    pub fn deny_all(&mut self, username: &str, reason: &str, users: &Users) -> Vec<TransferWork> {
        let pending: Vec<TransferKey> = self
            .transfers
            .values()
            .filter(|transfer| transfer.username == username && transfer.phase.is_active())
            .map(UploadTransfer::key)
            .collect();
        let mut updates = Vec::new();
        for key in &pending {
            if let Some(conn_id) = self.transfers.conn_of(key) {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
            self.net.peer(
                key.0.clone(),
                PeerMessage::UploadDenied {
                    file: key.1.clone(),
                    reason: reason.to_owned(),
                },
            );
            updates.extend(self.fail(key, reason.to_owned()));
        }
        if !pending.is_empty() {
            self.check_queue(users);
        }
        updates
    }

    fn activate(&mut self, key: &TransferKey) {
        self.token = increment_token(self.token);
        let token = self.token;
        self.queue.mark_active(key, token);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::GettingStatus;
        transfer.activated_at = Some(Instant::now());
        let username = transfer.username.clone();
        let virtual_path = transfer.virtual_path.clone();
        let size = transfer.size;
        self.transfers.attach_token(key, token);
        info!(username, virtual_path, token, "requesting upload");
        self.net.peer(
            username,
            PeerMessage::TransferRequest {
                direction: TransferDirection::Upload,
                token,
                file: virtual_path,
                filesize: Some(size),
            },
        );
    }

    pub fn handle_transfer_response(
        &mut self,
        username: &str,
        token: u32,
        allowed: bool,
        reason: Option<&str>,
        users: &Users,
    ) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_token(username, token).cloned() else {
            debug!(username, token, "transfer response for unknown upload");
            return Vec::new();
        };
        if let Some(reason) = reason {
            let updates = if reason == TransferRejectReason::COMPLETE {
                self.finish(&key, false)
            } else {
                self.fail(&key, reason.to_owned())
            };
            self.check_queue(users);
            return updates;
        }
        if allowed {
            self.net.send(NetworkCommand::RequestFileConnection {
                username: username.to_owned(),
                token,
            });
        }
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
            .clone()
            .expect("upload activated without a resolved path");
        match fs::File::open(&real_path) {
            Ok(file) => {
                transfer.phase = TransferPhase::Transferring;
                transfer.started_at = Some(Instant::now());
                transfer.speed_bps = 0;
                transfer.speed.reset();
                let size = transfer.size;
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
                self.net.send(NetworkCommand::CloseConnection(conn_id));
                self.net.peer(
                    username,
                    PeerMessage::UploadFailed {
                        file: key.1.clone(),
                    },
                );
                let updates = self.fail(&key, format!("local file error: {error}"));
                self.check_queue(users);
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
        if let Some(conn_id) = self.transfers.conn_of(&key) {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
        self.net.peer(
            username,
            PeerMessage::UploadFailed {
                file: key.1.clone(),
            },
        );
        let updates = self.fail(&key, error.to_owned());
        self.check_queue(users);
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
            token.and_then(|token| self.transfers.key_by_token(username, token).cloned())
        }) {
            Some(key) => key,
            None => return Vec::new(),
        };
        self.transfers.detach_conn(&key);
        let transfer = self.transfers.get(&key).unwrap();
        let updates = match transfer.phase {
            TransferPhase::Transferring if transfer.bytes_done >= transfer.size => {
                self.finish(&key, true)
            }
            TransferPhase::Transferring => self.fail(&key, "connection closed".into()),
            _ => {
                self.deactivate(&key);
                Vec::new()
            }
        };
        self.check_queue(users);
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
                    "connection timeout"
                };
                updates.extend(self.fail(&key, reason.into()));
            }
        }
        if !updates.is_empty() {
            self.check_queue(users);
        }
        updates
    }

    pub fn abort(&mut self, id: TransferId, users: &Users) -> (AbortResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (AbortResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get(&key).unwrap();
        if !transfer.phase.is_active() {
            return (AbortResult::Aborted, Vec::new());
        }
        self.deactivate(&key);
        if let Some(conn_id) = self.transfers.detach_conn(&key) {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
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
        self.check_queue(users);
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
            let (key, detached) = self.transfers.remove(*id).unwrap();
            if let Some(conn_id) = detached.conn_id {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
            self.queue.release(&key, detached.token);
        }
        removed
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

    pub fn handle_place_in_queue_request(&mut self, username: &str, virtual_path: &str) {
        let Some(place) = self.queue.place_of(username, virtual_path) else {
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
            updates.extend(self.fail(key, "request timed out".into()));
        }
        if !expired.is_empty() {
            self.check_queue(users);
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

    fn finish(&mut self, key: &TransferKey, send_speed: bool) -> Vec<TransferWork> {
        self.deactivate(key);
        self.transfers.detach_conn(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Finished;
        transfer.bytes_done = transfer.size;
        let mut avg_speed_bps = None;
        if send_speed && let Some(started_at) = transfer.started_at {
            let elapsed = started_at.elapsed().as_secs_f64();
            let bytes_sent = transfer.bytes_done - transfer.started_offset;
            if elapsed >= 1.0 && bytes_sent > 0 {
                self.upload_speed = (bytes_sent as f64 / elapsed) as u32;
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
        self.transfers.detach_conn(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(reason);
        vec![TransferWork::Update(transfer.snapshot())]
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
    use crate::client::shares;
    use crate::network::spawn as spawn_network;
    use crate::types::SharedFolder;

    #[tokio::test]
    async fn restriction_tiers_control_slot_grants() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500);
        let mut ids = TransferIds::new(&[]);

        let dir =
            std::env::temp_dir().join(format!("newkitine-uploads-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song.mp3"), b"payload").unwrap();
        let shares = shares::scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir.clone(),
                buddy_only: false,
            }],
            &dir.with_extension("slots.cache"),
        )
        .expect("scan test shares");

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_restriction("leech".into(), Restriction::Deprioritized);

        let (_, accepted) = uploads.handle_queue_upload(
            &mut ids,
            "leech",
            "Music\\song.mp3",
            Some(&shares),
            &users,
        );
        assert!(accepted);
        let (_, accepted) = uploads.handle_queue_upload(
            &mut ids,
            "human",
            "Music\\song.mp3",
            Some(&shares),
            &users,
        );
        assert!(accepted);

        uploads.set_limits(1, 500);
        uploads.check_queue(&users);
        assert!(uploads.queue.has_active("human"));
        assert!(!uploads.queue.has_active("leech"));

        users.set_restriction("leech".into(), Restriction::Hold);
        uploads.set_limits(2, 500);
        uploads.check_queue(&users);
        assert!(!uploads.queue.has_active("leech"));

        users.set_restriction("leech".into(), Restriction::None);
        uploads.check_queue(&users);
        assert!(uploads.queue.has_active("leech"));
    }

    #[tokio::test]
    async fn denied_restriction_rejects_queue_requests() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 2, 500);
        let mut ids = TransferIds::new(&[]);

        let dir = std::env::temp_dir().join(format!("newkitine-deny-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song.mp3"), b"payload").unwrap();
        let shares = shares::scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir.clone(),
                buddy_only: false,
            }],
            &dir.with_extension("deny.cache"),
        )
        .expect("scan test shares");

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_restriction(
            "leech".into(),
            Restriction::Denied {
                reason: "not welcome".into(),
            },
        );
        let (updates, accepted) = uploads.handle_queue_upload(
            &mut ids,
            "leech",
            "Music\\song.mp3",
            Some(&shares),
            &users,
        );
        assert!(!accepted);
        assert!(updates.is_empty());
    }
}
