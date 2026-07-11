#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Download,
    Upload,
}

impl TransferDirection {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Download),
            1 => Some(Self::Upload),
            _ => None,
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            Self::Download => 0,
            Self::Upload => 1,
        }
    }
}

pub struct TransferRejectReason;

impl TransferRejectReason {
    pub const QUEUED: &'static str = "Queued";
    pub const COMPLETE: &'static str = "Complete";
    pub const CANCELLED: &'static str = "Cancelled";
    pub const FILE_READ_ERROR: &'static str = "File read error.";
    pub const FILE_NOT_SHARED: &'static str = "File not shared.";
    pub const BANNED: &'static str = "Banned";
    pub const PENDING_SHUTDOWN: &'static str = "Pending shutdown.";
    pub const TOO_MANY_FILES: &'static str = "Too many files";
    pub const TOO_MANY_MEGABYTES: &'static str = "Too many megabytes";
    pub const DISALLOWED_EXTENSION: &'static str = "Disallowed extension";
}
