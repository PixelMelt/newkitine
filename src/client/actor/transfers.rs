use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use tracing::warn;

use super::ClientActor;
use crate::client::transfers::destination_root;
use crate::client::{
    AbortResult, ClientEvent, EnqueueResult, FOLDER_DOWNLOAD_FILE_LIMIT, RetryResult, TransferWork,
};
use crate::network::ConnId;
use crate::network::NetworkCommand;
use crate::types::{
    FileAttributes, FileInfo, FolderContents, Restriction, TransferDirection, TransferId,
    TransferStatus,
};

use crate::protocol::PeerMessage;

impl ClientActor {
    pub(super) fn emit_transfers(&self, work: Vec<TransferWork>) {
        for item in work {
            self.emit_transfer_work(item);
        }
    }

    pub(super) fn enqueue_download(
        &mut self,
        username: String,
        virtual_path: String,
        size: u64,
        attributes: FileAttributes,
        root: Option<String>,
        ack: oneshot::Sender<EnqueueResult>,
    ) {
        let (result, events) = self.downloads.enqueue(
            &mut self.transfer_ids,
            username,
            FileInfo {
                name: virtual_path,
                size,
                attributes,
            },
            root.as_deref(),
        );
        self.emit_transfers(events);
        Self::ack(ack, result);
    }

    pub(super) fn handle_folder_contents_response(
        &mut self,
        username: String,
        directory: String,
        folders: Vec<FolderContents>,
    ) {
        let Some(files) = self.folder_requests.accept(&username, &directory, folders) else {
            return;
        };
        if files.len() > FOLDER_DOWNLOAD_FILE_LIMIT {
            warn!(
                username,
                directory,
                files = files.len(),
                "folder contents exceed the download limit, refusing"
            );
            self.emit(ClientEvent::FolderRequestFailed {
                username,
                directory,
            });
            return;
        }
        let root = destination_root(&directory).to_owned();
        for mut file in files {
            file.name = format!("{directory}\\{}", file.name);
            let (_, events) =
                self.downloads
                    .enqueue(&mut self.transfer_ids, username.clone(), file, Some(&root));
            self.emit_transfers(events);
        }
    }

    pub(super) fn retry_download(&mut self, id: TransferId, ack: oneshot::Sender<RetryResult>) {
        let (result, events) = self.downloads.retry(&mut self.transfer_ids, id);
        self.emit_transfers(events);
        Self::ack(ack, result);
    }

    pub(super) fn abort_transfer(
        &mut self,
        direction: TransferDirection,
        id: TransferId,
        ack: oneshot::Sender<AbortResult>,
    ) {
        let (result, events) = match direction {
            TransferDirection::Download => self.downloads.abort(id),
            TransferDirection::Upload => self.uploads.abort(id, &self.users),
        };
        self.emit_transfers(events);
        Self::ack(ack, result);
    }

    pub(super) fn clear_transfers(
        &mut self,
        direction: TransferDirection,
        statuses: Vec<TransferStatus>,
        ack: oneshot::Sender<()>,
    ) {
        let ids = match direction {
            TransferDirection::Download => self.downloads.clear(&statuses),
            TransferDirection::Upload => self.uploads.clear(&statuses),
        };
        if !ids.is_empty() {
            self.emit_transfer_work(TransferWork::Removed { direction, ids });
        }
        Self::ack(ack, ());
    }

    pub(super) fn clear_all_transfers(
        &mut self,
        direction: TransferDirection,
        ack: oneshot::Sender<()>,
    ) {
        let ids = match direction {
            TransferDirection::Download => self.downloads.clear_all(),
            TransferDirection::Upload => self.uploads.clear_all(),
        };
        if !ids.is_empty() {
            self.emit_transfer_work(TransferWork::Removed { direction, ids });
        }
        Self::ack(ack, ());
    }

    pub(super) fn set_user_restriction(&mut self, username: String, restriction: Restriction) {
        if let Restriction::Denied { reason } = &restriction {
            let updates = self.uploads.deny_all(&username, reason, &self.users);
            self.emit_transfers(updates);
        }
        self.users.set_restriction(username, restriction);
        self.uploads.check_queue(&self.users);
    }

    pub(super) fn deny_file(&mut self, username: String, virtual_path: String, ttl: Duration) {
        let key = (username, virtual_path);
        let updates = self.uploads.deny_file(&key, &self.users);
        self.emit_transfers(updates);
        self.users.deny_file(key.0, key.1, Instant::now() + ttl);
    }

    pub(super) fn sweep(&mut self) {
        for (username, directory) in self.folder_requests.sweep() {
            self.emit(ClientEvent::FolderRequestFailed {
                username,
                directory,
            });
        }
        let downloads = self.downloads.sweep_request_timeouts();
        self.emit_transfers(downloads);
        let uploads = self.uploads.sweep_request_timeouts(&self.users);
        self.emit_transfers(uploads);
        if self.session.logged_in {
            self.downloads.request_queue_positions();
        }
    }

    pub(super) fn handle_peer_connection_error(
        &mut self,
        username: &str,
        unsent: &[PeerMessage],
        is_offline: bool,
    ) {
        for message in unsent {
            match message {
                PeerMessage::SharedFileListRequest => {
                    self.net
                        .send(NetworkCommand::DisallowSharedListUser(username.to_owned()));
                }
                PeerMessage::UserInfoRequest => {
                    self.net
                        .send(NetworkCommand::DisallowUserInfoUser(username.to_owned()));
                }
                PeerMessage::FolderContentsRequest { directory, .. } => {
                    let key = (username.to_owned(), directory.clone());
                    if let Some((username, directory)) = self.folder_requests.time_out(key) {
                        self.emit(ClientEvent::FolderRequestFailed {
                            username,
                            directory,
                        });
                    }
                }
                _ => {}
            }
        }
        let downloads = self
            .downloads
            .handle_peer_connection_error(username, unsent, is_offline);
        self.emit_transfers(downloads);
        let uploads =
            self.uploads
                .handle_peer_connection_error(username, unsent, is_offline, &self.users);
        self.emit_transfers(uploads);
    }

    pub(super) fn handle_file_transfer_init(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
        direction: TransferDirection,
    ) {
        let updates = match direction {
            TransferDirection::Download => self
                .downloads
                .handle_file_transfer_init(username, token, conn_id),
            TransferDirection::Upload => {
                self.uploads
                    .handle_file_transfer_init(username, token, conn_id, &self.users)
            }
        };
        self.emit_transfers(updates);
    }

    pub(super) fn handle_file_connection_closed(
        &mut self,
        username: &str,
        token: u32,
        conn_id: ConnId,
        direction: TransferDirection,
    ) {
        let updates = match direction {
            TransferDirection::Download => self.downloads.handle_file_connection_closed(conn_id),
            TransferDirection::Upload => {
                self.uploads
                    .handle_file_connection_closed(username, token, conn_id, &self.users)
            }
        };
        self.emit_transfers(updates);
    }
}
