mod actor;
mod downloads;
mod search;
mod shares;
mod uploads;
mod users;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::mpsc;

pub use downloads::{DownloadUpdate, Transfer, TransferStatus};
pub use shares::SharedFolder;
pub use uploads::{DEFAULT_QUEUE_FILE_LIMIT, DEFAULT_UPLOAD_SLOTS, UploadUpdate};

use crate::network::ConnId;
use crate::protocol::{PeerMessage, ServerRequest, ServerResponse, initial_token};
use crate::types::{
    FileAttributes, FileInfo, FolderContents, Observation, Restriction, SearchScope, UserStatus,
};

pub const DEFAULT_SERVER: &str = "server.slsknet.org:2242";

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server: SocketAddr,
    pub username: String,
    pub password: String,
    pub listen_port: u16,
    pub download_dir: PathBuf,
    pub incomplete_dir: PathBuf,
    pub description: String,
    pub shared_folders: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub buddies: Vec<String>,
    pub banned: Vec<String>,
    pub ignored: Vec<String>,
    pub ip_bans: Vec<String>,
    pub wishlist: Vec<String>,
    pub liked_interests: Vec<String>,
    pub hated_interests: Vec<String>,
}

impl ClientConfig {
    pub fn new(
        server: SocketAddr,
        username: impl Into<String>,
        password: impl Into<String>,
        listen_port: u16,
        download_dir: PathBuf,
    ) -> Self {
        Self {
            server,
            username: username.into(),
            password: password.into(),
            listen_port,
            incomplete_dir: download_dir.join("incomplete"),
            download_dir,
            description: String::new(),
            shared_folders: Vec::new(),
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            queue_file_limit: DEFAULT_QUEUE_FILE_LIMIT,
            upload_limit_kbps: 0,
            download_limit_kbps: 0,
            auto_reconnect: true,
            buddies: Vec::new(),
            banned: Vec::new(),
            ignored: Vec::new(),
            ip_bans: Vec::new(),
            wishlist: Vec::new(),
            liked_interests: Vec::new(),
            hated_interests: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum ClientEvent {
    LoggedIn {
        username: String,
        banner: String,
    },
    LoginFailed {
        reason: String,
        detail: Option<String>,
    },
    Disconnected {
        manual: bool,
    },
    ConnectionCount(usize),
    SharesScanned {
        folders: u32,
        files: u32,
    },
    SearchResults {
        token: u32,
        query: String,
        username: String,
        results: Vec<FileInfo>,
        free_upload_slots: bool,
        upload_speed: u32,
        queue_size: u32,
    },
    UserStatus {
        username: String,
        status: Option<UserStatus>,
        privileged: bool,
    },
    Download(DownloadUpdate),
    Upload(UploadUpdate),
    PlaceInQueue {
        username: String,
        virtual_path: String,
        place: u32,
    },
    SharedFileList {
        username: String,
        shares: Vec<FolderContents>,
        private_shares: Vec<FolderContents>,
    },
    UserInfo {
        username: String,
        description: String,
        picture: Option<Vec<u8>>,
        total_uploads: u32,
        queue_size: u32,
        slots_available: bool,
    },
    PrivateMessage {
        username: String,
        message: String,
        timestamp: u32,
        is_new_message: bool,
    },
    RoomMessage {
        room: String,
        username: String,
        message: String,
    },
    Server(ServerResponse),
    Peer {
        username: String,
        conn_id: ConnId,
        message: PeerMessage,
    },
    Observed(Observation),
}

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
    },
    AbortDownload {
        username: String,
        virtual_path: String,
    },
    AbortUpload {
        username: String,
        virtual_path: String,
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
    AddWish {
        term: String,
    },
    RemoveWish {
        term: String,
    },
    RescanShares,
    SetTransferLimits {
        upload_kbps: u32,
        download_kbps: u32,
    },
    SetListenPort {
        port: u16,
    },
    SetLogin {
        server: SocketAddr,
        username: String,
        password: String,
    },
    SetDescription {
        description: String,
    },
    SetSharedFolders {
        folders: Vec<SharedFolder>,
    },
    SetUploadSlots {
        upload_slots: usize,
        queue_file_limit: usize,
    },
    SetDownloadDirs {
        download_dir: PathBuf,
        incomplete_dir: PathBuf,
    },
    SetAutoReconnect {
        enabled: bool,
    },
    Server(ServerRequest),
    Connect,
    Reconnect,
    Disconnect,
}

pub struct Client {
    commands: mpsc::UnboundedSender<ClientCommand>,
    token_counter: Arc<AtomicU32>,
}

impl Client {
    pub fn spawn(config: ClientConfig) -> (Self, mpsc::UnboundedReceiver<ClientEvent>) {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let token_counter = Arc::new(AtomicU32::new(initial_token()));
        tokio::spawn(actor::run(
            config,
            commands_rx,
            events_tx,
            token_counter.clone(),
        ));
        (
            Self {
                commands: commands_tx,
                token_counter,
            },
            events_rx,
        )
    }

    fn send(&self, command: ClientCommand) {
        let _ = self.commands.send(command);
    }

    pub fn search(&self, query: &str, scope: SearchScope) -> u32 {
        let token = self.token_counter.fetch_add(1, Ordering::Relaxed) + 1;
        self.send(ClientCommand::Search {
            token,
            query: query.to_owned(),
            scope,
        });
        token
    }

    pub fn cancel_search(&self, token: u32) {
        self.send(ClientCommand::CancelSearch { token });
    }

    pub fn download(
        &self,
        username: &str,
        virtual_path: &str,
        size: u64,
        attributes: FileAttributes,
    ) {
        self.send(ClientCommand::Download {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
            size,
            attributes,
        });
    }

    pub fn abort_download(&self, username: &str, virtual_path: &str) {
        self.send(ClientCommand::AbortDownload {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
        });
    }

    pub fn abort_upload(&self, username: &str, virtual_path: &str) {
        self.send(ClientCommand::AbortUpload {
            username: username.to_owned(),
            virtual_path: virtual_path.to_owned(),
        });
    }

    pub fn browse_user(&self, username: &str) {
        self.send(ClientCommand::BrowseUser {
            username: username.to_owned(),
        });
    }

    pub fn request_user_info(&self, username: &str) {
        self.send(ClientCommand::RequestUserInfo {
            username: username.to_owned(),
        });
    }

    pub fn watch_user(&self, username: &str) {
        self.send(ClientCommand::Server(ServerRequest::WatchUser {
            user: username.to_owned(),
        }));
    }

    pub fn unwatch_user(&self, username: &str) {
        self.send(ClientCommand::Server(ServerRequest::UnwatchUser {
            user: username.to_owned(),
        }));
    }

    pub fn add_buddy(&self, username: &str) {
        self.send(ClientCommand::AddBuddy {
            username: username.to_owned(),
        });
    }

    pub fn remove_buddy(&self, username: &str) {
        self.send(ClientCommand::RemoveBuddy {
            username: username.to_owned(),
        });
    }

    pub fn ban_user(&self, username: &str) {
        self.send(ClientCommand::BanUser {
            username: username.to_owned(),
        });
    }

    pub fn unban_user(&self, username: &str) {
        self.send(ClientCommand::UnbanUser {
            username: username.to_owned(),
        });
    }

    pub fn ignore_user(&self, username: &str) {
        self.send(ClientCommand::IgnoreUser {
            username: username.to_owned(),
        });
    }

    pub fn unignore_user(&self, username: &str) {
        self.send(ClientCommand::UnignoreUser {
            username: username.to_owned(),
        });
    }

    pub fn set_ip_bans(&self, patterns: Vec<String>) {
        self.send(ClientCommand::SetIpBans { patterns });
    }

    pub fn set_user_restriction(&self, username: &str, restriction: Restriction) {
        self.send(ClientCommand::SetUserRestriction {
            username: username.to_owned(),
            restriction,
        });
    }

    pub fn add_wish(&self, term: &str) {
        self.send(ClientCommand::AddWish {
            term: term.to_owned(),
        });
    }

    pub fn remove_wish(&self, term: &str) {
        self.send(ClientCommand::RemoveWish {
            term: term.to_owned(),
        });
    }

    pub fn rescan_shares(&self) {
        self.send(ClientCommand::RescanShares);
    }

    pub fn set_transfer_limits(&self, upload_kbps: u32, download_kbps: u32) {
        self.send(ClientCommand::SetTransferLimits {
            upload_kbps,
            download_kbps,
        });
    }

    pub fn set_listen_port(&self, port: u16) {
        self.send(ClientCommand::SetListenPort { port });
    }

    pub fn set_login(&self, server: SocketAddr, username: &str, password: &str) {
        self.send(ClientCommand::SetLogin {
            server,
            username: username.to_owned(),
            password: password.to_owned(),
        });
    }

    pub fn set_description(&self, description: &str) {
        self.send(ClientCommand::SetDescription {
            description: description.to_owned(),
        });
    }

    pub fn set_shared_folders(&self, folders: Vec<SharedFolder>) {
        self.send(ClientCommand::SetSharedFolders { folders });
    }

    pub fn set_upload_slots(&self, upload_slots: usize, queue_file_limit: usize) {
        self.send(ClientCommand::SetUploadSlots {
            upload_slots,
            queue_file_limit,
        });
    }

    pub fn set_download_dirs(&self, download_dir: PathBuf, incomplete_dir: PathBuf) {
        self.send(ClientCommand::SetDownloadDirs {
            download_dir,
            incomplete_dir,
        });
    }

    pub fn set_auto_reconnect(&self, enabled: bool) {
        self.send(ClientCommand::SetAutoReconnect { enabled });
    }

    pub fn reconnect(&self) {
        self.send(ClientCommand::Reconnect);
    }

    pub fn add_liked_interest(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::AddThingILike {
            thing: thing.to_owned(),
        }));
    }

    pub fn remove_liked_interest(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::RemoveThingILike {
            thing: thing.to_owned(),
        }));
    }

    pub fn add_hated_interest(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::AddThingIHate {
            thing: thing.to_owned(),
        }));
    }

    pub fn remove_hated_interest(&self, thing: &str) {
        self.send(ClientCommand::Server(ServerRequest::RemoveThingIHate {
            thing: thing.to_owned(),
        }));
    }

    pub fn request_recommendations(&self) {
        self.send(ClientCommand::Server(ServerRequest::Recommendations));
    }

    pub fn join_room(&self, room: &str) {
        self.send(ClientCommand::Server(ServerRequest::JoinRoom {
            room: room.to_owned(),
            private: false,
        }));
    }

    pub fn leave_room(&self, room: &str) {
        self.send(ClientCommand::Server(ServerRequest::LeaveRoom {
            room: room.to_owned(),
        }));
    }

    pub fn say_room(&self, room: &str, message: &str) {
        self.send(ClientCommand::Server(ServerRequest::SayChatroom {
            room: room.to_owned(),
            message: message.to_owned(),
        }));
    }

    pub fn send_private_message(&self, username: &str, message: &str) {
        self.send(ClientCommand::Server(ServerRequest::MessageUser {
            user: username.to_owned(),
            message: message.to_owned(),
        }));
    }

    pub fn request_room_list(&self) {
        self.send(ClientCommand::Server(ServerRequest::RoomList));
    }

    pub fn server_request(&self, request: ServerRequest) {
        self.send(ClientCommand::Server(request));
    }

    pub fn connect(&self) {
        self.send(ClientCommand::Connect);
    }

    pub fn disconnect(&self) {
        self.send(ClientCommand::Disconnect);
    }
}
