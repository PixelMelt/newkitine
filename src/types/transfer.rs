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
    pub fn is_cleared(self) -> bool {
        matches!(self, Self::Finished | Self::Aborted | Self::Failed)
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
