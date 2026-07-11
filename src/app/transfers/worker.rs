use std::sync::Arc;

use tokio::sync::mpsc;

use crate::types::{TransferDirection, TransferEvent, TransferId};

use super::events::{apply, remove};
use crate::app::state::App;

pub enum TransferWork {
    Event(TransferEvent),
    Removed {
        direction: TransferDirection,
        ids: Vec<TransferId>,
    },
}

pub async fn submit(app: &App, work: TransferWork) {
    app.transfer_work
        .send(work)
        .await
        .expect("transfer worker gone");
}

pub async fn run_worker(app: Arc<App>, mut work: mpsc::Receiver<TransferWork>) {
    while let Some(item) = work.recv().await {
        match item {
            TransferWork::Event(event) => apply(&app, event).await,
            TransferWork::Removed { direction, ids } => remove(&app, direction, ids).await,
        }
    }
    tracing::error!("transfer worker terminated, exiting");
    std::process::exit(1);
}
