use std::collections::{HashMap, HashSet};
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

pub(super) type PlacementDone = (TransferKey, std::io::Result<PathBuf>);

pub(in crate::client) struct Downloads {
    net: NetworkHandle,
    download_dir: PathBuf,
    incomplete_dir: PathBuf,
    username_subfolders: bool,
    placements: mpsc::UnboundedSender<PlacementDone>,
    transfers: Registry<Transfer>,
    basename_limits: HashMap<PathBuf, usize>,
    queue_limits: HashMap<String, usize>,
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
        self.transfers.insert(id, key.clone(), transfer);
        users.watch(&self.net, &username);
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
        let mut queued = Vec::new();
        for transfer in self.transfers.values() {
            if transfer.needs_watch() {
                users.watch(&self.net, &transfer.username);
            }
            if transfer.phase == TransferPhase::Queued {
                queued.push(transfer.key());
            }
        }
        for key in queued {
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

    pub fn user_offline(&mut self, username: &str) -> Vec<TransferWork> {
        let keys = self.keys_where(|transfer| {
            transfer.username == username
                && match &transfer.phase {
                    TransferPhase::Queued
                    | TransferPhase::Limited
                    | TransferPhase::GettingStatus => true,
                    phase => matches!(recovery(phase), Some(Recovery::Connection | Recovery::Io)),
                }
        });
        let mut updates = Vec::new();
        for key in keys {
            self.transfers.detach(&key);
            let transfer = self.transfers.get_mut(&key).unwrap();
            transfer.phase = TransferPhase::Failed(USER_OFFLINE.into());
            transfer.activated_at = None;
            transfer.queue_place = 0;
            updates.push(TransferWork::Update(transfer.snapshot()));
        }
        updates
    }

    pub fn user_online(&mut self, users: &mut Users, username: &str) -> Vec<TransferWork> {
        let keys = self.keys_where(|transfer| {
            transfer.username == username
                && (transfer.phase == TransferPhase::Limited || recovery(&transfer.phase).is_some())
        });
        keys.iter()
            .flat_map(|key| self.requeue(users, key))
            .collect()
    }

    pub fn retry_failed(&mut self, users: &mut Users) -> Vec<TransferWork> {
        let connection = due(&mut self.connection_retry_at, CONNECTION_RETRY_INTERVAL);
        let io = due(&mut self.io_retry_at, IO_RETRY_INTERVAL);
        if !connection && !io {
            return Vec::new();
        }
        let keys = self.keys_where(|transfer| match recovery(&transfer.phase) {
            Some(Recovery::Connection) => connection,
            Some(Recovery::Io) => io,
            Some(Recovery::UserOnline) | None => false,
        });
        keys.iter()
            .flat_map(|key| self.requeue(users, key))
            .collect()
    }

    pub fn release_limited(&mut self, users: &mut Users) -> Vec<TransferWork> {
        if self.queue_limits.is_empty() {
            return Vec::new();
        }
        let busy: HashSet<&str> = self
            .transfers
            .values()
            .filter(|transfer| transfer.phase == TransferPhase::Queued)
            .map(|transfer| transfer.username.as_str())
            .collect();
        let drained: Vec<(String, usize)> = self
            .queue_limits
            .iter()
            .filter(|(username, _)| !busy.contains(username.as_str()))
            .map(|(username, batch)| (username.clone(), *batch))
            .collect();
        let mut updates = Vec::new();
        for (username, batch) in drained {
            let limited = self.keys_where(|transfer| {
                transfer.username == username && transfer.phase == TransferPhase::Limited
            });
            if limited.len() <= batch {
                self.queue_limits.remove(&username);
            }
            for key in limited.iter().take(batch) {
                updates.extend(self.requeue(users, key));
            }
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
            self.queue_limits.insert(
                username.to_owned(),
                queued.saturating_sub(1).max(MIN_LIMITED_BATCH),
            );
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
        assert_eq!(queue_uploads(&sent).len(), 20);
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

        let updates = downloads.user_offline("uploader");
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

        let updates = downloads.user_online(&mut users, "uploader");
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

        let mut fresh = no_users();
        downloads.start_session(&mut fresh);
        let sent = drain(&mut commands);
        let watched: Vec<&ServerRequest> = server_requests(&sent)
            .into_iter()
            .filter(|request| matches!(request, ServerRequest::WatchUser { .. }))
            .collect();
        assert_eq!(
            watched,
            vec![&ServerRequest::WatchUser {
                user: "away".into()
            }]
        );
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

        assert!(downloads.release_limited(&mut users).is_empty());

        downloads.handle_transfer_request("uploader", 7, &keys[0].1, Some(100));
        drain(&mut commands);
        let released = downloads.release_limited(&mut users);
        assert_eq!(released.len(), MIN_LIMITED_BATCH);
        let sent = drain(&mut commands);
        let requested = queue_uploads(&sent);
        let expected: Vec<&str> = keys[1..=MIN_LIMITED_BATCH]
            .iter()
            .map(|key| key.1.as_str())
            .collect();
        assert_eq!(requested, expected);
        assert!(downloads.release_limited(&mut users).is_empty());

        for key in &keys[1..=MIN_LIMITED_BATCH] {
            let _ = downloads.handle_upload_denied("uploader", &key.1, "Cancelled");
        }
        let released = downloads.release_limited(&mut users);
        assert_eq!(released.len(), 2);
        assert!(downloads.queue_limits.is_empty());
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
        assert!(downloads.retry_failed(&mut users).is_empty());

        let past = |interval: Duration| {
            Instant::now()
                .checked_sub(interval + Duration::from_secs(1))
                .unwrap()
        };
        downloads.connection_retry_at = past(CONNECTION_RETRY_INTERVAL);
        let retried = downloads.retry_failed(&mut users);
        assert_eq!(retried.len(), 2);
        assert_eq!(
            queue_uploads(&drain(&mut commands)),
            vec![timed_out.1.as_str(), shutdown.1.as_str()]
        );
        assert!(matches!(phase(&downloads, &io), TransferPhase::Failed(_)));

        downloads.io_retry_at = past(IO_RETRY_INTERVAL);
        let retried = downloads.retry_failed(&mut users);
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
}
