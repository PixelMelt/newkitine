use super::ClientActor;
use crate::client::{ClientEvent, Observation, SearchResult, UserInfoReceived};
use crate::network::NetworkCommand;
use crate::protocol::PeerMessage;
use crate::types::TransferDirection;

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
                if !self.users.is_ignored(&username) {
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
                    let updates = self
                        .downloads
                        .handle_transfer_request(&username, token, &file, filesize);
                    self.emit_transfers(updates);
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
            PeerMessage::TransferResponse { token, reason, .. } => {
                let updates = self.uploads.handle_transfer_response(
                    &username,
                    token,
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
            PeerMessage::UserInfoRequest => self.respond_to_user_info(username),
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
            PeerMessage::FolderContentsResponse {
                directory, folders, ..
            } => self.handle_folder_contents_response(username, directory, folders),
            _ => {}
        }
    }
}
