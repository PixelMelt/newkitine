mod downloads;
mod files;
mod queue;
mod registry;
mod speed;
mod uploads;

pub(super) use downloads::Downloads;
pub(super) use uploads::Uploads;

use crate::types::{TransferDirection, TransferId, TransferSnapshot, TransferStatus};

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

pub(super) struct TransferIds {
    next: u64,
}

impl TransferIds {
    pub(super) fn new(seeds: &[TransferSnapshot]) -> Self {
        Self {
            next: seeds.iter().map(|seed| seed.id.0).max().unwrap_or(0) + 1,
        }
    }

    pub(super) fn mint(&mut self) -> TransferId {
        let id = TransferId(self.next);
        self.next += 1;
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TransferPhase {
    Queued,
    GettingStatus,
    Transferring,
    Placing,
    Finished,
    Aborted,
    Failed(String),
}

impl TransferPhase {
    pub(super) fn status(&self) -> TransferStatus {
        match self {
            Self::Queued | Self::GettingStatus => TransferStatus::Queued,
            Self::Transferring | Self::Placing => TransferStatus::Transferring,
            Self::Finished => TransferStatus::Finished,
            Self::Aborted => TransferStatus::Aborted,
            Self::Failed(_) => TransferStatus::Failed,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Queued | Self::GettingStatus | Self::Transferring | Self::Placing
        )
    }

    pub(super) fn from_seed(seed: &TransferSnapshot) -> Self {
        match seed.status {
            TransferStatus::Queued => Self::Queued,
            TransferStatus::Transferring => {
                unreachable!("bootstrap normalizes transferring transfers before seeding")
            }
            TransferStatus::Finished => Self::Finished,
            TransferStatus::Aborted => Self::Aborted,
            TransferStatus::Failed => Self::Failed(seed.failure_reason.clone().unwrap_or_default()),
        }
    }
}

pub(super) struct TransferRejectReason;

impl TransferRejectReason {
    pub(super) const QUEUED: &'static str = "Queued";
    pub(super) const COMPLETE: &'static str = "Complete";
    pub(super) const CANCELLED: &'static str = "Cancelled";
    pub(super) const FILE_NOT_SHARED: &'static str = "File not shared.";
    pub(super) const TOO_MANY_FILES: &'static str = "Too many files";
    pub(super) const TOO_MANY_MEGABYTES: &'static str = "Too many megabytes";
}
