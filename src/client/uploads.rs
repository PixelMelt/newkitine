use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::downloads::TransferStatus;
use super::shares::SharesIndex;
use super::users::Users;
use crate::network::{ConnId, NetworkCommand, NetworkHandle};
use crate::protocol::{PeerMessage, ServerRequest, increment_token, initial_token};
use crate::types::{FileAttributes, Restriction, TransferDirection, TransferRejectReason};

const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
pub const DEFAULT_UPLOAD_SLOTS: usize = 2;
pub const DEFAULT_QUEUE_FILE_LIMIT: usize = 500;

#[derive(Debug)]
pub enum UploadUpdate {
    Queued {
        username: String,
        virtual_path: String,
        attributes: FileAttributes,
    },
    Started {
        username: String,
        virtual_path: String,
        real_path: PathBuf,
    },
    Progress {
        username: String,
        virtual_path: String,
        bytes_done: u64,
        size: u64,
    },
    Finished {
        username: String,
        virtual_path: String,
        size: u64,
        speed_bps: Option<u32>,
    },
    Failed {
        username: String,
        virtual_path: String,
        reason: String,
    },
}

#[derive(Debug)]
struct UploadTransfer {
    username: String,
    virtual_path: String,
    real_path: PathBuf,
    size: u64,
    status: TransferStatus,
    token: Option<u32>,
    conn_id: Option<ConnId>,
    bytes_done: u64,
    started_offset: u64,
    activated_at: Option<Instant>,
    started_at: Option<Instant>,
}

pub struct Uploads {
    net: NetworkHandle,
    upload_slots: usize,
    queue_file_limit: usize,
    transfers: HashMap<(String, String), UploadTransfer>,
    queue: Vec<(String, String)>,
    active_tokens: HashMap<(String, u32), String>,
    active_users: HashMap<String, u32>,
    user_counters: HashMap<String, u64>,
    counter: u64,
    token: u32,
    pub upload_speed: u32,
}

impl Uploads {
    pub fn new(net: NetworkHandle, upload_slots: usize, queue_file_limit: usize) -> Self {
        Self {
            net,
            upload_slots,
            queue_file_limit,
            transfers: HashMap::new(),
            queue: Vec::new(),
            active_tokens: HashMap::new(),
            active_users: HashMap::new(),
            user_counters: HashMap::new(),
            counter: 0,
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
        self.active_tokens.len() < self.upload_slots
    }

    pub fn queue_size(&self) -> u32 {
        self.queue.len() as u32
    }

    pub fn owns_token(&self, username: &str, token: u32) -> bool {
        self.active_tokens
            .contains_key(&(username.to_owned(), token))
    }

    pub fn handle_queue_upload(
        &mut self,
        username: &str,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> (Vec<UploadUpdate>, bool) {
        match self.validate_request(username, virtual_path, shares, users) {
            Ok((real_path, size, attributes)) => {
                let updates = self.enqueue(username, virtual_path, real_path, size, attributes);
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
        username: &str,
        token: u32,
        virtual_path: &str,
        shares: Option<&SharesIndex>,
        users: &Users,
    ) -> (Vec<UploadUpdate>, bool) {
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
                let updates = self.enqueue(username, virtual_path, real_path, size, attributes);
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
        let queued_for_user = self
            .queue
            .iter()
            .filter(|(queued_username, _)| queued_username == username)
            .count();
        if queued_for_user >= self.queue_file_limit {
            return Err(TransferRejectReason::TOO_MANY_FILES.into());
        }
        Ok((real_path.to_owned(), size, attributes.clone()))
    }

    fn enqueue(
        &mut self,
        username: &str,
        virtual_path: &str,
        real_path: PathBuf,
        size: u64,
        attributes: FileAttributes,
    ) -> Vec<UploadUpdate> {
        let key = (username.to_owned(), virtual_path.to_owned());
        if let Some(existing) = self.transfers.get(&key)
            && matches!(
                existing.status,
                TransferStatus::Queued
                    | TransferStatus::GettingStatus
                    | TransferStatus::Transferring
            )
        {
            debug!(username, virtual_path, "upload already queued");
            return Vec::new();
        }
        self.transfers.insert(
            key.clone(),
            UploadTransfer {
                username: username.to_owned(),
                virtual_path: virtual_path.to_owned(),
                real_path,
                size,
                status: TransferStatus::Queued,
                token: None,
                conn_id: None,
                bytes_done: 0,
                started_offset: 0,
                activated_at: None,
                started_at: None,
            },
        );
        self.queue.push(key);
        self.update_user_counter(username);
        vec![UploadUpdate::Queued {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
            attributes,
        }]
    }

    fn update_user_counter(&mut self, username: &str) {
        let has_queued = self.queue.iter().any(|(user, _)| user == username);
        if has_queued && !self.active_users.contains_key(username) {
            self.counter += 1;
            self.user_counters.insert(username.to_owned(), self.counter);
        } else {
            self.user_counters.remove(username);
        }
    }

    pub fn check_queue(&mut self, users: &Users) {
        while self.is_new_upload_accepted() {
            let eligible: Vec<&String> = self
                .user_counters
                .keys()
                .filter(|username| !matches!(users.restriction(username), Some(Restriction::Hold)))
                .collect();
            let privileged: Vec<&&String> = eligible
                .iter()
                .filter(|username| users.is_privileged(username))
                .collect();
            let normal: Vec<&&String> = eligible
                .iter()
                .filter(|username| {
                    !matches!(
                        users.restriction(username),
                        Some(Restriction::Deprioritized)
                    )
                })
                .collect();
            let tier: Vec<&String> = if !privileged.is_empty() {
                privileged.into_iter().copied().collect()
            } else if !normal.is_empty() {
                normal.into_iter().copied().collect()
            } else {
                eligible
            };
            let Some(key) = tier
                .into_iter()
                .min_by_key(|username| self.user_counters[*username])
                .cloned()
                .and_then(|username| {
                    self.queue
                        .iter()
                        .find(|(user, _)| *user == username)
                        .cloned()
                })
            else {
                break;
            };
            self.activate(&key);
        }
    }

    pub fn deny_all(&mut self, username: &str, reason: &str, users: &Users) -> Vec<UploadUpdate> {
        let pending: Vec<(String, String)> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.username == username
                    && matches!(
                        transfer.status,
                        TransferStatus::Queued
                            | TransferStatus::GettingStatus
                            | TransferStatus::Transferring
                    )
            })
            .map(|transfer| (transfer.username.clone(), transfer.virtual_path.clone()))
            .collect();
        let mut updates = Vec::new();
        for key in &pending {
            if let Some(conn_id) = self
                .transfers
                .get(key)
                .and_then(|transfer| transfer.conn_id)
            {
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

    fn activate(&mut self, key: &(String, String)) {
        self.token = increment_token(self.token);
        let token = self.token;
        self.queue.retain(|queued| queued != key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.status = TransferStatus::GettingStatus;
        transfer.token = Some(token);
        transfer.activated_at = Some(Instant::now());
        let username = transfer.username.clone();
        let virtual_path = transfer.virtual_path.clone();
        let size = transfer.size;
        self.active_tokens
            .insert((username.clone(), token), virtual_path.clone());
        self.active_users.insert(username.clone(), token);
        self.user_counters.remove(&username);
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
    ) -> Vec<UploadUpdate> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            debug!(username, token, "transfer response for unknown upload");
            return Vec::new();
        };
        if let Some(reason) = reason {
            let key = (username.to_owned(), virtual_path);
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
    ) -> Vec<UploadUpdate> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        let transfer = self.transfers.get_mut(&key).unwrap();
        if transfer.conn_id.is_some() {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
            return Vec::new();
        }
        transfer.conn_id = Some(conn_id);
        transfer.activated_at = None;

        match fs::File::open(&transfer.real_path) {
            Ok(file) => {
                transfer.status = TransferStatus::Transferring;
                transfer.started_at = Some(Instant::now());
                let real_path = transfer.real_path.clone();
                let size = transfer.size;
                info!(username, virtual_path, size, "upload started");
                self.net.send(NetworkCommand::UploadFile {
                    conn_id,
                    file,
                    size,
                });
                vec![UploadUpdate::Started {
                    username: username.to_owned(),
                    virtual_path,
                    real_path,
                }]
            }
            Err(error) => {
                self.net.send(NetworkCommand::CloseConnection(conn_id));
                self.net.peer(
                    username,
                    PeerMessage::UploadFailed {
                        file: virtual_path.clone(),
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
    ) -> Vec<UploadUpdate> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        let Some(transfer) = self.transfers.get_mut(&key) else {
            return Vec::new();
        };
        transfer.started_offset = offset;
        transfer.bytes_done = offset + bytes_sent;
        vec![UploadUpdate::Progress {
            username: username.to_owned(),
            virtual_path,
            bytes_done: transfer.bytes_done,
            size: transfer.size,
        }]
    }

    pub fn handle_transfer_error(
        &mut self,
        username: &str,
        token: u32,
        error: &str,
        users: &Users,
    ) -> Vec<UploadUpdate> {
        let Some(virtual_path) = self
            .active_tokens
            .get(&(username.to_owned(), token))
            .cloned()
        else {
            return Vec::new();
        };
        let key = (username.to_owned(), virtual_path.clone());
        if let Some(conn_id) = self
            .transfers
            .get(&key)
            .and_then(|transfer| transfer.conn_id)
        {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
        self.net
            .peer(username, PeerMessage::UploadFailed { file: virtual_path });
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
    ) -> Vec<UploadUpdate> {
        let key = match self.transfers.iter().find(|(_, transfer)| {
            transfer.conn_id == Some(conn_id)
                || (token.is_some() && transfer.token == token && transfer.username == username)
        }) {
            Some((key, _)) => key.clone(),
            None => return Vec::new(),
        };
        let transfer = self.transfers.get(&key).unwrap();
        let updates = match transfer.status {
            TransferStatus::Transferring if transfer.bytes_done >= transfer.size => {
                self.finish(&key, true)
            }
            TransferStatus::Transferring => self.fail(&key, "connection closed".into()),
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
    ) -> Vec<UploadUpdate> {
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

    pub fn abort(&mut self, username: &str, virtual_path: &str, users: &Users) {
        let key = (username.to_owned(), virtual_path.to_owned());
        let Some(transfer) = self.transfers.get(&key) else {
            return;
        };
        let was_pending = matches!(
            transfer.status,
            TransferStatus::Queued | TransferStatus::GettingStatus | TransferStatus::Transferring
        );
        self.deactivate(&key);
        let transfer = self.transfers.remove(&key).unwrap();
        if let Some(conn_id) = transfer.conn_id {
            self.net.send(NetworkCommand::CloseConnection(conn_id));
        }
        if was_pending {
            self.net.peer(
                key.0,
                PeerMessage::UploadDenied {
                    file: key.1,
                    reason: TransferRejectReason::CANCELLED.into(),
                },
            );
        }
        self.check_queue(users);
    }

    pub fn handle_place_in_queue_request(&mut self, username: &str, virtual_path: &str) {
        let Some(place) = self
            .queue
            .iter()
            .position(|key| key.0 == username && key.1 == virtual_path)
        else {
            return;
        };
        self.net.peer(
            username,
            PeerMessage::PlaceInQueueResponse {
                filename: virtual_path.to_owned(),
                place: place as u32 + 1,
            },
        );
    }

    pub fn sweep_request_timeouts(&mut self, users: &Users) -> Vec<UploadUpdate> {
        let expired: Vec<(String, String)> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.status == TransferStatus::GettingStatus
                    && transfer
                        .activated_at
                        .is_some_and(|at| at.elapsed() > TRANSFER_REQUEST_TIMEOUT)
            })
            .map(|transfer| (transfer.username.clone(), transfer.virtual_path.clone()))
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

    pub fn reset(&mut self) {
        let active: Vec<(String, String)> = self
            .transfers
            .values()
            .filter(|transfer| {
                matches!(
                    transfer.status,
                    TransferStatus::GettingStatus | TransferStatus::Transferring
                )
            })
            .map(|transfer| (transfer.username.clone(), transfer.virtual_path.clone()))
            .collect();
        for key in &active {
            self.fail(key, "server disconnected".into());
        }
    }

    fn finish(&mut self, key: &(String, String), send_speed: bool) -> Vec<UploadUpdate> {
        self.deactivate(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.status = TransferStatus::Finished;
        transfer.conn_id = None;
        let size = transfer.size;
        let mut speed_bps = None;
        if send_speed && let Some(started_at) = transfer.started_at {
            let elapsed = started_at.elapsed().as_secs_f64();
            let bytes_sent = transfer.bytes_done - transfer.started_offset;
            if elapsed >= 1.0 && bytes_sent > 0 {
                self.upload_speed = (bytes_sent as f64 / elapsed) as u32;
                speed_bps = Some(self.upload_speed);
                self.net.server(ServerRequest::SendUploadSpeed {
                    speed: self.upload_speed,
                });
            }
        }
        info!(username = key.0, virtual_path = key.1, "upload finished");
        vec![UploadUpdate::Finished {
            username: key.0.clone(),
            virtual_path: key.1.clone(),
            size,
            speed_bps,
        }]
    }

    fn fail(&mut self, key: &(String, String), reason: String) -> Vec<UploadUpdate> {
        let Some(transfer) = self.transfers.get(key) else {
            return Vec::new();
        };
        if matches!(
            transfer.status,
            TransferStatus::Finished | TransferStatus::Failed(_)
        ) {
            return Vec::new();
        }
        self.deactivate(key);
        let transfer = self.transfers.get_mut(key).unwrap();
        transfer.status = TransferStatus::Failed(reason.clone());
        transfer.conn_id = None;
        vec![UploadUpdate::Failed {
            username: key.0.clone(),
            virtual_path: key.1.clone(),
            reason,
        }]
    }

    fn deactivate(&mut self, key: &(String, String)) {
        let Some(transfer) = self.transfers.get_mut(key) else {
            return;
        };
        if let Some(token) = transfer.token.take() {
            self.active_tokens
                .remove(&(transfer.username.clone(), token));
            if self.active_users.get(&key.0) == Some(&token) {
                self.active_users.remove(&key.0);
            }
        }
        self.queue.retain(|queued| queued != key);
        self.update_user_counter(&key.0);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::client::shares::{self, SharedFolder};
    use crate::network::spawn as spawn_network;

    #[tokio::test]
    async fn restriction_tiers_control_slot_grants() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 0, 500);

        let dir =
            std::env::temp_dir().join(format!("newkitine-uploads-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song.mp3"), b"payload").unwrap();
        let shares = shares::scan(&[SharedFolder {
            virtual_name: "Music".into(),
            path: dir,
            buddy_only: false,
        }]);

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_restriction("leech".into(), Restriction::Deprioritized);

        let (_, accepted) =
            uploads.handle_queue_upload("leech", "Music\\song.mp3", Some(&shares), &users);
        assert!(accepted);
        let (_, accepted) =
            uploads.handle_queue_upload("human", "Music\\song.mp3", Some(&shares), &users);
        assert!(accepted);

        uploads.set_limits(1, 500);
        uploads.check_queue(&users);
        assert!(uploads.active_users.contains_key("human"));
        assert!(!uploads.active_users.contains_key("leech"));

        users.set_restriction("leech".into(), Restriction::Hold);
        uploads.set_limits(2, 500);
        uploads.check_queue(&users);
        assert!(!uploads.active_users.contains_key("leech"));

        users.set_restriction("leech".into(), Restriction::None);
        uploads.check_queue(&users);
        assert!(uploads.active_users.contains_key("leech"));
    }

    #[tokio::test]
    async fn denied_restriction_rejects_queue_requests() {
        let (net, _events) = spawn_network();
        let mut uploads = Uploads::new(net, 2, 500);

        let dir = std::env::temp_dir().join(format!("newkitine-deny-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song.mp3"), b"payload").unwrap();
        let shares = shares::scan(&[SharedFolder {
            virtual_name: "Music".into(),
            path: dir,
            buddy_only: false,
        }]);

        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_restriction(
            "leech".into(),
            Restriction::Denied {
                reason: "not welcome".into(),
            },
        );
        let (updates, accepted) =
            uploads.handle_queue_upload("leech", "Music\\song.mp3", Some(&shares), &users);
        assert!(!accepted);
        assert!(updates.is_empty());
    }
}
