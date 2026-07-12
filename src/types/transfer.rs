use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Upload => "upload",
        }
    }
}

impl std::str::FromStr for TransferDirection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "download" => Self::Download,
            "upload" => Self::Upload,
            other => return Err(format!("unknown transfer direction {other:?}")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Queued,
    Transferring,
    Finished,
    Aborted,
    Failed,
}

impl TransferStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Transferring => "transferring",
            Self::Finished => "finished",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub fn is_cleared(self) -> bool {
        matches!(self, Self::Finished | Self::Aborted | Self::Failed)
    }
}

impl std::str::FromStr for TransferStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "queued" => Self::Queued,
            "transferring" => Self::Transferring,
            "finished" => Self::Finished,
            "aborted" => Self::Aborted,
            "failed" => Self::Failed,
            other => return Err(format!("unknown transfer status {other:?}")),
        })
    }
}

#[derive(Clone, Serialize)]
pub struct TransferView {
    pub id: TransferId,
    #[serde(skip)]
    pub direction: TransferDirection,
    pub username: String,
    pub virtual_path: String,
    pub size: u64,
    pub bytes_done: u64,
    pub status: TransferStatus,
    pub failure_reason: Option<String>,
    pub file_path: Option<String>,
    pub queue_place: u32,
    pub speed_bps: u32,
    pub attributes: crate::types::FileAttributes,
    pub updated_at: i64,
}

impl TransferView {
    pub fn from_snapshot(snapshot: TransferSnapshot, updated_at: i64) -> Self {
        Self {
            id: snapshot.id,
            direction: snapshot.direction,
            username: snapshot.username,
            virtual_path: snapshot.virtual_path,
            size: snapshot.size,
            bytes_done: snapshot.bytes_done,
            status: snapshot.status,
            failure_reason: snapshot.failure_reason,
            file_path: snapshot.file_path,
            queue_place: snapshot.queue_place,
            speed_bps: snapshot.speed_bps,
            attributes: snapshot.attributes,
            updated_at,
        }
    }
}

impl From<&TransferView> for TransferSnapshot {
    fn from(view: &TransferView) -> Self {
        Self {
            id: view.id,
            direction: view.direction,
            username: view.username.clone(),
            virtual_path: view.virtual_path.clone(),
            size: view.size,
            bytes_done: view.bytes_done,
            status: view.status,
            failure_reason: view.failure_reason.clone(),
            file_path: view.file_path.clone(),
            queue_place: view.queue_place,
            speed_bps: view.speed_bps,
            attributes: view.attributes.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub id: TransferId,
    pub direction: TransferDirection,
    pub username: String,
    pub virtual_path: String,
    pub size: u64,
    pub bytes_done: u64,
    pub status: TransferStatus,
    pub failure_reason: Option<String>,
    pub file_path: Option<String>,
    pub queue_place: u32,
    pub speed_bps: u32,
    pub attributes: crate::types::FileAttributes,
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
