use crate::types::{FileInfo, FolderContents, Recommendations, SimilarUser, UserStats, UserStatus};

use super::observation::Observation;

#[derive(Debug)]
pub struct SearchResult {
    pub token: u32,
    pub username: String,
    pub results: Vec<FileInfo>,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
}

#[derive(Debug)]
pub struct UserInfoReceived {
    pub username: String,
    pub description: String,
    pub picture: Option<Vec<u8>>,
    pub total_uploads: u32,
    pub queue_size: u32,
    pub slots_available: bool,
}

#[derive(Debug)]
pub enum ClientEvent {
    Connecting,
    LoggedIn {
        username: String,
        banner: String,
    },
    LoginFailed {
        reason: String,
        detail: Option<String>,
    },
    Disconnected,
    ConnectionCount(usize),
    ShareScanStarted,
    ShareScanProgress {
        files: u64,
    },
    SharesScanned {
        folders: u32,
        files: u32,
    },
    ShareScanFailed {
        error: String,
    },
    SearchStarted {
        token: u32,
        query: String,
    },
    SearchResults(SearchResult),
    UserStatus {
        username: String,
        status: Option<UserStatus>,
        privileged: bool,
    },
    WatchedUser {
        username: String,
        exists: bool,
        status: Option<UserStatus>,
        stats: Option<UserStats>,
    },
    UserStats {
        username: String,
        stats: UserStats,
    },
    UserInterests {
        username: String,
        liked: Vec<String>,
        hated: Vec<String>,
    },
    SharedFileList {
        username: String,
        shares: Vec<FolderContents>,
        private_shares: Vec<FolderContents>,
    },
    UserInfo(UserInfoReceived),
    PrivateMessage {
        username: String,
        message: String,
        timestamp: u32,
    },
    RoomMessage {
        room: String,
        username: String,
        message: String,
    },
    RoomList {
        rooms: Vec<(String, u32)>,
    },
    RoomJoined {
        room: String,
        users: Vec<String>,
    },
    RoomLeft {
        room: String,
    },
    RoomUserJoined {
        room: String,
        username: String,
    },
    RoomUserLeft {
        room: String,
        username: String,
    },
    Recommendations {
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    GlobalRecommendations {
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    ItemRecommendations {
        thing: String,
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    SimilarUsers(Vec<SimilarUser>),
    ItemSimilarUsers {
        thing: String,
        usernames: Vec<String>,
    },
    Privileges {
        seconds: u32,
    },
    AdminMessage {
        message: String,
    },
    Observed(Observation),
}
