use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info};

use super::files;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use super::{TransferIds, TransferPhase, TransferRejectReason};
use crate::client::users::Users;
use crate::client::{AbortResult, EnqueueResult, RetryResult, TransferWork};
use crate::network::ConnId;
use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::PeerMessage;
use crate::types::{
    FileAttributes, FileInfo, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const QUEUE_POSITION_INTERVAL: Duration = Duration::from_secs(300);
const CONNECTION_RETRY_INTERVAL: Duration = Duration::from_secs(180);
const IO_RETRY_INTERVAL: Duration = Duration::from_secs(900);
const MIN_LIMITED_BATCH: usize = 5;
const RECOVERY_BATCH: usize = 200;

const USER_OFFLINE: &str = "user is offline";
const CONNECTION_TIMEOUT: &str = "connection timeout";
const CONNECTION_CLOSED: &str = "connection closed";
const REQUEST_TIMED_OUT: &str = "request timed out";
const UPLOAD_FAILED: &str = "upload failed";
const LOCAL_FILE_ERROR: &str = "local file error";
const PLACEMENT_ERROR: &str = "cannot place finished download";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    UserOnline,
    Connection,
    Io,
}

fn recovery(phase: &TransferPhase) -> Option<Recovery> {
    let TransferPhase::Failed(reason) = phase else {
        return None;
    };
    match reason.as_str() {
        USER_OFFLINE => Some(Recovery::UserOnline),
        CONNECTION_TIMEOUT
        | CONNECTION_CLOSED
        | REQUEST_TIMED_OUT
        | UPLOAD_FAILED
        | TransferRejectReason::PENDING_SHUTDOWN => Some(Recovery::Connection),
        TransferRejectReason::FILE_READ_ERROR => Some(Recovery::Io),
        reason if reason.starts_with(LOCAL_FILE_ERROR) || reason.starts_with(PLACEMENT_ERROR) => {
            Some(Recovery::Io)
        }
        _ => None,
    }
}

fn parks_offline(phase: &TransferPhase) -> bool {
    match phase {
        TransferPhase::Queued | TransferPhase::Limited | TransferPhase::GettingStatus => true,
        phase => matches!(recovery(phase), Some(Recovery::Connection | Recovery::Io)),
    }
}

fn is_queue_limit(reason: &str) -> bool {
    reason == TransferRejectReason::TOO_MANY_FILES
        || reason == TransferRejectReason::TOO_MANY_MEGABYTES
        || reason.starts_with(TransferRejectReason::USER_LIMIT_PREFIX)
}

fn due(at: &mut Instant, interval: Duration) -> bool {
    if at.elapsed() < interval {
        return false;
    }
    *at = Instant::now();
    true
}

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

    fn needs_watch(&self) -> bool {
        self.phase.is_active() || recovery(&self.phase).is_some()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryStep {
    Request,
    Park,
}

#[derive(Debug, Default)]
struct QueueLimit {
    batch: usize,
    held: VecDeque<TransferKey>,
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
    queue_limits: HashMap<String, QueueLimit>,
    recovery_queue: VecDeque<TransferKey>,
    recovery_pending: HashMap<TransferKey, RecoveryStep>,
    queue_positions_at: Instant,
    connection_retry_at: Instant,
    io_retry_at: Instant,
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
            queue_limits: HashMap::new(),
            recovery_queue: VecDeque::new(),
            recovery_pending: HashMap::new(),
            queue_positions_at: Instant::now(),
            connection_retry_at: Instant::now(),
            io_retry_at: Instant::now(),
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
        users: &mut Users,
        username: String,
        file: FileInfo,
        root: Option<&str>,
    ) -> (EnqueueResult, Vec<TransferWork>) {
        let key = (username, file.name.clone());
        let id = match self.transfers.get(&key) {
            Some(existing) if existing.phase.is_active() => {
                debug!(
                    username = key.0,
                    virtual_path = key.1,
                    "download already in progress"
                );
                return (EnqueueResult::AlreadyActive, Vec::new());
            }
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        let folder_path = self.destination(&key.0, &file.name, root);
        (
            EnqueueResult::Enqueued,
            self.queue(users, id, key.0, file, folder_path),
        )
    }

    fn queue(
        &mut self,
        users: &mut Users,
        id: TransferId,
        username: String,
        file: FileInfo,
        folder_path: PathBuf,
    ) -> Vec<TransferWork> {
        let FileInfo {
            name: virtual_path,
            size,
            attributes,
        } = file;
        let key = (username.clone(), virtual_path.clone());
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
            return vec![finished];
        }
        let queued = TransferWork::Update(transfer.snapshot());
        self.recovery_pending.remove(&key);
        self.transfers.insert(id, key.clone(), transfer);
        users.watch(&username);
        self.send_queue_request(key);
        vec![queued]
    }

    fn requeue(&mut self, users: &mut Users, key: &TransferKey) -> Vec<TransferWork> {
        let transfer = self.transfers.get(key).unwrap();
        let (id, username, folder_path, file) = (
            transfer.id,
            transfer.username.clone(),
            transfer.folder_path.clone(),
            FileInfo {
                name: transfer.virtual_path.clone(),
                size: transfer.size,
                attributes: transfer.attributes.clone(),
            },
        );
        self.queue(users, id, username, file, folder_path)
    }

    pub fn retry(&mut self, users: &mut Users, id: TransferId) -> (RetryResult, Vec<TransferWork>) {
        let Some(key) = self.transfers.key_of(id).cloned() else {
            return (RetryResult::NotFound, Vec::new());
        };
        if self.transfers.get(&key).unwrap().phase.is_active() {
            return (RetryResult::AlreadyActive, Vec::new());
        }
        (RetryResult::Requeued, self.requeue(users, &key))
    }

    pub fn needs_watch(&self, username: &str) -> bool {
        self.transfers
            .values()
            .any(|transfer| transfer.username == username && transfer.needs_watch())
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

    pub fn start_session(&mut self, users: &mut Users) {
        let now = Instant::now();
        self.queue_positions_at = now;
        self.connection_retry_at = now;
        self.io_retry_at = now;
        for transfer in self.transfers.values() {
            if transfer.needs_watch() {
                users.watch(&transfer.username);
            }
        }
        let queued = self.keys_where(|transfer| transfer.phase == TransferPhase::Queued);
        self.schedule(queued, RecoveryStep::Request);
    }

    fn schedule(&mut self, keys: Vec<TransferKey>, step: RecoveryStep) {
        for key in keys {
            if self.recovery_pending.insert(key.clone(), step).is_none() {
                self.recovery_queue.push_back(key);
            }
        }
    }

    fn unschedule_user(&mut self, username: &str) {
        self.recovery_pending.retain(|key, _| key.0 != username);
        self.recovery_queue.retain(|key| key.0 != username);
    }

    pub fn drain_recovery(&mut self, users: &mut Users) -> Vec<TransferWork> {
        let mut updates = Vec::new();
        let mut handled = 0;
        while handled < RECOVERY_BATCH {
            let Some(key) = self.recovery_queue.pop_front() else {
                break;
            };
            let Some(step) = self.recovery_pending.remove(&key) else {
                continue;
            };
            let Some(transfer) = self.transfers.get(&key) else {
                continue;
            };
            match (step, &transfer.phase) {
                (RecoveryStep::Request, TransferPhase::Queued) => self.send_queue_request(key),
                (RecoveryStep::Request, TransferPhase::Limited) => {
                    updates.extend(self.requeue(users, &key));
                }
                (RecoveryStep::Request, phase) if recovery(phase).is_some() => {
                    updates.extend(self.requeue(users, &key));
                }
                (RecoveryStep::Park, phase) if parks_offline(phase) => {
                    updates.push(self.park_offline(&key));
                }
                _ => continue,
            }
            handled += 1;
        }
        updates
    }

    fn park_offline(&mut self, key: &TransferKey) -> TransferWork {
        self.transfers.detach(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(USER_OFFLINE.into());
        transfer.activated_at = None;
        transfer.queue_place = 0;
        TransferWork::Update(transfer.snapshot())
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
        if !due(&mut self.queue_positions_at, QUEUE_POSITION_INTERVAL) {
            return;
        }
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

    fn keys_where(&self, predicate: impl Fn(&Transfer) -> bool) -> Vec<TransferKey> {
        let mut matching: Vec<(TransferId, TransferKey)> = self
            .transfers
            .values()
            .filter(|transfer| predicate(transfer))
            .map(|transfer| (transfer.id, transfer.key()))
            .collect();
        matching.sort_unstable_by_key(|(id, _)| id.0);
        matching.into_iter().map(|(_, key)| key).collect()
    }

    pub fn user_offline(&mut self, username: &str) {
        self.unschedule_user(username);
        let keys = self
            .keys_where(|transfer| transfer.username == username && parks_offline(&transfer.phase));
        self.schedule(keys, RecoveryStep::Park);
    }

    pub fn user_online(&mut self, username: &str) {
        self.unschedule_user(username);
        let keys = self.keys_where(|transfer| {
            transfer.username == username
                && (matches!(
                    transfer.phase,
                    TransferPhase::Queued | TransferPhase::Limited
                ) || recovery(&transfer.phase).is_some())
        });
        self.schedule(keys, RecoveryStep::Request);
    }

    pub fn retry_failed(&mut self) {
        let connection = due(&mut self.connection_retry_at, CONNECTION_RETRY_INTERVAL);
        let io = due(&mut self.io_retry_at, IO_RETRY_INTERVAL);
        if !connection && !io {
            return;
        }
        let keys = self.keys_where(|transfer| match recovery(&transfer.phase) {
            Some(Recovery::Connection) => connection,
            Some(Recovery::Io) => io,
            Some(Recovery::UserOnline) | None => false,
        });
        self.schedule(keys, RecoveryStep::Request);
    }

    pub fn release_limited(&mut self) {
        if self.queue_limits.is_empty() {
            return;
        }
        let busy: HashSet<&str> = self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
            .map(|transfer| transfer.username.as_str())
            .chain(self.recovery_pending.keys().map(|key| key.0.as_str()))
            .collect();
        let drained: Vec<String> = self
            .queue_limits
            .keys()
            .filter(|username| !busy.contains(username.as_str()))
            .cloned()
            .collect();
        for username in drained {
            let limit = self.queue_limits.get_mut(&username).unwrap();
            let mut released = Vec::new();
            while released.len() < limit.batch {
                let Some(key) = limit.held.pop_front() else {
                    break;
                };
                if self
                    .transfers
                    .get(&key)
                    .is_some_and(|transfer| transfer.phase == TransferPhase::Limited)
                {
                    released.push(key);
                }
            }
            if limit.held.is_empty() {
                self.queue_limits.remove(&username);
            }
            self.schedule(released, RecoveryStep::Request);
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
                    TransferPhase::Queued
                        | TransferPhase::Limited
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
        let incomplete_dir = self.incomplete_dir.clone();
        let limit = self.basename_limit(&incomplete_dir);
        let incomplete_path = files::incomplete_file_path(&incomplete_dir, username, &key.1, limit);
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
                self.fail(&key, format!("{LOCAL_FILE_ERROR}: {error}"))
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
        self.fail(&key, format!("{LOCAL_FILE_ERROR}: {error}"))
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: Option<u32>,
        conn_id: ConnId,
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
        match transfer.phase {
            TransferPhase::Transferring => self.fail(&key, CONNECTION_CLOSED.into()),
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
            self.send_queue_request(key);
            return Vec::new();
        }
        if is_queue_limit(reason) && transfer.phase == TransferPhase::Queued {
            info!(
                username,
                virtual_path = file,
                reason,
                "uploader queue limit reached, holding download until the queue drains"
            );
            let queued = self
                .transfers
                .values()
                .filter(|transfer| {
                    transfer.username == username && transfer.phase == TransferPhase::Queued
                })
                .count();
            let limit = self.queue_limits.entry(username.to_owned()).or_default();
            limit.batch = queued.saturating_sub(1).max(MIN_LIMITED_BATCH);
            limit.held.push_back(key.clone());
            self.transfers.detach(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Limited;
            transfer.queue_place = 0;
            return vec![TransferWork::Update(transfer.snapshot())];
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
            self.send_queue_request(key);
            return Vec::new();
        }
        self.fail(&key, UPLOAD_FAILED.into())
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
                    USER_OFFLINE
                } else {
                    CONNECTION_TIMEOUT
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
            updates.extend(self.fail(&key, REQUEST_TIMED_OUT.into()));
        }
        updates
    }

    pub fn reset(&mut self) -> Vec<TransferWork> {
        self.queue_limits.clear();
        self.recovery_queue.clear();
        self.recovery_pending.clear();
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
        let destination_dir = transfer.folder_path.clone();
        let virtual_path = transfer.virtual_path.clone();
        let limit = self.basename_limit(&destination_dir);
        let basename = files::download_basename(&virtual_path, limit);
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
                    delivered_bytes: transfer.size,
                }]
            }
            Err(error) => self.fail(key, format!("{PLACEMENT_ERROR}: {error}")),
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
    use crate::protocol::ServerRequest;
    use std::collections::HashSet;

    fn no_users() -> Users {
        Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new())
    }

    fn queued_download(downloads: &mut Downloads, ids: &mut TransferIds) -> TransferKey {
        let (result, _) = downloads.enqueue(
            ids,
            &mut no_users(),
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
            &mut no_users(),
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
            &mut no_users(),
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
            &mut no_users(),
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

    fn drain(commands: &mut mpsc::Receiver<NetworkCommand>) -> Vec<NetworkCommand> {
        let mut drained = Vec::new();
        while let Ok(command) = commands.try_recv() {
            drained.push(command);
        }
        drained
    }

    fn server_requests(commands: &[NetworkCommand]) -> Vec<&ServerRequest> {
        commands
            .iter()
            .filter_map(|command| match command {
                NetworkCommand::SendServerMessage(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    fn queue_uploads(commands: &[NetworkCommand]) -> Vec<&str> {
        commands
            .iter()
            .filter_map(|command| match command {
                NetworkCommand::SendPeerMessage {
                    message: PeerMessage::QueueUpload { file, .. },
                    ..
                } => Some(file.as_str()),
                _ => None,
            })
            .collect()
    }

    fn recording_downloads() -> (Downloads, mpsc::Receiver<NetworkCommand>) {
        let (net, commands) = crate::network::test_channel();
        let downloads = Downloads::new(
            net,
            "/nonexistent-downloads".into(),
            "/nonexistent-incomplete".into(),
            false,
            mpsc::unbounded_channel().0,
        );
        (downloads, commands)
    }

    fn enqueue_file(
        downloads: &mut Downloads,
        ids: &mut TransferIds,
        users: &mut Users,
        username: &str,
        name: &str,
    ) -> TransferKey {
        let (result, _) = downloads.enqueue(
            ids,
            users,
            username.into(),
            FileInfo {
                name: name.into(),
                size: 100,
                attributes: FileAttributes::default(),
            },
            None,
        );
        assert_eq!(result, EnqueueResult::Enqueued);
        (username.into(), name.into())
    }

    fn phase(downloads: &Downloads, key: &TransferKey) -> TransferPhase {
        downloads.transfers.get(key).unwrap().phase.clone()
    }

    #[tokio::test]
    async fn a_folder_enqueue_watches_the_uploader_once() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        for index in 0..20 {
            enqueue_file(
                &mut downloads,
                &mut ids,
                &mut users,
                "uploader",
                &format!("Music\\{index}.mp3"),
            );
        }
        assert!(server_requests(&drain(&mut commands)).is_empty());
        users.send_watches(&downloads.net);
        users.send_watches(&downloads.net);
        let sent = drain(&mut commands);
        let requests = server_requests(&sent);
        assert_eq!(
            requests,
            vec![
                &ServerRequest::WatchUser {
                    user: "uploader".into()
                },
                &ServerRequest::GetUserStatus {
                    user: "uploader".into()
                },
            ]
        );
        assert!(queue_uploads(&sent).is_empty());
    }

    #[tokio::test]
    async fn offline_uploader_parks_pending_downloads_and_resumes_them_on_return() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let queued = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\1.mp3");
        let dropped = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\2.mp3");
        let denied = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\3.mp3");
        let _ = downloads.handle_peer_connection_error(
            "uploader",
            &[PeerMessage::QueueUpload {
                file: dropped.1.clone(),
                legacy_client: false,
            }],
            false,
        );
        let _ =
            downloads.handle_upload_denied("uploader", &denied.1, TransferRejectReason::CANCELLED);
        drain(&mut commands);

        downloads.user_offline("uploader");
        let updates = downloads.drain_recovery(&mut users);
        assert_eq!(updates.len(), 2);
        assert_eq!(
            phase(&downloads, &queued),
            TransferPhase::Failed(USER_OFFLINE.into())
        );
        assert_eq!(
            phase(&downloads, &dropped),
            TransferPhase::Failed(USER_OFFLINE.into())
        );
        assert_eq!(
            phase(&downloads, &denied),
            TransferPhase::Failed(TransferRejectReason::CANCELLED.into())
        );

        downloads.user_online("uploader");
        assert!(queue_uploads(&drain(&mut commands)).is_empty());
        let updates = downloads.drain_recovery(&mut users);
        assert!(
            updates
                .iter()
                .all(|work| matches!(work, TransferWork::Update(snapshot) if snapshot.status == TransferStatus::Queued))
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(phase(&downloads, &queued), TransferPhase::Queued);
        assert_eq!(phase(&downloads, &dropped), TransferPhase::Queued);
        assert_eq!(
            queue_uploads(&drain(&mut commands)),
            vec![queued.1.as_str(), dropped.1.as_str()]
        );
    }

    #[tokio::test]
    async fn a_new_session_watches_users_with_recoverable_downloads_only() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let offline = enqueue_file(&mut downloads, &mut ids, &mut users, "away", "a\\1.mp3");
        let _ = downloads.handle_peer_connection_error(
            "away",
            &[PeerMessage::QueueUpload {
                file: offline.1.clone(),
                legacy_client: false,
            }],
            true,
        );
        let rejected = enqueue_file(&mut downloads, &mut ids, &mut users, "rejecter", "b\\1.mp3");
        let _ = downloads.handle_upload_denied(
            "rejecter",
            &rejected.1,
            TransferRejectReason::FILE_NOT_SHARED,
        );
        let _ = downloads.handle_upload_denied(
            "rejecter",
            &rejected.1,
            TransferRejectReason::FILE_NOT_SHARED,
        );
        assert!(matches!(
            phase(&downloads, &rejected),
            TransferPhase::Failed(_)
        ));
        drain(&mut commands);

        let fresh_key = enqueue_file(&mut downloads, &mut ids, &mut users, "fresh", "c\\1.mp3");
        drain(&mut commands);

        let mut fresh = no_users();
        downloads.start_session(&mut fresh);
        fresh.send_watches(&downloads.net);
        let sent = drain(&mut commands);
        assert!(queue_uploads(&sent).is_empty());
        assert!(downloads.drain_recovery(&mut fresh).is_empty());
        assert_eq!(
            queue_uploads(&drain(&mut commands)),
            vec![fresh_key.1.as_str()]
        );
        let watched: HashSet<&str> = server_requests(&sent)
            .into_iter()
            .filter_map(|request| match request {
                ServerRequest::WatchUser { user } => Some(user.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(watched, HashSet::from(["away", "fresh"]));
        assert!(downloads.needs_watch("away"));
        assert!(!downloads.needs_watch("rejecter"));
    }

    #[tokio::test]
    async fn queue_limit_denials_hold_downloads_until_the_queue_drains() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let keys: Vec<TransferKey> = (0..8)
            .map(|index| {
                enqueue_file(
                    &mut downloads,
                    &mut ids,
                    &mut users,
                    "uploader",
                    &format!("a\\{index}.mp3"),
                )
            })
            .collect();
        for key in &keys[1..] {
            let updates = downloads.handle_upload_denied(
                "uploader",
                &key.1,
                TransferRejectReason::TOO_MANY_FILES,
            );
            assert!(matches!(
                updates.as_slice(),
                [TransferWork::Update(snapshot)] if snapshot.status == TransferStatus::Queued
            ));
            assert_eq!(phase(&downloads, key), TransferPhase::Limited);
        }
        drain(&mut commands);

        downloads.release_limited();
        assert!(downloads.drain_recovery(&mut users).is_empty());

        downloads.handle_transfer_request("uploader", 7, &keys[0].1, Some(100));
        drain(&mut commands);
        downloads.release_limited();
        let released = downloads.drain_recovery(&mut users);
        assert_eq!(released.len(), MIN_LIMITED_BATCH);
        let sent = drain(&mut commands);
        let requested = queue_uploads(&sent);
        let expected: Vec<&str> = keys[1..=MIN_LIMITED_BATCH]
            .iter()
            .map(|key| key.1.as_str())
            .collect();
        assert_eq!(requested, expected);
        downloads.release_limited();
        assert!(downloads.drain_recovery(&mut users).is_empty());

        for key in &keys[1..=MIN_LIMITED_BATCH] {
            let _ = downloads.handle_upload_denied(
                "uploader",
                &key.1,
                TransferRejectReason::TOO_MANY_MEGABYTES,
            );
        }
        downloads.release_limited();
        let released = downloads.drain_recovery(&mut users);
        assert_eq!(released.len(), MIN_LIMITED_BATCH);
        let sent = drain(&mut commands);
        let rotated: Vec<&str> = keys[MIN_LIMITED_BATCH + 1..]
            .iter()
            .chain(&keys[1..=3])
            .map(|key| key.1.as_str())
            .collect();
        assert_eq!(queue_uploads(&sent), rotated);
    }

    #[tokio::test]
    async fn connection_and_io_failures_retry_on_their_timers_and_rejections_do_not() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let timed_out = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\1.mp3");
        let shutdown = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\2.mp3");
        let io = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\3.mp3");
        let rejected = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\4.mp3");
        let offline = enqueue_file(&mut downloads, &mut ids, &mut users, "gone", "b\\1.mp3");
        let _ = downloads.handle_peer_connection_error(
            "uploader",
            &[PeerMessage::QueueUpload {
                file: timed_out.1.clone(),
                legacy_client: false,
            }],
            false,
        );
        let _ = downloads.handle_upload_denied(
            "uploader",
            &shutdown.1,
            TransferRejectReason::PENDING_SHUTDOWN,
        );
        downloads.handle_transfer_request("uploader", 9, &io.1, Some(100));
        let _ = downloads.handle_transfer_error("uploader", 9, "No space left on device");
        let _ = downloads.handle_upload_denied("uploader", &rejected.1, "Banned");
        let _ = downloads.handle_peer_connection_error(
            "gone",
            &[PeerMessage::QueueUpload {
                file: offline.1.clone(),
                legacy_client: false,
            }],
            true,
        );
        drain(&mut commands);

        downloads.start_session(&mut users);
        drain(&mut commands);
        downloads.retry_failed();
        assert!(downloads.drain_recovery(&mut users).is_empty());

        let past = |interval: Duration| {
            Instant::now()
                .checked_sub(interval + Duration::from_secs(1))
                .unwrap()
        };
        downloads.connection_retry_at = past(CONNECTION_RETRY_INTERVAL);
        downloads.retry_failed();
        let retried = downloads.drain_recovery(&mut users);
        assert_eq!(retried.len(), 2);
        assert_eq!(
            queue_uploads(&drain(&mut commands)),
            vec![timed_out.1.as_str(), shutdown.1.as_str()]
        );
        assert!(matches!(phase(&downloads, &io), TransferPhase::Failed(_)));

        downloads.io_retry_at = past(IO_RETRY_INTERVAL);
        downloads.retry_failed();
        let retried = downloads.drain_recovery(&mut users);
        assert_eq!(retried.len(), 1);
        assert_eq!(queue_uploads(&drain(&mut commands)), vec![io.1.as_str()]);
        assert_eq!(
            phase(&downloads, &rejected),
            TransferPhase::Failed("Banned".into())
        );
        assert_eq!(
            phase(&downloads, &offline),
            TransferPhase::Failed(USER_OFFLINE.into())
        );
    }

    #[tokio::test]
    async fn recovery_is_paced_and_dropped_when_the_uploader_leaves() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let total = RECOVERY_BATCH + 50;
        for index in 0..total {
            let key = enqueue_file(
                &mut downloads,
                &mut ids,
                &mut users,
                "uploader",
                &format!("a\\{index}.mp3"),
            );
            let _ = downloads.handle_peer_connection_error(
                "uploader",
                &[PeerMessage::QueueUpload {
                    file: key.1,
                    legacy_client: false,
                }],
                true,
            );
            drain(&mut commands);
        }
        downloads.user_online("uploader");
        assert_eq!(downloads.drain_recovery(&mut users).len(), RECOVERY_BATCH);
        assert_eq!(queue_uploads(&drain(&mut commands)).len(), RECOVERY_BATCH);
        downloads.user_offline("uploader");
        assert_eq!(downloads.drain_recovery(&mut users).len(), RECOVERY_BATCH);
        assert!(downloads.drain_recovery(&mut users).is_empty());
        assert!(queue_uploads(&drain(&mut commands)).is_empty());
        downloads.user_online("uploader");
        assert_eq!(downloads.drain_recovery(&mut users).len(), RECOVERY_BATCH);
        assert_eq!(downloads.drain_recovery(&mut users).len(), 50);
    }

    #[tokio::test]
    async fn a_manual_retry_cancels_the_pending_automatic_one() {
        let (mut downloads, mut commands) = recording_downloads();
        let mut ids = TransferIds::new(&[]);
        let mut users = no_users();
        let key = enqueue_file(&mut downloads, &mut ids, &mut users, "uploader", "a\\1.mp3");
        let _ = downloads.handle_peer_connection_error(
            "uploader",
            &[PeerMessage::QueueUpload {
                file: key.1.clone(),
                legacy_client: false,
            }],
            true,
        );
        downloads.user_online("uploader");
        let id = downloads.transfers.get(&key).unwrap().id;
        let (result, _) = downloads.retry(&mut users, id);
        assert_eq!(result, RetryResult::Requeued);
        let _ = downloads.handle_upload_denied(
            "uploader",
            &key.1,
            TransferRejectReason::TOO_MANY_FILES,
        );
        drain(&mut commands);
        assert!(downloads.drain_recovery(&mut users).is_empty());
        assert!(queue_uploads(&drain(&mut commands)).is_empty());
        assert_eq!(phase(&downloads, &key), TransferPhase::Limited);
    }
}
