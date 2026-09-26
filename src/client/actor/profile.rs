use super::ClientActor;
use crate::client::{ClientEvent, Observation};
use crate::protocol::PeerMessage;
use crate::types::{DescriptionContext, Restriction};

impl ClientActor {
    pub(super) fn respond_to_user_info(&mut self, username: String) {
        self.emit(ClientEvent::Observed(Observation::UserInfoRequest {
            username: username.clone(),
        }));
        let response = if self.users.is_banned(&username) {
            PeerMessage::UserInfoResponse {
                description: format!(
                    "You are not allowed to download my shared files.\nReason: {}",
                    self.config.transfers.banned_message
                ),
                picture: None,
                total_uploads: 0,
                queue_size: 0,
                slots_available: false,
                upload_allowed: Some(0),
            }
        } else {
            PeerMessage::UserInfoResponse {
                description: self.render_description(&username),
                picture: None,
                total_uploads: self.uploads.total_slots(),
                queue_size: self.uploads.queue_size(),
                slots_available: self.uploads.is_new_upload_accepted(),
                upload_allowed: Some(0),
            }
        };
        self.net.peer(username, response);
    }

    fn render_description(&self, username: &str) -> String {
        let (folders, files) = self.sharing.counts();
        self.config.description.render(&DescriptionContext {
            user_name: username,
            user_restriction: self
                .users
                .restriction(username)
                .unwrap_or(&Restriction::None)
                .as_str(),
            user_queued_files: self.uploads.queued_for(username),
            user_active_uploads: self.uploads.active_uploads(username),
            user_is_buddy: self.users.is_buddy(username),
            user_is_ignored: self.users.is_ignored(username),
            user_is_banned: self.users.is_banned(username),
            user_is_privileged: self.users.is_privileged(username),
            my_name: &self.config.login.username,
            my_shared_files: files,
            my_shared_folders: folders,
            my_queue_size: self.uploads.queue_size(),
            my_slots: self.uploads.total_slots(),
            my_free_slots: self.uploads.free_slots(),
            my_upload_speed: self.uploads.upload_speed,
        })
    }
}
