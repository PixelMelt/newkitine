use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::files;
use super::registry::{Registry, TransferKey};
use super::speed::SpeedMeter;
use super::{
    CONNECTION_CLOSED, CONNECTION_TIMEOUT, LOCAL_FILE_ERROR, TRANSFER_REQUEST_TIMEOUT, TransferIds,
    TransferPhase, TransferRejectReason, USER_OFFLINE, VIRTUAL_PATH_LIMIT, peer_reason,
};
use crate::client::users::Users;
use crate::client::{AbortResult, EnqueueResult, RetryResult, TransferWork};
use crate::network::ConnId;
use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::PeerMessage;
use crate::types::{
    FileAttributes, FileInfo, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

const QUEUE_POSITION_INTERVAL: Duration = Duration::from_secs(300);
const CONNECTION_RETRY_INTERVAL: Duration = Duration::from_secs(180);
const IO_RETRY_INTERVAL: Duration = Duration::from_secs(900);
const MIN_LIMITED_BATCH: usize = 5;
const RECOVERY_BATCH: usize = 200;

const REQUEST_TIMED_OUT: &str = "request timed out";
const UPLOAD_FAILED: &str = "upload failed";
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
        reason if is_local_reason(reason) => Some(Recovery::Io),
        _ => None,
    }
}

fn is_local_reason(reason: &str) -> bool {
    reason.starts_with(LOCAL_FILE_ERROR) || reason.starts_with(PLACEMENT_ERROR)
}

fn is_internal_reason(reason: &str) -> bool {
    matches!(
        reason,
        USER_OFFLINE | CONNECTION_TIMEOUT | CONNECTION_CLOSED | REQUEST_TIMED_OUT | UPLOAD_FAILED
    ) || is_local_reason(reason)
}

fn resumes_online(phase: &TransferPhase) -> bool {
    matches!(phase, TransferPhase::Queued | TransferPhase::Limited) || recovery(phase).is_some()
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
    retry_attempt: bool,
    legacy_attempt: bool,
    size_changed: bool,
    incomplete_path: Option<PathBuf>,
    activated_at: Option<Instant>,
    speed: SpeedMeter,
}

impl Transfer {
    fn new(
        id: TransferId,
        username: String,
        virtual_path: String,
        folder_path: PathBuf,
        size: u64,
        attributes: FileAttributes,
    ) -> Self {
        Self {
            id,
            username,
            virtual_path,
            folder_path,
            size,
            attributes,
            phase: TransferPhase::Queued,
            bytes_done: 0,
            file_path: None,
            queue_place: 0,
            retry_attempt: false,
            legacy_attempt: false,
            size_changed: false,
            incomplete_path: None,
            activated_at: None,
            speed: SpeedMeter::default(),
        }
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
                self.speed.bps()
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
        let mut transfer = Transfer::new(
            seed.id,
            seed.username,
            seed.virtual_path,
            folder_path,
            seed.size,
            seed.attributes,
        );
        transfer.phase = phase;
        transfer.bytes_done = seed.bytes_done;
        transfer.file_path = seed.file_path;
        self.transfers.insert(seed.id, key, transfer);
    }

    pub fn enqueue(
        &mut self,
        ids: &mut TransferIds,
        users: &mut Users,
        username: String,
        file: FileInfo,
        root: Option<&str>,
    ) -> (EnqueueResult, Vec<TransferWork>) {
        if file.name.len() > VIRTUAL_PATH_LIMIT {
            return (EnqueueResult::PathTooLong, Vec::new());
        }
        let Some(id) = self.admissible_id(ids, &username, &file.name) else {
            return (EnqueueResult::AlreadyActive, Vec::new());
        };
        let folder_path = self.destination(&username, &file.name, root);
        (
            EnqueueResult::Enqueued,
            self.queue(users, id, username, file, folder_path),
        )
    }

    pub fn enqueue_folder(
        &mut self,
        ids: &mut TransferIds,
        users: &mut Users,
        username: String,
        files: Vec<FileInfo>,
        root: &str,
    ) -> Vec<TransferWork> {
        let mut work = Vec::with_capacity(files.len());
        let mut requests = Vec::new();
        for file in files {
            if file.name.len() > VIRTUAL_PATH_LIMIT {
                warn!(username, "skipping a folder file whose path is too long");
                continue;
            }
            let Some(id) = self.admissible_id(ids, &username, &file.name) else {
                continue;
            };
            let folder_path = self.destination(&username, &file.name, Some(root));
            let virtual_path = file.name.clone();
            let admitted = self.admit(id, username.clone(), file, folder_path);
            if matches!(admitted, TransferWork::Update(_)) {
                requests.push(PeerMessage::QueueUpload {
                    file: virtual_path,
                    legacy_client: false,
                });
            }
            work.push(admitted);
        }
        if !requests.is_empty() {
            users.watch(&username);
            self.net.send(NetworkCommand::SendPeerMessages {
                username,
                messages: requests,
            });
        }
        work
    }

    fn admissible_id(
        &self,
        ids: &mut TransferIds,
        username: &str,
        virtual_path: &str,
    ) -> Option<TransferId> {
        match self
            .transfers
            .get(&(username.to_owned(), virtual_path.to_owned()))
        {
            Some(existing) if existing.phase.is_active() => {
                debug!(username, virtual_path, "download already in progress");
                None
            }
            Some(existing) => Some(existing.id),
            None => Some(ids.mint()),
        }
    }

    fn queue(
        &mut self,
        users: &mut Users,
        id: TransferId,
        username: String,
        file: FileInfo,
        folder_path: PathBuf,
    ) -> Vec<TransferWork> {
        let key = (username.clone(), file.name.clone());
        let admitted = self.admit(id, username, file, folder_path);
        if matches!(admitted, TransferWork::Update(_)) {
            users.watch(&key.0);
            self.send_queue_request(key);
        }
        vec![admitted]
    }

    fn admit(
        &mut self,
        id: TransferId,
        username: String,
        file: FileInfo,
        folder_path: PathBuf,
    ) -> TransferWork {
        let key = (username.clone(), file.name.clone());
        self.recovery_pending.remove(&key);
        let limit = self.basename_limit(&folder_path);
        let basename = files::download_basename(&file.name, limit);
        let downloaded = files::complete_file_path(&folder_path, &basename, file.size);
        let mut transfer = Transfer::new(
            id,
            username,
            file.name,
            folder_path,
            file.size,
            file.attributes,
        );
        let admitted = match downloaded {
            Some(destination) => {
                info!(
                    username = key.0,
                    virtual_path = key.1,
                    ?destination,
                    "file is already downloaded"
                );
                transfer.phase = TransferPhase::Finished;
                transfer.bytes_done = transfer.size;
                transfer.file_path = Some(destination.display().to_string());
                TransferWork::Finished {
                    snapshot: transfer.snapshot(),
                    avg_speed_bps: None,
                    delivered_bytes: 0,
                }
            }
            None => TransferWork::Update(transfer.snapshot()),
        };
        self.transfers.insert(id, key, transfer);
        admitted
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
        if !transfer.phase.is_active() && recovery(&transfer.phase).is_none() {
            return (AbortResult::Aborted, Vec::new());
        }
        transfer.phase = TransferPhase::Aborted;
        let aborted = TransferWork::Update(transfer.snapshot());
        self.release(&key);
        (AbortResult::Aborted, vec![aborted])
    }

    pub fn clear(&mut self, statuses: Option<&[TransferStatus]>) -> Vec<TransferId> {
        let removed: Vec<TransferId> = self
            .transfers
            .values()
            .filter(|transfer| {
                statuses.is_none_or(|statuses| statuses.contains(&transfer.phase.status()))
            })
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
        let queued = self
            .transfers
            .keys_where(|transfer| transfer.phase == TransferPhase::Queued);
        self.schedule(queued, RecoveryStep::Request);
    }

    fn schedule(&mut self, keys: Vec<TransferKey>, step: RecoveryStep) {
        for key in keys {
            if let Entry::Vacant(pending) = self.recovery_pending.entry(key.clone()) {
                pending.insert(step);
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
                (RecoveryStep::Request, phase) if resumes_online(phase) => {
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
        self.release(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.phase = TransferPhase::Failed(USER_OFFLINE.into());
        transfer.activated_at = None;
        transfer.queue_place = 0;
        transfer.legacy_attempt = false;
        transfer.size_changed = false;
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
        let mut requests: HashMap<String, Vec<PeerMessage>> = HashMap::new();
        for transfer in self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
        {
            requests.entry(transfer.username.clone()).or_default().push(
                PeerMessage::PlaceInQueueRequest {
                    file: transfer.virtual_path.clone(),
                    legacy_client: transfer.legacy_attempt,
                },
            );
        }
        for (username, messages) in requests {
            self.net
                .send(NetworkCommand::SendPeerMessages { username, messages });
        }
    }

    pub fn user_offline(&mut self, username: &str) {
        self.unschedule_user(username);
        let keys = self
            .transfers
            .keys_where(|transfer| transfer.username == username && parks_offline(&transfer.phase));
        self.schedule(keys, RecoveryStep::Park);
    }

    pub fn user_online(&mut self, username: &str) {
        self.unschedule_user(username);
        let keys = self.transfers.keys_where(|transfer| {
            transfer.username == username && resumes_online(&transfer.phase)
        });
        self.schedule(keys, RecoveryStep::Request);
    }

    pub fn retry_failed(&mut self) {
        let connection = due(&mut self.connection_retry_at, CONNECTION_RETRY_INTERVAL);
        let io = due(&mut self.io_retry_at, IO_RETRY_INTERVAL);
        if !connection && !io {
            return;
        }
        let keys = self
            .transfers
            .keys_where(|transfer| match recovery(&transfer.phase) {
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
        if transfer.phase != TransferPhase::Queued {
            return None;
        }
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
        let accepted = match self.transfers.get_mut(&key) {
            Some(transfer)
                if !token_taken
                    && matches!(
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
                    if transfer.size != size {
                        transfer.size_changed = true;
                    }
                    transfer.size = size;
                }
                transfer.phase = TransferPhase::GettingStatus;
                transfer.activated_at = Some(Instant::now());
                transfer.queue_place = 0;
                Ok(TransferWork::Update(transfer.snapshot()))
            }
            Some(transfer) if transfer.phase == TransferPhase::Finished => {
                Err(TransferRejectReason::COMPLETE)
            }
            _ => Err(TransferRejectReason::CANCELLED),
        };
        if accepted.is_ok() {
            self.transfers.attach_token(&key, token);
        }
        self.net.peer(
            username,
            PeerMessage::TransferResponse {
                token,
                allowed: accepted.is_ok(),
                reason: accepted.as_ref().err().map(|reason| (*reason).to_owned()),
                filesize: None,
            },
        );
        accepted.into_iter().collect()
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
            Err(error) => self.fail(&key, format!("{LOCAL_FILE_ERROR}: {error}")),
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
        transfer.speed.sample(transfer.bytes_done);
        vec![TransferWork::Progress(transfer.snapshot())]
    }

    pub fn handle_transfer_error(&mut self, conn_id: ConnId, error: &str) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned() else {
            return Vec::new();
        };
        self.fail(&key, format!("{LOCAL_FILE_ERROR}: {error}"))
    }

    pub fn handle_file_connection_closed(&mut self, conn_id: ConnId) -> Vec<TransferWork> {
        let Some(key) = self.transfers.key_by_conn(conn_id).cloned() else {
            return Vec::new();
        };
        self.transfers.detach_conn(&key);
        match self.transfers.get(&key).unwrap().phase {
            TransferPhase::Transferring => self.fail(&key, CONNECTION_CLOSED.into()),
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
        match transfer.phase {
            TransferPhase::Queued => {}
            TransferPhase::Limited if !is_queue_limit(reason) => {}
            _ => return Vec::new(),
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
            self.transfers.get_mut(&key).unwrap().legacy_attempt = true;
            self.send_queue_request(key);
            return Vec::new();
        }
        if is_queue_limit(reason) {
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
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Limited;
            transfer.queue_place = 0;
            return vec![TransferWork::Update(transfer.snapshot())];
        }
        let reason = peer_reason(reason);
        if is_internal_reason(&reason) {
            return self.fail(&key, TransferRejectReason::CANCELLED.into());
        }
        self.fail(&key, reason)
    }

    pub fn handle_upload_failed(&mut self, username: &str, file: &str) -> Vec<TransferWork> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Placing | TransferPhase::Finished | TransferPhase::Aborted
        ) || matches!(&transfer.phase, TransferPhase::Failed(reason) if is_local_reason(reason))
        {
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
        let updates = self.fail(&key, UPLOAD_FAILED.into());
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
            USER_OFFLINE
        } else {
            CONNECTION_TIMEOUT
        };
        let mut updates = Vec::new();
        for message in unsent {
            if let PeerMessage::QueueUpload { file, .. }
            | PeerMessage::PlaceInQueueRequest { file, .. } = message
            {
                let key = (username.to_owned(), file.clone());
                if self.transfers.get(&key).is_some_and(|transfer| {
                    matches!(
                        transfer.phase,
                        TransferPhase::Queued | TransferPhase::Limited
                    )
                }) {
                    updates.extend(self.fail(&key, reason.into()));
                }
            }
        }
        updates
    }

    pub fn sweep_request_timeouts(&mut self) -> Vec<TransferWork> {
        let expired = self.transfers.keys_where(|transfer| {
            transfer.phase == TransferPhase::GettingStatus
                && transfer
                    .activated_at
                    .is_some_and(|at| at.elapsed() > TRANSFER_REQUEST_TIMEOUT)
        });
        expired
            .iter()
            .flat_map(|key| self.fail(key, REQUEST_TIMED_OUT.into()))
            .collect()
    }

    pub fn reset(&mut self) -> Vec<TransferWork> {
        self.queue_limits.clear();
        self.recovery_queue.clear();
        self.recovery_pending.clear();
        let active = self.transfers.keys_where(|transfer| {
            matches!(
                transfer.phase,
                TransferPhase::GettingStatus | TransferPhase::Transferring
            )
        });
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
            Err(error) => self.fail(key, format!("{PLACEMENT_ERROR}: {error}")),
        }
    }

    fn release(&mut self, key: &TransferKey) {
        if let Some(conn_id) = self.transfers.detach(key).conn_id {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
    }

    fn fail(&mut self, key: &TransferKey, reason: String) -> Vec<TransferWork> {
        let active = |transfer: &Transfer| transfer.phase.is_active();
        if !self.transfers.get(key).is_some_and(active) {
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
    use crate::protocol::ServerRequest;

    struct Harness {
        downloads: Downloads,
        commands: mpsc::Receiver<NetworkCommand>,
        ids: TransferIds,
        users: Users,
        dir: PathBuf,
    }

    fn no_users() -> Users {
        Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new())
    }

    impl Harness {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("soulseek-{name}-{}", std::process::id()));
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
                users: no_users(),
                dir,
            }
        }

        fn enqueue(
            &mut self,
            username: &str,
            file: &str,
            root: Option<&str>,
        ) -> (EnqueueResult, Vec<TransferWork>) {
            self.downloads.enqueue(
                &mut self.ids,
                &mut self.users,
                username.into(),
                FileInfo {
                    name: file.into(),
                    size: 100,
                    attributes: FileAttributes::default(),
                },
                root,
            )
        }

        fn queue_from(&mut self, username: &str, file: &str) -> TransferKey {
            assert_eq!(
                self.enqueue(username, file, None).0,
                EnqueueResult::Enqueued
            );
            (username.into(), file.into())
        }

        fn queue(&mut self, file: &str) -> TransferKey {
            self.queue_from("uploader", file)
        }

        fn request(&mut self, file: &str, token: u32, size: u64) -> Vec<TransferWork> {
            self.downloads
                .handle_transfer_request("uploader", token, file, Some(size))
        }

        fn start(&mut self, file: &str, token: u32, conn_id: ConnId) -> TransferKey {
            let key = (String::from("uploader"), String::from(file));
            assert_eq!(self.request(file, token, 100).len(), 1);
            let started = self
                .downloads
                .handle_file_transfer_init("uploader", token, conn_id);
            assert_eq!(started.len(), 1);
            assert_eq!(self.phase(&key), TransferPhase::Transferring);
            key
        }

        fn deny(&mut self, key: &TransferKey, reason: &str) -> Vec<TransferWork> {
            self.downloads.handle_upload_denied(&key.0, &key.1, reason)
        }

        fn transfer(&self, key: &TransferKey) -> &Transfer {
            self.downloads.transfers.get(key).unwrap()
        }

        fn phase(&self, key: &TransferKey) -> TransferPhase {
            self.transfer(key).phase.clone()
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

        fn recover(&mut self) -> Vec<TransferWork> {
            self.downloads.drain_recovery(&mut self.users)
        }

        fn send_watches(&mut self) {
            self.users.send_watches(&self.downloads.net);
        }

        fn fail_connection(&mut self, key: &TransferKey, is_offline: bool) -> Vec<TransferWork> {
            self.downloads.handle_peer_connection_error(
                &key.0,
                &[PeerMessage::QueueUpload {
                    file: key.1.clone(),
                    legacy_client: false,
                }],
                is_offline,
            )
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn failed(reason: &str) -> TransferPhase {
        TransferPhase::Failed(reason.into())
    }

    fn past(interval: Duration) -> Instant {
        Instant::now()
            .checked_sub(interval + Duration::from_secs(1))
            .unwrap()
    }

    fn statuses(work: &[TransferWork]) -> Vec<TransferStatus> {
        work.iter()
            .map(|item| match item {
                TransferWork::Update(snapshot) => snapshot.status,
                other => panic!("expected an update, got {other:?}"),
            })
            .collect()
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

    fn assert_watches_the_uploader_once(commands: &[NetworkCommand]) {
        assert_eq!(
            server_requests(commands),
            vec![
                &ServerRequest::WatchUser {
                    user: "uploader".into()
                },
                &ServerRequest::GetUserStatus {
                    user: "uploader".into()
                },
            ]
        );
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

    #[tokio::test]
    async fn folder_downloads_keep_the_remote_hierarchy() {
        let mut harness = Harness::new("hierarchy");
        harness
            .downloads
            .set_dirs("/downloads".into(), "/incomplete".into(), true);
        let file = "share\\Soulseek\\folder1\\sub1\\file4.mp3";
        let (result, _) = harness.enqueue("uploader", file, Some("share\\Soulseek"));
        assert_eq!(result, EnqueueResult::Enqueued);
        assert_eq!(
            harness
                .transfer(&("uploader".into(), file.into()))
                .folder_path,
            PathBuf::from("/downloads/uploader/Soulseek/folder1/sub1")
        );
    }

    fn enqueue_beside_existing(name: &str, existing_len: usize) -> (Harness, Vec<TransferWork>) {
        let mut harness = Harness::new(name);
        let destination = harness.dir.join("complete/Album");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("song.mp3"), vec![0u8; existing_len]).unwrap();
        let (result, work) =
            harness.enqueue("uploader", "share\\Album\\song.mp3", Some("share\\Album"));
        assert_eq!(result, EnqueueResult::Enqueued);
        (harness, work)
    }

    #[tokio::test]
    async fn an_already_downloaded_file_finishes_without_asking_the_peer() {
        let (mut harness, work) = enqueue_beside_existing("skip", 100);
        assert!(matches!(
            work.as_slice(),
            [TransferWork::Finished {
                delivered_bytes: 0,
                ..
            }]
        ));
        let key = ("uploader".to_owned(), "share\\Album\\song.mp3".to_owned());
        let transfer = harness.transfer(&key);
        assert_eq!(transfer.phase, TransferPhase::Finished);
        assert_eq!(
            transfer.file_path.as_deref(),
            harness.dir.join("complete/Album/song.mp3").to_str()
        );
        assert!(harness.drain().is_empty());
    }

    #[tokio::test]
    async fn a_size_mismatch_still_downloads() {
        let (_harness, work) = enqueue_beside_existing("mismatch", 64);
        assert!(matches!(work.as_slice(), [TransferWork::Update(_)]));
    }

    #[tokio::test]
    async fn late_denial_does_not_revive_aborted_transfer() {
        let mut harness = Harness::new("late-denial");
        let key = harness.queue("Music\\song.mp3");
        let id = harness.transfer(&key).id;
        let (aborted, _) = harness.downloads.abort(id);
        assert_eq!(aborted, AbortResult::Aborted);
        let updates = harness.deny(&key, TransferRejectReason::FILE_NOT_SHARED);
        assert!(updates.is_empty());
        let transfer = harness.transfer(&key);
        assert_eq!(transfer.phase, TransferPhase::Aborted);
        assert!(!transfer.legacy_attempt);
    }

    #[tokio::test]
    async fn a_token_held_by_another_download_is_refused() {
        let mut harness = Harness::new("token-reuse");
        let first = harness.queue("Music\\a.mp3");
        let second = harness.queue("Music\\b.mp3");
        harness.start("Music\\a.mp3", 7, 1);
        harness.drain();

        assert!(harness.request("Music\\b.mp3", 7, 100).is_empty());
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
        let downloads = &mut harness.downloads;
        assert!(downloads.handle_download_progress(1, 0).is_empty());
        assert!(downloads.handle_transfer_error(1, "stale").is_empty());
        assert!(downloads.handle_file_connection_closed(1).is_empty());
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
            failed(&format!("{LOCAL_FILE_ERROR}: disk full"))
        );
    }

    #[tokio::test]
    async fn denials_and_connection_errors_only_touch_queued_downloads() {
        let mut harness = Harness::new("queued-only");
        let key = harness.queue("Music\\a.mp3");
        let requested = harness.request("Music\\a.mp3", 7, 100);
        assert_eq!(statuses(&requested), vec![TransferStatus::Queued]);

        let denied = harness.deny(&key, TransferRejectReason::TOO_MANY_FILES);
        assert!(denied.is_empty());
        let errored = harness.fail_connection(&key, true);
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
    async fn a_denial_reason_from_a_peer_is_bounded_and_cannot_name_an_internal_status() {
        let mut harness = Harness::new("denial-reason");
        let long = harness.queue("Music\\a.mp3");
        let _ = harness.deny(&long, &"x".repeat(100_000));
        let TransferPhase::Failed(reason) = harness.phase(&long) else {
            panic!("expected a failed download");
        };
        assert_eq!(reason.len(), 255);

        let spoofed = harness.queue("Music\\b.mp3");
        let _ = harness.deny(&spoofed, CONNECTION_TIMEOUT);
        assert_eq!(
            harness.phase(&spoofed),
            failed(TransferRejectReason::CANCELLED)
        );
    }

    #[tokio::test]
    async fn aborting_a_download_that_waits_to_retry_stops_the_retry() {
        let mut harness = Harness::new("abort-waiting");
        let key = harness.queue("Music\\a.mp3");
        let _ = harness.fail_connection(&key, false);
        assert_eq!(harness.phase(&key), failed(CONNECTION_TIMEOUT));
        let id = harness.transfer(&key).id;
        let (result, work) = harness.downloads.abort(id);
        assert_eq!(result, AbortResult::Aborted);
        assert_eq!(statuses(&work), vec![TransferStatus::Aborted]);
        harness.downloads.user_online("uploader");
        assert!(harness.recover().is_empty());
        assert_eq!(harness.phase(&key), TransferPhase::Aborted);
    }

    #[tokio::test]
    async fn an_overlong_virtual_path_is_refused() {
        let mut harness = Harness::new("long-path");
        let (result, work) = harness.enqueue("uploader", &"a".repeat(VIRTUAL_PATH_LIMIT + 1), None);
        assert_eq!(result, EnqueueResult::PathTooLong);
        assert!(work.is_empty());
    }

    #[tokio::test]
    async fn limited_downloads_stay_held_until_rejected_or_disconnected() {
        let mut harness = Harness::new("limited-denials");
        let rejected = harness.queue("Music\\a.mp3");
        let dropped = harness.queue("Music\\b.mp3");
        for key in [&rejected, &dropped] {
            let held = harness.deny(key, TransferRejectReason::TOO_MANY_FILES);
            assert_eq!(statuses(&held), vec![TransferStatus::Queued]);
        }
        assert!(
            harness
                .deny(&rejected, TransferRejectReason::TOO_MANY_MEGABYTES)
                .is_empty()
        );
        assert_eq!(harness.phase(&rejected), TransferPhase::Limited);
        assert_eq!(harness.downloads.queue_limits["uploader"].held.len(), 2);

        let failed_work = harness.deny(&rejected, TransferRejectReason::FILE_NOT_SHARED);
        assert_eq!(statuses(&failed_work), vec![TransferStatus::Failed]);
        let failed_work = harness.fail_connection(&dropped, false);
        assert_eq!(statuses(&failed_work), vec![TransferStatus::Failed]);
        assert_eq!(harness.phase(&dropped), failed(CONNECTION_TIMEOUT));
    }

    #[tokio::test]
    async fn every_requeue_is_published() {
        let mut harness = Harness::new("requeue-updates");
        let key = harness.queue("Music\\a.mp3");
        let work = harness.fail_connection(&key, true);
        assert_eq!(statuses(&work), vec![TransferStatus::Failed]);

        harness.downloads.user_online("uploader");
        let requeued = harness.recover();
        assert_eq!(statuses(&requeued), vec![TransferStatus::Queued]);

        let work = harness.deny(&key, TransferRejectReason::CANCELLED);
        assert_eq!(statuses(&work), vec![TransferStatus::Failed]);
        let revived = harness.request("Music\\a.mp3", 9, 100);
        assert_eq!(statuses(&revived), vec![TransferStatus::Queued]);
        assert_eq!(harness.phase(&key), TransferPhase::GettingStatus);
    }

    #[tokio::test]
    async fn aborting_a_session_clears_size_changed_and_starting_clears_retry() {
        let mut harness = Harness::new("flag-resets");
        let key = harness.queue("Music\\a.mp3");
        let _ = harness.request("Music\\a.mp3", 7, 250);
        assert!(harness.transfer(&key).size_changed);
        let _ = harness
            .downloads
            .handle_file_transfer_init("uploader", 7, 1);
        let _ = harness.downloads.handle_transfer_error(1, "reset by peer");
        assert!(!harness.transfer(&key).size_changed);

        let _ = harness.request("Music\\a.mp3", 6, 400);
        assert!(harness.transfer(&key).size_changed);
        let reset = harness.downloads.reset();
        assert_eq!(statuses(&reset), vec![TransferStatus::Queued]);
        assert!(!harness.transfer(&key).size_changed);

        let _ = harness
            .downloads
            .handle_upload_failed("uploader", "Music\\a.mp3");
        let transfer = harness.transfer(&key);
        assert!(transfer.retry_attempt);
        assert_eq!(transfer.phase, TransferPhase::Queued);
        let _ = harness.request("Music\\a.mp3", 8, 250);
        let _ = harness
            .downloads
            .handle_file_transfer_init("uploader", 8, 2);
        assert!(!harness.transfer(&key).retry_attempt);
    }

    #[tokio::test]
    async fn a_folder_is_requested_in_one_batch() {
        let mut harness = Harness::new("folder-batch");
        let files = (0..900)
            .map(|index| FileInfo {
                name: format!("share\\Album\\{index}.mp3"),
                size: 100,
                attributes: FileAttributes::default(),
            })
            .collect();
        let work = harness.downloads.enqueue_folder(
            &mut harness.ids,
            &mut harness.users,
            "uploader".into(),
            files,
            "share\\Album",
        );
        assert_eq!(work.len(), 900);
        let commands = harness.drain();
        assert!(matches!(
            commands.as_slice(),
            [NetworkCommand::SendPeerMessages { messages, .. }] if messages.len() == 900
        ));
        harness.send_watches();
        assert_watches_the_uploader_once(&harness.drain());
    }

    #[tokio::test]
    async fn a_folder_enqueue_watches_the_uploader_once() {
        let mut harness = Harness::new("watch-once");
        for index in 0..20 {
            harness.queue(&format!("Music\\{index}.mp3"));
        }
        assert!(server_requests(&harness.drain()).is_empty());
        harness.send_watches();
        harness.send_watches();
        let sent = harness.drain();
        assert_watches_the_uploader_once(&sent);
        assert!(queue_uploads(&sent).is_empty());
    }

    #[tokio::test]
    async fn offline_uploader_parks_pending_downloads_and_resumes_them_on_return() {
        let mut harness = Harness::new("offline-park");
        let queued = harness.queue("a\\1.mp3");
        let dropped = harness.queue("a\\2.mp3");
        let denied = harness.queue("a\\3.mp3");
        let _ = harness.fail_connection(&dropped, false);
        let _ = harness.deny(&denied, TransferRejectReason::CANCELLED);
        harness.drain();

        harness.downloads.user_offline("uploader");
        let updates = harness.recover();
        assert_eq!(updates.len(), 2);
        assert_eq!(harness.phase(&queued), failed(USER_OFFLINE));
        assert_eq!(harness.phase(&dropped), failed(USER_OFFLINE));
        assert_eq!(
            harness.phase(&denied),
            failed(TransferRejectReason::CANCELLED)
        );

        harness.downloads.user_online("uploader");
        assert!(queue_uploads(&harness.drain()).is_empty());
        let updates = harness.recover();
        assert_eq!(
            statuses(&updates),
            vec![TransferStatus::Queued, TransferStatus::Queued]
        );
        assert_eq!(harness.phase(&queued), TransferPhase::Queued);
        assert_eq!(harness.phase(&dropped), TransferPhase::Queued);
        assert_eq!(
            queue_uploads(&harness.drain()),
            vec![queued.1.as_str(), dropped.1.as_str()]
        );
    }

    #[tokio::test]
    async fn parking_a_requested_download_forgets_its_token() {
        let mut harness = Harness::new("park-token");
        let key = harness.queue("a\\1.mp3");
        let _ = harness.request(&key.1, 7, 100);
        harness.downloads.user_offline("uploader");
        assert_eq!(statuses(&harness.recover()), vec![TransferStatus::Failed]);
        let downloads = &mut harness.downloads;
        assert!(downloads.transfers.key_by_token("uploader", 7).is_none());
        harness.drain();
        let downloads = &mut harness.downloads;
        assert!(
            downloads
                .handle_file_transfer_init("uploader", 7, 3)
                .is_empty()
        );
        assert!(harness.closed(3));
    }

    #[tokio::test]
    async fn a_new_session_watches_users_with_recoverable_downloads_only() {
        let mut harness = Harness::new("session-watch");
        let offline = harness.queue_from("away", "a\\1.mp3");
        let _ = harness.fail_connection(&offline, true);
        let rejected = harness.queue_from("rejecter", "b\\1.mp3");
        for _ in 0..2 {
            let _ = harness.deny(&rejected, TransferRejectReason::FILE_NOT_SHARED);
        }
        assert!(matches!(harness.phase(&rejected), TransferPhase::Failed(_)));
        harness.drain();

        let fresh_key = harness.queue_from("fresh", "c\\1.mp3");
        harness.drain();

        harness.users = no_users();
        harness.downloads.start_session(&mut harness.users);
        harness.send_watches();
        let sent = harness.drain();
        assert!(queue_uploads(&sent).is_empty());
        assert!(harness.recover().is_empty());
        assert_eq!(queue_uploads(&harness.drain()), vec![fresh_key.1.as_str()]);
        let watched: HashSet<&str> = server_requests(&sent)
            .into_iter()
            .filter_map(|request| match request {
                ServerRequest::WatchUser { user } => Some(user.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(watched, HashSet::from(["away", "fresh"]));
        assert!(harness.downloads.needs_watch("away"));
        assert!(!harness.downloads.needs_watch("rejecter"));
    }

    #[tokio::test]
    async fn queue_limit_denials_hold_downloads_until_the_queue_drains() {
        let mut harness = Harness::new("queue-limit");
        let keys: Vec<TransferKey> = (0..8)
            .map(|index| harness.queue(&format!("a\\{index}.mp3")))
            .collect();
        for key in &keys[1..] {
            let updates = harness.deny(key, TransferRejectReason::TOO_MANY_FILES);
            assert_eq!(statuses(&updates), vec![TransferStatus::Queued]);
            assert_eq!(harness.phase(key), TransferPhase::Limited);
        }
        harness.drain();

        harness.downloads.release_limited();
        assert!(harness.recover().is_empty());

        let _ = harness.request(&keys[0].1, 7, 100);
        harness.drain();
        harness.downloads.release_limited();
        let released = harness.recover();
        assert_eq!(released.len(), MIN_LIMITED_BATCH);
        let sent = harness.drain();
        let expected: Vec<&str> = keys[1..=MIN_LIMITED_BATCH]
            .iter()
            .map(|key| key.1.as_str())
            .collect();
        assert_eq!(queue_uploads(&sent), expected);
        harness.downloads.release_limited();
        assert!(harness.recover().is_empty());

        for key in &keys[1..=MIN_LIMITED_BATCH] {
            let _ = harness.deny(key, TransferRejectReason::TOO_MANY_MEGABYTES);
        }
        harness.downloads.release_limited();
        let released = harness.recover();
        assert_eq!(released.len(), MIN_LIMITED_BATCH);
        let sent = harness.drain();
        let rotated: Vec<&str> = keys[MIN_LIMITED_BATCH + 1..]
            .iter()
            .chain(&keys[1..=3])
            .map(|key| key.1.as_str())
            .collect();
        assert_eq!(queue_uploads(&sent), rotated);
    }

    #[tokio::test]
    async fn connection_and_io_failures_retry_on_their_timers_and_rejections_do_not() {
        let mut harness = Harness::new("retry-timers");
        let timed_out = harness.queue("a\\1.mp3");
        let shutdown = harness.queue("a\\2.mp3");
        let io = harness.queue("a\\3.mp3");
        let rejected = harness.queue("a\\4.mp3");
        let offline = harness.queue_from("gone", "b\\1.mp3");
        let _ = harness.fail_connection(&timed_out, false);
        let _ = harness.deny(&shutdown, TransferRejectReason::PENDING_SHUTDOWN);
        harness.start(&io.1, 9, 1);
        let _ = harness
            .downloads
            .handle_transfer_error(1, "No space left on device");
        let _ = harness.deny(&rejected, "Banned");
        let _ = harness.fail_connection(&offline, true);
        harness.drain();

        harness.downloads.start_session(&mut harness.users);
        harness.drain();
        harness.downloads.retry_failed();
        assert!(harness.recover().is_empty());

        harness.downloads.connection_retry_at = past(CONNECTION_RETRY_INTERVAL);
        harness.downloads.retry_failed();
        assert_eq!(harness.recover().len(), 2);
        assert_eq!(
            queue_uploads(&harness.drain()),
            vec![timed_out.1.as_str(), shutdown.1.as_str()]
        );
        assert!(matches!(harness.phase(&io), TransferPhase::Failed(_)));

        harness.downloads.io_retry_at = past(IO_RETRY_INTERVAL);
        harness.downloads.retry_failed();
        assert_eq!(harness.recover().len(), 1);
        assert_eq!(queue_uploads(&harness.drain()), vec![io.1.as_str()]);
        assert_eq!(harness.phase(&rejected), failed("Banned"));
        assert_eq!(harness.phase(&offline), failed(USER_OFFLINE));
    }

    #[tokio::test]
    async fn a_retry_timer_does_not_override_pending_offline_parking() {
        let mut harness = Harness::new("park-over-retry");
        let key = harness.queue("a\\1.mp3");
        let _ = harness.fail_connection(&key, false);
        harness.downloads.user_offline("uploader");
        harness.downloads.connection_retry_at = past(CONNECTION_RETRY_INTERVAL);
        harness.downloads.retry_failed();
        harness.drain();
        assert_eq!(statuses(&harness.recover()), vec![TransferStatus::Failed]);
        assert!(queue_uploads(&harness.drain()).is_empty());
        assert_eq!(harness.phase(&key), failed(USER_OFFLINE));
    }

    #[tokio::test]
    async fn queue_positions_are_requested_in_one_batch_per_user() {
        let mut harness = Harness::new("queue-positions");
        for index in 0..300 {
            harness.queue(&format!("a\\{index}.mp3"));
        }
        harness.queue_from("other", "b\\1.mp3");
        harness.drain();
        harness.downloads.queue_positions_at = past(QUEUE_POSITION_INTERVAL);
        harness.downloads.request_queue_positions();
        let mut batches: Vec<(String, usize)> = harness
            .drain()
            .into_iter()
            .map(|command| match command {
                NetworkCommand::SendPeerMessages { username, messages } => {
                    (username, messages.len())
                }
                other => panic!("expected a batch, got {other:?}"),
            })
            .collect();
        batches.sort();
        assert_eq!(
            batches,
            vec![("other".to_owned(), 1), ("uploader".to_owned(), 300)]
        );
    }

    #[tokio::test]
    async fn recovery_is_paced_and_dropped_when_the_uploader_leaves() {
        let mut harness = Harness::new("recovery-pacing");
        let total = RECOVERY_BATCH + 50;
        for index in 0..total {
            let key = harness.queue(&format!("a\\{index}.mp3"));
            let _ = harness.fail_connection(&key, true);
            harness.drain();
        }
        harness.downloads.user_online("uploader");
        assert_eq!(harness.recover().len(), RECOVERY_BATCH);
        assert_eq!(queue_uploads(&harness.drain()).len(), RECOVERY_BATCH);
        harness.downloads.user_offline("uploader");
        assert_eq!(harness.recover().len(), RECOVERY_BATCH);
        assert!(harness.recover().is_empty());
        assert!(queue_uploads(&harness.drain()).is_empty());
        harness.downloads.user_online("uploader");
        assert_eq!(harness.recover().len(), RECOVERY_BATCH);
        assert_eq!(harness.recover().len(), 50);
    }

    #[tokio::test]
    async fn a_manual_retry_cancels_the_pending_automatic_one() {
        let mut harness = Harness::new("manual-retry");
        let key = harness.queue("a\\1.mp3");
        let _ = harness.fail_connection(&key, true);
        harness.downloads.user_online("uploader");
        let id = harness.transfer(&key).id;
        let (result, _) = harness.downloads.retry(&mut harness.users, id);
        assert_eq!(result, RetryResult::Requeued);
        let _ = harness.deny(&key, TransferRejectReason::TOO_MANY_FILES);
        harness.drain();
        assert!(harness.recover().is_empty());
        assert!(queue_uploads(&harness.drain()).is_empty());
        assert_eq!(harness.phase(&key), TransferPhase::Limited);
    }
}
