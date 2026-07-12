mod db;
pub mod http;

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::MySqlPool;
use tokio::sync::mpsc;

use crate::types::{TransferDirection, TransferId, TransferStatus, TransferView, TransferWork};

use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

#[derive(Default)]
pub struct Transfers {
    transfers: HashMap<TransferId, TransferView>,
}

impl Transfers {
    pub fn load(views: Vec<TransferView>) -> Self {
        Self {
            transfers: views.into_iter().map(|view| (view.id, view)).collect(),
        }
    }

    pub fn list(&self, direction: TransferDirection) -> Vec<&TransferView> {
        let mut list: Vec<&TransferView> = self
            .transfers
            .values()
            .filter(|view| view.direction == direction)
            .collect();
        list.sort_by_key(|view| std::cmp::Reverse(view.updated_at));
        list
    }

    fn upsert(&mut self, view: TransferView) {
        self.transfers.insert(view.id, view);
    }

    fn remove(&mut self, ids: &[TransferId]) {
        for id in ids {
            self.transfers.remove(id);
        }
    }
}

pub async fn bootstrap(pool: &MySqlPool) -> Vec<TransferView> {
    let mut views = db::load_transfers(pool).await;
    for view in &mut views {
        let normalized = match (view.direction, view.status) {
            (TransferDirection::Download, TransferStatus::Transferring) => {
                view.status = TransferStatus::Queued;
                view.failure_reason = None;
                true
            }
            (TransferDirection::Upload, TransferStatus::Queued | TransferStatus::Transferring) => {
                view.status = TransferStatus::Failed;
                view.failure_reason = Some("interrupted by restart".into());
                true
            }
            _ => false,
        };
        if normalized {
            view.speed_bps = 0;
            view.updated_at = now();
            db::upsert_transfer(pool, view)
                .await
                .unwrap_or_else(|error| fatal(error));
        }
    }
    views
}

fn project(app: &App, view: TransferView) {
    let mut data = app.projection.write();
    let event = AppEvent::Transfer {
        direction: view.direction,
        transfer: view.clone(),
    };
    data.transfers.upsert(view);
    data.broadcast(event);
}

pub async fn run_worker(app: Arc<App>, mut work: mpsc::Receiver<TransferWork>) {
    while let Some(item) = work.recv().await {
        match item {
            TransferWork::Progress(snapshot) => {
                project(&app, TransferView::from_snapshot(snapshot, now()));
            }
            TransferWork::Update(snapshot) => {
                let view = TransferView::from_snapshot(snapshot, now());
                db::upsert_transfer(&app.db, &view)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                project(&app, view);
            }
            TransferWork::Finished {
                snapshot,
                avg_speed_bps,
            } => {
                let view = TransferView::from_snapshot(snapshot, now());
                db::record_transfer(
                    &app.db,
                    view.direction,
                    &view.username,
                    &view.virtual_path,
                    view.size,
                    avg_speed_bps,
                    view.updated_at,
                )
                .await
                .unwrap_or_else(|error| fatal(error));
                db::upsert_transfer(&app.db, &view)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                project(&app, view);
            }
            TransferWork::Removed { direction, ids } => {
                db::delete_transfers(&app.db, &ids)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                let mut data = app.projection.write();
                data.transfers.remove(&ids);
                data.broadcast(AppEvent::TransfersRemoved { direction, ids });
            }
        }
    }
    tracing::error!("transfer stream ended, client actor is gone");
}
