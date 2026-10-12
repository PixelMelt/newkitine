use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use super::sharing::ScanJob;
use super::{ClientActor, Command};
use crate::client::search::SearchQuery;
use crate::client::{
    AbortResult, COMMAND_QUEUE_CAPACITY, ClientBootstrap, ClientEvent, EVENT_QUEUE_CAPACITY,
    EnqueueResult, RetryResult, SearchScope, TRANSFER_QUEUE_CAPACITY, TransferWork,
};
use crate::protocol::{ServerRequest, initial_token};
use crate::types::{
    FileAttributes, Restriction, RuntimeConfig, TransferDirection, TransferId, TransferStatus,
};

pub struct Client {
    commands: mpsc::Sender<Command>,
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
        tokio::spawn(super::run(
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

    async fn run(&self, command: impl FnOnce(&mut ClientActor) + Send + 'static) {
        if self.commands.send(Box::new(command)).await.is_err() {
            panic!("client actor terminated");
        }
    }

    async fn ask<T: Send + 'static>(
        &self,
        command: impl FnOnce(&mut ClientActor) -> T + Send + 'static,
    ) -> T {
        let (ack, done) = oneshot::channel();
        self.run(move |actor| {
            if ack.send(command(actor)).is_err() {
                debug!("command acknowledgement dropped, caller went away");
            }
        })
        .await;
        done.await.expect("client actor dropped acknowledgement")
    }

    async fn server(&self, request: ServerRequest) {
        self.run(move |actor| actor.net.server(request)).await;
    }

    pub async fn search(&self, query: &str, scope: SearchScope) -> Option<u32> {
        let filter = SearchQuery::parse(query);
        if filter.transmitted.is_empty() {
            return None;
        }
        let token = self.token_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let query = query.to_owned();
        self.run(move |actor| actor.start_search(token, query, filter, scope))
            .await;
        Some(token)
    }

    pub async fn cancel_search(&self, token: u32) {
        self.run(move |actor| actor.cancel_search(token)).await;
    }

    pub async fn download(
        &self,
        username: &str,
        virtual_path: &str,
        size: u64,
        attributes: FileAttributes,
        root: Option<&str>,
    ) -> EnqueueResult {
        let username = username.to_owned();
        let virtual_path = virtual_path.to_owned();
        let root = root.map(str::to_owned);
        self.ask(move |actor| {
            actor.enqueue_download(username, virtual_path, size, attributes, root)
        })
        .await
    }

    pub async fn request_folder(&self, username: &str, directory: &str) {
        let username = username.to_owned();
        let directory = directory.to_owned();
        self.run(move |actor| actor.folder_requests.request(username, directory))
            .await;
    }

    pub async fn retry_download(&self, id: TransferId) -> RetryResult {
        self.ask(move |actor| actor.retry_download(id)).await
    }

    pub async fn abort_transfer(
        &self,
        direction: TransferDirection,
        id: TransferId,
    ) -> AbortResult {
        self.ask(move |actor| actor.abort_transfer(direction, id))
            .await
    }

    pub async fn clear_transfers(
        &self,
        direction: TransferDirection,
        statuses: Option<Vec<TransferStatus>>,
    ) {
        self.ask(move |actor| actor.clear_transfers(direction, statuses.as_deref()))
            .await
    }

    pub async fn browse_user(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.browse_user(username)).await;
    }

    pub async fn request_user_info(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.request_user_info(username))
            .await;
    }

    pub async fn add_buddy(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.users.add_buddy(&actor.net, username))
            .await;
    }

    pub async fn remove_buddy(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.remove_buddy(&username)).await;
    }

    pub async fn ban_user(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.ban_user(username)).await;
    }

    pub async fn unban_user(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| {
            actor.users.banned.remove(&username);
        })
        .await;
    }

    pub async fn ignore_user(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| {
            actor.users.ignored.insert(username);
        })
        .await;
    }

    pub async fn unignore_user(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| {
            actor.users.ignored.remove(&username);
        })
        .await;
    }

    pub async fn set_ip_bans(&self, patterns: Vec<String>) {
        self.run(move |actor| actor.set_ip_bans(patterns)).await;
    }

    pub async fn set_user_restriction(&self, username: &str, restriction: Restriction) {
        let username = username.to_owned();
        self.run(move |actor| actor.set_user_restriction(username, restriction))
            .await;
    }

    pub async fn deny_file(&self, username: &str, virtual_path: &str, ttl: Duration) {
        let username = username.to_owned();
        let virtual_path = virtual_path.to_owned();
        self.run(move |actor| actor.deny_file(username, virtual_path, ttl))
            .await;
    }

    pub async fn clear_file_denials(&self, username: &str) {
        let username = username.to_owned();
        self.run(move |actor| actor.users.clear_file_denials(&username))
            .await;
    }

    pub async fn set_interest(&self, thing: &str, hated: bool, present: bool) {
        let thing = thing.to_owned();
        self.run(move |actor| actor.set_interest(thing, hated, present))
            .await;
    }

    pub async fn set_wish(&self, term: &str, present: bool) {
        let term = term.to_owned();
        self.run(move |actor| {
            if present {
                actor.add_wish(term);
            } else {
                actor.remove_wish(&term);
            }
        })
        .await;
    }

    pub async fn rescan_shares(&self) {
        self.run(|actor| actor.start_scan(ScanJob::RESCAN)).await;
    }

    pub async fn apply_config(&self, config: RuntimeConfig) {
        self.ask(move |actor| actor.apply_config(config)).await
    }

    pub async fn request_recommendations(&self) {
        self.server(ServerRequest::Recommendations).await;
    }

    pub async fn request_global_recommendations(&self) {
        self.server(ServerRequest::GlobalRecommendations).await;
    }

    pub async fn request_similar_users(&self) {
        self.server(ServerRequest::SimilarUsers).await;
    }

    pub async fn request_item_recommendations(&self, thing: &str) {
        let thing = thing.to_owned();
        self.server(ServerRequest::ItemRecommendations { thing })
            .await;
    }

    pub async fn request_item_similar_users(&self, thing: &str) {
        let thing = thing.to_owned();
        self.server(ServerRequest::ItemSimilarUsers { thing }).await;
    }

    pub async fn request_user_stats(&self, username: &str) {
        let user = username.to_owned();
        self.server(ServerRequest::GetUserStats { user }).await;
    }

    pub async fn request_user_interests(&self, username: &str) {
        let user = username.to_owned();
        self.server(ServerRequest::UserInterests { user }).await;
    }

    pub async fn join_room(&self, room: &str) {
        let room = room.to_owned();
        self.server(ServerRequest::JoinRoom {
            room,
            private: false,
        })
        .await;
    }

    pub async fn leave_room(&self, room: &str) {
        let room = room.to_owned();
        self.server(ServerRequest::LeaveRoom { room }).await;
    }

    pub async fn say_room(&self, room: &str, message: &str) {
        self.server(ServerRequest::SayChatroom {
            room: room.to_owned(),
            message: message.to_owned(),
        })
        .await;
    }

    pub async fn send_private_message(&self, username: &str, message: &str) {
        self.server(ServerRequest::MessageUser {
            user: username.to_owned(),
            message: message.to_owned(),
        })
        .await;
    }

    pub async fn request_room_list(&self) {
        self.server(ServerRequest::RoomList).await;
    }

    pub async fn connect(&self) {
        self.run(ClientActor::request_connect).await;
    }

    pub async fn reconnect(&self) {
        self.run(ClientActor::reconnect).await;
    }

    pub async fn disconnect(&self) {
        self.run(ClientActor::disconnect).await;
    }
}
