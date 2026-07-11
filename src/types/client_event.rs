use crate::types::{
    FolderContents, Observation, Recommendations, SearchResult, SimilarUser, TransferDirection,
    TransferEvent, TransferId, UserStats, UserStatus,
};

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
    Disconnected,
    ConnectionCount(usize),
    SharesScanned {
        folders: u32,
        files: u32,
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
    Transfer(TransferEvent),
    TransfersRemoved {
        direction: TransferDirection,
        ids: Vec<TransferId>,
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
