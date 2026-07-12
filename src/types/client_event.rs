use crate::types::{
    FolderContents, Observation, Recommendations, SearchResult, SimilarUser, UserInfoReceived,
    UserStats, UserStatus,
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
