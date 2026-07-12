use super::transfer::{TransferDirection, TransferId, TransferSnapshot};

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Enqueued,
    AlreadyActive,
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryResult {
    Requeued,
    AlreadyActive,
    NotFound,
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortResult {
    Aborted,
    NotFound,
}

#[derive(Debug)]
pub enum TransferWork {
    Progress(TransferSnapshot),
    Update(TransferSnapshot),
    Finished {
        snapshot: TransferSnapshot,
        avg_speed_bps: Option<u32>,
    },
    Removed {
        direction: TransferDirection,
        ids: Vec<TransferId>,
    },
}
