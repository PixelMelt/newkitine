use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::files;
use super::speed::SpeedMeter;
use crate::network::NetworkHandle;
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::{
    AbortResult, ConnId, EnqueueResult, FileAttributes, NetworkCommand, TransferDirection,
    TransferEvent, TransferId, TransferRejectReason, TransferSeed, TransferStatus, TransferUpdate,
};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

pub struct TransferIds {
    next: u64,
}

impl TransferIds {
    pub fn new(seeds: &[TransferSeed]) -> Self {
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
        matches!(self, Self::Queued | Self::GettingStatus | Self::Transferring)
    }

    pub fn from_seed(seed: &TransferSeed) -> Self {
        match seed.status {
            TransferStatus::Queued | TransferStatus::Transferring => Self::Queued,
            TransferStatus::Finished => Self::Finished,
            TransferStatus::Aborted => Self::Aborted,
            TransferStatus::Failed => {
                Self::Failed(seed.failure_reason.clone().unwrap_or_default())
            }
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
    pub token: Option<u32>,
    pub conn_id: Option<ConnId>,
    pub bytes_done: u64,
    pub retry_attempt: bool,
    pub size_changed: bool,
    pub activated_at: Option<Instant>,
    pub(crate) speed: SpeedMeter,
}

impl Transfer {
    pub fn key(&self) -> (String, String) {
        (self.username.clone(), self.virtual_path.clone())
    }

    pub fn basename(&self) -> String {
        self.virtual_path
            .rsplit('\\')
            .next()
            .unwrap_or(&self.virtual_path)
            .to_owned()
    }

    fn queued_event(&self) -> TransferEvent {
        event(
            self.id,
            TransferUpdate::Queued {
                username: self.username.clone(),
                virtual_path: self.virtual_path.clone(),
                size: self.size,
                attributes: self.attributes.clone(),
            },
        )
    }
}

fn event(id: TransferId, update: TransferUpdate) -> TransferEvent {
    TransferEvent {
        id,
        direction: TransferDirection::Download,
        update,
    }
}

pub struct Downloads {
    net: NetworkHandle,
    download_dir: PathBuf,
    incomplete_dir: PathBuf,
    transfers: HashMap<(String, String), Transfer>,
    ids: HashMap<TransferId, (String, String)>,
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
            ids: HashMap::new(),
            active_tokens: HashMap::new(),
            conn_tokens: HashMap::new(),
        }
    }

    pub fn set_dirs(&mut self, download_dir: PathBuf, incomplete_dir: PathBuf) {
        self.download_dir = download_dir;
        self.incomplete_dir = incomplete_dir;
    }

    pub fn seed(&mut self, seed: TransferSeed) {
        let phase = TransferPhase::from_seed(&seed);
        let key = (seed.username.clone(), seed.virtual_path.clone());
        self.ids.insert(seed.id, key.clone());
        self.transfers.insert(
            key,
            Transfer {
                id: seed.id,
                username: seed.username,
                virtual_path: seed.virtual_path,
                size: seed.size,
                attributes: seed.attributes,
                phase,
                token: None,
                conn_id: None,
                bytes_done: 0,
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
    ) -> (EnqueueResult, Vec<TransferEvent>) {
        let key = (username.clone(), virtual_path.clone());
        let id = match self.transfers.get(&key) {
            Some(existing) if existing.phase.is_active() => {
                debug!(username, virtual_path, "download already in progress");
                return (EnqueueResult::AlreadyActive, Vec::new());
            }
            Some(existing) => existing.id,
            None => ids.mint(),
        };
        self.ids.insert(id, key.clone());
        let transfer = Transfer {
            id,
            username: username.clone(),
            virtual_path: virtual_path.clone(),
            size,
            attributes,
            phase: TransferPhase::Queued,
            token: None,
            conn_id: None,
            bytes_done: 0,
            retry_attempt: false,
            size_changed: false,
            activated_at: None,
            speed: SpeedMeter::default(),
        };
        let queued = transfer.queued_event();
        self.transfers.insert(key, transfer);
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

    pub fn abort(&mut self, id: TransferId) -> (AbortResult, Vec<TransferEvent>) {
        let Some(key) = self.ids.get(&id).cloned() else {
            return (AbortResult::NotFound, Vec::new());
        };
        let transfer = self.transfers.get_mut(&key).unwrap();
        if !transfer.phase.is_active() {
            return (AbortResult::Aborted, Vec::new());
        }
        transfer.phase = TransferPhase::Aborted;
        if let Some(token) = transfer.token.take() {
            self.active_tokens
                .remove(&(transfer.username.clone(), token));
        }
        if let Some(conn_id) = transfer.conn_id.take() {
            self.conn_tokens.remove(&conn_id);
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
        (
            AbortResult::Aborted,
            vec![event(id, TransferUpdate::Aborted)],
        )
    }

    pub fn clear(&mut self, statuses: &[TransferStatus]) -> Vec<TransferId> {
        let removed: Vec<TransferId> = self
            .transfers
            .values()
            .filter(|transfer| statuses.contains(&transfer.phase.status()))
            .map(|transfer| transfer.id)
            .collect();
        for id in &removed {
            let key = self.ids.remove(id).unwrap();
            self.transfers.remove(&key);
        }
        removed
    }

    pub fn clear_all(&mut self) -> Vec<TransferId> {
        for transfer in self.transfers.values_mut() {
            if let Some(token) = transfer.token.take() {
                self.active_tokens
                    .remove(&(transfer.username.clone(), token));
            }
            if let Some(conn_id) = transfer.conn_id.take() {
                self.conn_tokens.remove(&conn_id);
                self.net.send(NetworkCommand::CloseConnection(conn_id));
            }
        }
        self.transfers.clear();
        self.ids.drain().map(|(id, _)| id).collect()
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
        let keys: Vec<(String, String)> = self
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
        &self,
        username: &str,
        virtual_path: &str,
        place: u32,
    ) -> Option<TransferEvent> {
        let transfer = self
            .transfers
            .get(&(username.to_owned(), virtual_path.to_owned()))?;
        Some(event(transfer.id, TransferUpdate::QueuePlace { place }))
    }

    pub fn owns_token(&self, username: &str, token: u32) -> bool {
        self.active_tokens
            .contains_key(&(username.to_owned(), token))
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
                transfer.token = Some(token);
                transfer.activated_at = Some(Instant::now());
                self.active_tokens
                    .insert((username.to_owned(), token), file.to_owned());
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
        self.net.peer(username, response);
    }

    pub fn handle_file_transfer_init(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
    ) -> Vec<TransferEvent> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            debug!(
                username,
                token, "file transfer init with unknown token, closing"
            );
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        let incomplete_path = files::incomplete_file_path(&self.incomplete_dir, username, &virtual_path);
        let incomplete_dir = self.incomplete_dir.clone();
        let transfer = self.transfers.get_mut(&key).unwrap();
        if transfer.conn_id.is_some() {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        }
        transfer.conn_id = Some(conn_id);
        transfer.activated_at = None;
        self.conn_tokens
            .insert(conn_id, (username.to_owned(), token));

        let size_changed = transfer.size_changed;
        match files::open_incomplete(&incomplete_dir, &incomplete_path, size_changed) {
            Ok((file, offset)) => {
                if transfer.size > offset {
                    transfer.phase = TransferPhase::Transferring;
                    transfer.bytes_done = offset;
                    transfer.speed.reset();
                    let size = transfer.size;
                    let id = transfer.id;
                    info!(username, virtual_path, offset, size, "download started");
                    self.net.send(NetworkCommand::DownloadFile {
                        conn_id,
                        file,
                        offset,
                        bytes_left: transfer.size - offset,
                    });
                    vec![event(id, TransferUpdate::Started { file_path: None })]
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
    ) -> Vec<TransferEvent> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path);
        let Some(transfer) = self.transfers.get_mut(&key) else {
            return Vec::new();
        };
        transfer.bytes_done = transfer.size.saturating_sub(bytes_left);
        if bytes_left == 0 {
            return self.finish(&key);
        }
        let speed_bps = transfer.speed.sample(transfer.bytes_done);
        vec![event(
            transfer.id,
            TransferUpdate::Progress {
                bytes_done: transfer.bytes_done,
                size: transfer.size,
                speed_bps,
            },
        )]
    }

    pub fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: Option<u32>,
        conn_id: ConnId,
    ) -> Vec<TransferEvent> {
        let token = match token.or_else(|| self.conn_tokens.get(&conn_id).map(|(_, token)| *token))
        {
            Some(token) => token,
            None => return Vec::new(),
        };
        self.conn_tokens.remove(&conn_id);
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path);
        let Some(transfer) = self.transfers.get(&key) else {
            return Vec::new();
        };
        match transfer.phase {
            TransferPhase::Transferring => self.fail(&key, "connection closed".into()),
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
    ) -> Vec<TransferEvent> {
        let key = (username.to_owned(), file.to_owned());
        match self.transfers.get(&key) {
            Some(transfer) if transfer.phase != TransferPhase::Finished => {
                self.fail(&key, reason.to_owned())
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_upload_failed(&mut self, username: &str, file: &str) -> Vec<TransferEvent> {
        let key = (username.to_owned(), file.to_owned());
        let Some(transfer) = self.transfers.get_mut(&key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Finished | TransferPhase::Aborted
        ) {
            return Vec::new();
        }
        if !transfer.retry_attempt {
            transfer.retry_attempt = true;
            transfer.phase = TransferPhase::Queued;
            transfer.token = None;
            transfer.conn_id = None;
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
    ) -> Vec<TransferEvent> {
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

    pub fn sweep_request_timeouts(&mut self) -> Vec<TransferEvent> {
        let expired: Vec<(String, String)> = self
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

    pub fn reset(&mut self) -> Vec<TransferEvent> {
        self.active_tokens.clear();
        self.conn_tokens.clear();
        let mut updates = Vec::new();
        for transfer in self.transfers.values_mut() {
            if matches!(
                transfer.phase,
                TransferPhase::GettingStatus | TransferPhase::Transferring
            ) {
                transfer.phase = TransferPhase::Queued;
                transfer.token = None;
                transfer.conn_id = None;
                transfer.activated_at = None;
                updates.push(transfer.queued_event());
            }
        }
        updates
    }

    fn finish(&mut self, key: &(String, String)) -> Vec<TransferEvent> {
        let transfer = self.transfers.get(key).unwrap();
        let username = transfer.username.clone();
        let virtual_path = transfer.virtual_path.clone();
        let basename = transfer.basename();
        let incomplete_path =
            files::incomplete_file_path(&self.incomplete_dir, &username, &virtual_path);

        match files::place_download(&self.download_dir, &incomplete_path, &basename) {
            Ok(destination) => {
                let transfer = self.transfers.get_mut(key).unwrap();
                transfer.phase = TransferPhase::Finished;
                if let Some(token) = transfer.token.take() {
                    self.active_tokens
                        .remove(&(transfer.username.clone(), token));
                }
                info!(username, virtual_path, ?destination, "download finished");
                vec![event(
                    transfer.id,
                    TransferUpdate::Finished {
                        file_path: Some(destination),
                        size: transfer.size,
                        speed_bps: None,
                    },
                )]
            }
            Err(error) => self.fail(key, format!("cannot place finished download: {error}")),
        }
    }

    fn fail(&mut self, key: &(String, String), reason: String) -> Vec<TransferEvent> {
        let Some(transfer) = self.transfers.get_mut(key) else {
            return Vec::new();
        };
        if matches!(
            transfer.phase,
            TransferPhase::Finished | TransferPhase::Aborted | TransferPhase::Failed(_)
        ) {
            return Vec::new();
        }
        transfer.phase = TransferPhase::Failed(reason.clone());
        if let Some(token) = transfer.token.take() {
            self.active_tokens
                .remove(&(transfer.username.clone(), token));
        }
        if let Some(conn_id) = transfer.conn_id.take() {
            self.conn_tokens.remove(&conn_id);
        }
        vec![event(transfer.id, TransferUpdate::Failed { reason })]
    }

}
