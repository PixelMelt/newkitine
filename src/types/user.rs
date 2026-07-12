#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UserStatus {
    Offline,
    Away,
    Online,
}

impl UserStatus {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Offline),
            1 => Some(Self::Away),
            2 => Some(Self::Online),
            _ => None,
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            Self::Offline => 0,
            Self::Away => 1,
            Self::Online => 2,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct UserStats {
    pub avgspeed: u32,
    pub uploadnum: u32,
    pub unknown: u32,
    pub files: u32,
    pub dirs: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct UserData {
    pub username: String,
    pub status: u32,
    pub stats: UserStats,
    pub slotsfull: u32,
    pub country: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SimilarUser {
    pub username: String,
    pub rating: u32,
}

#[derive(Default, Clone, serde::Serialize)]
pub struct BuddyView {
    pub username: String,
    pub status: String,
    pub privileged: bool,
    pub stats: UserStats,
    pub note: String,
}

#[derive(Default, Clone, serde::Serialize)]
pub struct UserInfoView {
    pub username: String,
    pub received: bool,
    pub description: String,
    pub picture_base64: Option<String>,
    pub upload_slots: u32,
    pub queue_size: u32,
    pub slots_available: bool,
    pub stats: Option<UserStats>,
    pub interests_liked: Vec<String>,
    pub interests_hated: Vec<String>,
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

#[derive(serde::Serialize)]
pub struct BrowseView {
    pub username: String,
    pub folders: Vec<crate::types::FolderContents>,
    pub private_folders: Vec<crate::types::FolderContents>,
    pub received_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Restriction {
    None,
    Deprioritized,
    Hold,
    Denied { reason: String },
}
