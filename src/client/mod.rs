mod actor;
mod bootstrap;
mod event;
mod observation;
mod search;
mod shares;
mod transfers;
mod users;

pub use bootstrap::ClientBootstrap;
pub use event::{ClientEvent, SearchResult, UserInfoReceived};
pub use observation::Observation;
pub use search::SearchScope;
pub use transfers::{AbortResult, EnqueueResult, RetryResult, TransferWork};

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::{mpsc, oneshot};

use crate::protocol::{ServerRequest, initial_token};
use crate::types::{
    FileAttributes, Restriction, RuntimeConfig, TransferDirection, TransferId, TransferStatus,
};

pub const COMMAND_QUEUE_CAPACITY: usize = 1024;
pub const EVENT_QUEUE_CAPACITY: usize = 4096;
pub const TRANSFER_QUEUE_CAPACITY: usize = 4096;

#[derive(Debug)]
pub(crate) enum ClientCommand {
    Search {
        token: u32,
        query: String,
        scope: SearchScope,
    },
    CancelSearch {
        token: u32,
    },
    Download {
        username: String,
        virtual_path: String,
        size: u64,
        attributes: FileAttributes,
        ack: oneshot::Sender<EnqueueResult>,
    },
    RetryDownload {
        id: TransferId,
        ack: oneshot::Sender<RetryResult>,
    },
    AbortTransfer {
        direction: TransferDirection,
        id: TransferId,
        ack: oneshot::Sender<AbortResult>,
    },
    ClearTransfers {
        direction: TransferDirection,
        statuses: Vec<TransferStatus>,
        ack: oneshot::Sender<()>,
    },
    ClearAllTransfers {
        direction: TransferDirection,
        ack: oneshot::Sender<()>,
    },
    BrowseUser {
        username: String,
    },
    RequestUserInfo {
        username: String,
    },
    AddBuddy {
        username: String,
    },
    RemoveBuddy {
        username: String,
    },
    BanUser {
        username: String,
    },
    UnbanUser {
        username: String,
    },
    IgnoreUser {
        username: String,
    },
    UnignoreUser {
        username: String,
    },
    SetIpBans {
        patterns: Vec<String>,
    },
    SetUserRestriction {
        username: String,
        restriction: Restriction,
    },
    AddInterest {
        thing: String,
        hated: bool,
    },
    RemoveInterest {
        thing: String,
        hated: bool,
    },
    AddWish {
        term: String,
    },
    RemoveWish {
        term: String,
    },
    RescanShares,
    ApplyConfig {
        config: Box<RuntimeConfig>,
        ack: oneshot::Sender<()>,
    },
    Server(ServerRequest),
    Connect,
    Reconnect,
    Disconnect,
}

pub struct Client {
    commands: mpsc::Sender<ClientCommand>,
    token_counter: Arc<AtomicU32>,
}

impl Client {
    pub fn spawn(
        config: ClientBootstrap,
    ) -> (
        Self,
        mpsc::Receiver<ClientEvent>,
        mpsc::Receiver<TransferWork>,
    ) {
        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let (transfers_tx, transfers_rx) = mpsc::channel(TRANSFER_QUEUE_CAPACITY);
        let token_counter = Arc::new(AtomicU32::new(initial_token()));
        tokio::spawn(actor::run(
            config,
            commands_rx,
            events_tx,
            transfers_tx,
            token_counter.clone(),
        ));
        (
            Self {
                commands: commands_tx,
                token_counter,
            },
            events_rx,
            transfers_rx,
        )
    }

    async fn send(&self, command: ClientCommand) {
        self.commands
            .send(command)
            .await
            .expect("client actor terminated");
    }

    pub async fn search(&self, query: &str, scope: SearchScope) -> u32 {
        let token = self.token_counter.fetch_add(1, Ordering::Relaxed) + 1;
        self.send(ClientCommand::Search {
            token,
            query: query.to_owned(),
            scope,
        })
        .await;
        token
    }

    pub async fn cancel_search(&self, token: u32) {
        self.send(ClientCommand::CancelSearch { token }).await;
    }

    pub async fn download(
        &self,
        username: &str,
        virtual_path: &str,
        size: u64,
        attributes: FileAttributes,
    ) -> EnqueueResult {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::Download {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
            size,
            attributes,
            ack,
        })
        .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn retry_download(&self, id: TransferId) -> RetryResult {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::RetryDownload { id, ack }).await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn abort_transfer(
        &self,
        direction: TransferDirection,
        id: TransferId,
    ) -> AbortResult {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::AbortTransfer { direction, id, ack })
            .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn clear_transfers(
        &self,
        direction: TransferDirection,
        statuses: Vec<TransferStatus>,
    ) {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::ClearTransfers {
            direction,
            statuses,
            ack,
        })
        .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn clear_all_transfers(&self, direction: TransferDirection) {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::ClearAllTransfers { direction, ack })
            .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn browse_user(&self, username: &str) {
        self.send(ClientCommand::BrowseUser {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn request_user_info(&self, username: &str) {
        self.send(ClientCommand::RequestUserInfo {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn add_buddy(&self, username: &str) {
        self.send(ClientCommand::AddBuddy {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn remove_buddy(&self, username: &str) {
        self.send(ClientCommand::RemoveBuddy {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn ban_user(&self, username: &str) {
        self.send(ClientCommand::BanUser {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn unban_user(&self, username: &str) {
        self.send(ClientCommand::UnbanUser {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn ignore_user(&self, username: &str) {
        self.send(ClientCommand::IgnoreUser {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn unignore_user(&self, username: &str) {
        self.send(ClientCommand::UnignoreUser {
            username: username.to_owned(),
        })
        .await;
    }

    pub async fn set_ip_bans(&self, patterns: Vec<String>) {
        self.send(ClientCommand::SetIpBans { patterns }).await;
    }

    pub async fn set_user_restriction(&self, username: &str, restriction: Restriction) {
        self.send(ClientCommand::SetUserRestriction {
            username: username.to_owned(),
            restriction,
        })
        .await;
    }

    pub async fn add_wish(&self, term: &str) {
        self.send(ClientCommand::AddWish {
            term: term.to_owned(),
        })
        .await;
    }

    pub async fn remove_wish(&self, term: &str) {
        self.send(ClientCommand::RemoveWish {
            term: term.to_owned(),
        })
        .await;
    }

    pub async fn rescan_shares(&self) {
        self.send(ClientCommand::RescanShares).await;
    }

    pub async fn apply_config(&self, config: RuntimeConfig) {
        let (ack, done) = oneshot::channel();
        self.send(ClientCommand::ApplyConfig {
            config: Box::new(config),
            ack,
        })
        .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    pub async fn reconnect(&self) {
        self.send(ClientCommand::Reconnect).await;
    }

    pub async fn add_liked_interest(&self, thing: &str) {
        self.send(ClientCommand::AddInterest {
            thing: thing.to_owned(),
            hated: false,
        })
        .await;
    }

    pub async fn remove_liked_interest(&self, thing: &str) {
        self.send(ClientCommand::RemoveInterest {
            thing: thing.to_owned(),
            hated: false,
        })
        .await;
    }

    pub async fn add_hated_interest(&self, thing: &str) {
        self.send(ClientCommand::AddInterest {
            thing: thing.to_owned(),
            hated: true,
        })
        .await;
    }

    pub async fn remove_hated_interest(&self, thing: &str) {
        self.send(ClientCommand::RemoveInterest {
            thing: thing.to_owned(),
            hated: true,
        })
        .await;
    }

    pub async fn request_recommendations(&self) {
        self.send(ClientCommand::Server(ServerRequest::Recommendations))
            .await;
    }

    pub async fn request_global_recommendations(&self) {
        self.send(ClientCommand::Server(ServerRequest::GlobalRecommendations))
            .await;
    }

    pub async fn request_similar_users(&self) {
        self.send(ClientCommand::Server(ServerRequest::SimilarUsers))
            .await;
    }

    pub async fn request_item_recommendations(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::ItemRecommendations {
            thing: thing.to_owned(),
        }))
        .await;
    }

    pub async fn request_item_similar_users(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::ItemSimilarUsers {
            thing: thing.to_owned(),
        }))
        .await;
    }

    pub async fn request_user_stats(&self, username: &str) {
        self.send(ClientCommand::Server(ServerRequest::GetUserStats {
            user: username.to_owned(),
        }))
        .await;
    }

    pub async fn request_user_interests(&self, username: &str) {
        self.send(ClientCommand::Server(ServerRequest::UserInterests {
            user: username.to_owned(),
        }))
        .await;
    }

    pub async fn join_room(&self, room: &str) {
        self.send(ClientCommand::Server(ServerRequest::JoinRoom {
            room: room.to_owned(),
            private: false,
        }))
        .await;
    }

    pub async fn leave_room(&self, room: &str) {
        self.send(ClientCommand::Server(ServerRequest::LeaveRoom {
            room: room.to_owned(),
        }))
        .await;
    }

    pub async fn say_room(&self, room: &str, message: &str) {
        self.send(ClientCommand::Server(ServerRequest::SayChatroom {
            room: room.to_owned(),
            message: message.to_owned(),
        }))
        .await;
    }

    pub async fn send_private_message(&self, username: &str, message: &str) {
        self.send(ClientCommand::Server(ServerRequest::MessageUser {
            user: username.to_owned(),
            message: message.to_owned(),
        }))
        .await;
    }

    pub async fn request_room_list(&self) {
        self.send(ClientCommand::Server(ServerRequest::RoomList))
            .await;
    }

    pub async fn connect(&self) {
        self.send(ClientCommand::Connect).await;
    }

    pub async fn disconnect(&self) {
        self.send(ClientCommand::Disconnect).await;
    }
}
