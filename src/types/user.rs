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
