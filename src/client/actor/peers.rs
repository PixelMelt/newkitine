use super::ClientActor;
use crate::protocol::PeerMessage;
use crate::types::{
    ClientEvent, NetworkCommand, Observation, SearchResult, TransferDirection, UserInfoReceived,
};

impl ClientActor {
    pub(super) fn handle_peer_message(&mut self, username: String, message: PeerMessage) {
        match message {
            PeerMessage::FileSearchResponse {
                token,
                results,
                free_upload_slots,
                upload_speed,
                queue_size,
                ..
            } => {
                if self.searches.contains(token) && !self.users.is_ignored(&username) {
                    self.emit(ClientEvent::SearchResults(SearchResult {
                        token,
                        username,
                        results,
                        free_upload_slots,
                        upload_speed,
                        queue_size,
                    }));
                }
            }
            PeerMessage::TransferRequest {
                direction,
                token,
                file,
                filesize,
            } => match direction {
                TransferDirection::Upload => {
                    self.downloads
                        .handle_transfer_request(&username, token, &file, filesize);
                }
                TransferDirection::Download => {
                    let (updates, accepted) = self.uploads.handle_legacy_transfer_request(
                        &mut self.transfer_ids,
                        &username,
                        token,
                        &file,
                        self.sharing.index.as_ref(),
                        &self.users,
                    );
                    self.emit(ClientEvent::Observed(Observation::QueueRequest {
                        username: username.clone(),
                        virtual_path: file,
                        accepted,
                    }));
                    self.emit_transfers(updates);
                }
            },
            PeerMessage::TransferResponse {
                token,
                allowed,
                reason,
                ..
            } => {
                let updates = self.uploads.handle_transfer_response(
                    &username,
                    token,
                    allowed,
                    reason.as_deref(),
                    &self.users,
                );
                self.emit_transfers(updates);
            }
            PeerMessage::UploadDenied { file, reason } => {
                let updates = self
                    .downloads
                    .handle_upload_denied(&username, &file, &reason);
                self.emit_transfers(updates);
            }
            PeerMessage::UploadFailed { file } => {
                let updates = self.downloads.handle_upload_failed(&username, &file);
                self.emit_transfers(updates);
            }
            PeerMessage::PlaceInQueueResponse { filename, place } => {
                if let Some(work) = self.downloads.queue_place(&username, &filename, place) {
                    self.emit_transfer_work(work);
                }
            }
            PeerMessage::PlaceInQueueRequest { file, .. } => {
                self.uploads.handle_place_in_queue_request(&username, &file);
            }
            PeerMessage::SharedFileListResponse {
                shares,
                private_shares,
                ..
            } => {
                self.net
                    .send(NetworkCommand::DisallowSharedListUser(username.clone()));
                self.emit(ClientEvent::SharedFileList {
                    username,
                    shares,
                    private_shares,
                });
            }
            PeerMessage::UserInfoResponse {
                description,
                picture,
                total_uploads,
                queue_size,
                slots_available,
                ..
            } => {
                self.net
                    .send(NetworkCommand::DisallowUserInfoUser(username.clone()));
                self.emit(ClientEvent::UserInfo(UserInfoReceived {
                    username,
                    description,
                    picture,
                    total_uploads,
                    queue_size,
                    slots_available,
                }));
            }
            PeerMessage::SharedFileListRequest => self.handle_browse_request(username),
            PeerMessage::UserInfoRequest => {
                self.emit(ClientEvent::Observed(Observation::UserInfoRequest {
                    username: username.clone(),
                }));
                self.net.peer(
                    username,
                    PeerMessage::UserInfoResponse {
                        description: self.config.description.clone(),
                        picture: None,
                        total_uploads: self.uploads.total_slots(),
                        queue_size: self.uploads.queue_size(),
                        slots_available: self.uploads.is_new_upload_accepted(),
                        upload_allowed: Some(0),
                    },
                );
            }
            PeerMessage::QueueUpload { file, .. } => {
                let (updates, accepted) = self.uploads.handle_queue_upload(
                    &mut self.transfer_ids,
                    &username,
                    &file,
                    self.sharing.index.as_ref(),
                    &self.users,
                );
                self.emit(ClientEvent::Observed(Observation::QueueRequest {
                    username: username.clone(),
                    virtual_path: file,
                    accepted,
                }));
                self.emit_transfers(updates);
            }
            PeerMessage::FolderContentsRequest {
                token, directory, ..
            } => self.handle_folder_contents_request(username, token, directory),
            _ => {}
        }
    }
}
