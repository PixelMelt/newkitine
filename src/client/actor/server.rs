use super::ClientActor;
use crate::client::ClientEvent;
use crate::client::users::Presence;
use crate::protocol::{ServerRequest, ServerResponse};
use crate::types::UserStatus;

impl ClientActor {
    pub(super) fn handle_server_message(&mut self, message: ServerResponse) {
        match message {
            ServerResponse::GetUserStatus {
                user,
                status,
                privileged,
            } => {
                let status = UserStatus::from_u32(status);
                let updates = match self
                    .users
                    .handle_user_status(&self.net, &user, status, privileged)
                {
                    Presence::WentOffline => self.downloads.user_offline(&user),
                    Presence::CameOnline => self.downloads.user_online(&mut self.users, &user),
                    Presence::Unchanged => Vec::new(),
                };
                self.emit_transfers(updates);
                self.emit(ClientEvent::UserStatus {
                    username: user,
                    status,
                    privileged,
                });
            }
            ServerResponse::ConnectToPeer {
                user, privileged, ..
            } => {
                self.users.set_privileged(&user, privileged);
            }
            ServerResponse::AddToPrivileged { user } => {
                self.users.set_privileged(&user, true);
            }
            ServerResponse::WatchUser {
                user,
                user_exists,
                status,
                stats,
                country: _,
            } => {
                self.emit(ClientEvent::WatchedUser {
                    username: user,
                    exists: user_exists,
                    status: status.and_then(UserStatus::from_u32),
                    stats,
                });
            }
            ServerResponse::GetUserStats { user, stats } => {
                self.emit(ClientEvent::UserStats {
                    username: user,
                    stats,
                });
            }
            ServerResponse::PrivilegedUsers { users } => {
                self.users.handle_privileged_users(users);
            }
            ServerResponse::FileSearch {
                search_username,
                token,
                search_term,
            } => {
                self.respond_to_search(&search_username, token, &search_term);
            }
            ServerResponse::ExcludedSearchPhrases { phrases } => {
                self.sharing.excluded_phrases = phrases
                    .into_iter()
                    .map(|phrase| phrase.to_lowercase())
                    .collect();
            }
            ServerResponse::WishlistInterval { seconds } => {
                self.set_wishlist_interval(seconds);
            }
            ServerResponse::MessageUser {
                message_id,
                timestamp,
                user,
                message,
                ..
            } => {
                self.net.server(ServerRequest::MessageAcked { message_id });
                if !self.users.is_ignored(&user) {
                    self.emit(ClientEvent::PrivateMessage {
                        username: user,
                        message,
                        timestamp,
                    });
                }
            }
            ServerResponse::SayChatroom {
                room,
                user,
                message,
            } => {
                if !self.users.is_ignored(&user) {
                    self.emit(ClientEvent::RoomMessage {
                        room,
                        username: user,
                        message,
                    });
                }
            }
            ServerResponse::RoomList { rooms, .. } => {
                self.emit(ClientEvent::RoomList { rooms });
            }
            ServerResponse::JoinRoom { room, users, .. } => {
                self.emit(ClientEvent::RoomJoined {
                    room,
                    users: users.into_iter().map(|user| user.username).collect(),
                });
            }
            ServerResponse::LeaveRoom { room } => {
                self.emit(ClientEvent::RoomLeft { room });
            }
            ServerResponse::UserJoinedRoom { room, userdata } => {
                self.emit(ClientEvent::RoomUserJoined {
                    room,
                    username: userdata.username,
                });
            }
            ServerResponse::UserLeftRoom { room, username } => {
                self.emit(ClientEvent::RoomUserLeft { room, username });
            }
            ServerResponse::UserInterests { user, likes, hates } => {
                self.emit(ClientEvent::UserInterests {
                    username: user,
                    liked: likes,
                    hated: hates,
                });
            }
            ServerResponse::Recommendations {
                recommendations,
                unrecommendations,
            } => {
                self.emit(ClientEvent::Recommendations {
                    recommendations,
                    unrecommendations,
                });
            }
            ServerResponse::GlobalRecommendations {
                recommendations,
                unrecommendations,
            } => {
                self.emit(ClientEvent::GlobalRecommendations {
                    recommendations,
                    unrecommendations,
                });
            }
            ServerResponse::ItemRecommendations {
                thing,
                recommendations,
                unrecommendations,
            } => {
                self.emit(ClientEvent::ItemRecommendations {
                    thing,
                    recommendations,
                    unrecommendations,
                });
            }
            ServerResponse::SimilarUsers { users } => {
                self.emit(ClientEvent::SimilarUsers(users));
            }
            ServerResponse::ItemSimilarUsers { thing, usernames } => {
                self.emit(ClientEvent::ItemSimilarUsers { thing, usernames });
            }
            ServerResponse::CheckPrivileges { seconds } => {
                self.emit(ClientEvent::Privileges { seconds });
            }
            ServerResponse::AdminMessage { msg } => {
                self.emit(ClientEvent::AdminMessage { message: msg });
            }
            ServerResponse::GetPeerAddress {
                user, ip_address, ..
            } => {
                self.emit(ClientEvent::PeerAddress {
                    username: user,
                    ip: ip_address,
                });
            }
            ServerResponse::Login(_)
            | ServerResponse::IgnoreUser { .. }
            | ServerResponse::UnignoreUser { .. }
            | ServerResponse::ServerPing
            | ServerResponse::SendConnectToken { .. }
            | ServerResponse::UploadSlotsFull { .. }
            | ServerResponse::Relogged
            | ServerResponse::SimilarRecommendations { .. }
            | ServerResponse::MyRecommendations { .. }
            | ServerResponse::PlaceInLineRequest { .. }
            | ServerResponse::PlaceInLineResponse { .. }
            | ServerResponse::RoomAdded { .. }
            | ServerResponse::RoomRemoved { .. }
            | ServerResponse::ExactFileSearch { .. }
            | ServerResponse::GlobalUserList { .. }
            | ServerResponse::TunneledMessage { .. }
            | ServerResponse::ParentMinSpeed { .. }
            | ServerResponse::ParentSpeedRatio { .. }
            | ServerResponse::ParentInactivityTimeout { .. }
            | ServerResponse::SearchInactivityTimeout { .. }
            | ServerResponse::MinParentsInCache { .. }
            | ServerResponse::DistribPingInterval { .. }
            | ServerResponse::EmbeddedMessage { .. }
            | ServerResponse::PossibleParents { .. }
            | ServerResponse::RoomTickers { .. }
            | ServerResponse::RoomTickerAdded { .. }
            | ServerResponse::RoomTickerRemoved { .. }
            | ServerResponse::UserPrivileged { .. }
            | ServerResponse::NotifyPrivileges { .. }
            | ServerResponse::AckNotifyPrivileges { .. }
            | ServerResponse::ResetDistributed
            | ServerResponse::RoomMembers { .. }
            | ServerResponse::AddRoomMember { .. }
            | ServerResponse::RemoveRoomMember { .. }
            | ServerResponse::RoomSomething { .. }
            | ServerResponse::RoomMembershipGranted { .. }
            | ServerResponse::RoomMembershipRevoked { .. }
            | ServerResponse::EnableRoomInvitations { .. }
            | ServerResponse::ChangePassword { .. }
            | ServerResponse::AddRoomOperator { .. }
            | ServerResponse::RemoveRoomOperator { .. }
            | ServerResponse::RoomOperatorshipGranted { .. }
            | ServerResponse::RoomOperatorshipRevoked { .. }
            | ServerResponse::RoomOperators { .. }
            | ServerResponse::GlobalRoomMessage { .. }
            | ServerResponse::RelatedSearch { .. }
            | ServerResponse::CantConnectToPeer { .. }
            | ServerResponse::CantCreateRoom { .. } => {}
        }
    }
}
