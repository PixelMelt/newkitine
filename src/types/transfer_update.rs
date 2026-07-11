use std::path::PathBuf;

use crate::types::{FileAttributes, TransferDirection, TransferId};

#[derive(Debug)]
pub struct TransferEvent {
    pub id: TransferId,
    pub direction: TransferDirection,
    pub update: TransferUpdate,
}

#[derive(Debug)]
pub enum TransferUpdate {
    Queued {
        username: String,
        virtual_path: String,
        size: u64,
        attributes: FileAttributes,
    },
    Started {
        file_path: Option<PathBuf>,
    },
    Progress {
        bytes_done: u64,
        size: u64,
        speed_bps: u32,
    },
    QueuePlace {
        place: u32,
    },
    Finished {
        file_path: Option<PathBuf>,
        size: u64,
        speed_bps: Option<u32>,
    },
    Failed {
        reason: String,
    },
    Aborted,
}
