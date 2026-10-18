mod db;
mod http;

pub(super) use http::router;

use std::collections::HashMap;
use std::sync::Arc;

use soulseek::client::TransferWork;
use soulseek::types::{TransferDirection, TransferId, TransferSnapshot, TransferStatus};
use sqlx::MySqlPool;
use tokio::sync::mpsc;

use super::behavior;
use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

#[derive(Clone, serde::Serialize)]
pub struct TransferView {
    #[serde(flatten)]
    pub snapshot: TransferSnapshot,
    pub updated_at: i64,
}

impl TransferView {
    fn new(snapshot: TransferSnapshot) -> Self {
        Self {
            snapshot,
            updated_at: now(),
        }
    }
}

#[derive(Default)]
pub struct Transfers {
    transfers: HashMap<TransferId, TransferView>,
}

impl Transfers {
    pub fn load(views: Vec<TransferView>) -> Self {
        Self {
            transfers: views
                .into_iter()
                .map(|view| (view.snapshot.id, view))
                .collect(),
        }
    }

    pub fn list(&self, direction: TransferDirection) -> Vec<&TransferView> {
        let mut list: Vec<&TransferView> = self
            .transfers
            .values()
            .filter(|view| view.snapshot.direction == direction)
            .collect();
        list.sort_by_key(|view| std::cmp::Reverse(view.updated_at));
        list
    }
}

pub async fn bootstrap(pool: &MySqlPool) -> Vec<TransferView> {
    let mut views = db::load_transfers(pool).await;
    for view in &mut views {
        if !view.snapshot.settle_after_restart() {
            continue;
        }
        view.updated_at = now();
        db::upsert_transfer(pool, view)
            .await
            .unwrap_or_else(|error| fatal(error));
    }
    views
}

fn project(app: &App, view: TransferView) {
    let mut data = app.projection.write();
    let event = AppEvent::Transfer {
        direction: view.snapshot.direction,
        transfer: view.clone(),
    };
    data.transfers.transfers.insert(view.snapshot.id, view);
    data.broadcast(event);
}

pub async fn run_worker(app: Arc<App>, mut work: mpsc::Receiver<TransferWork>) {
    while let Some(item) = work.recv().await {
        match item {
            TransferWork::Progress(snapshot) => {
                project(&app, TransferView::new(snapshot));
            }
            TransferWork::Update(snapshot) => {
                let view = TransferView::new(snapshot);
                db::upsert_transfer(&app.db, &view)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                project(&app, view);
            }
            TransferWork::Finished {
                snapshot,
                avg_speed_bps,
                delivered_bytes,
            } => {
                let view = TransferView::new(snapshot);
                let direction = view.snapshot.direction;
                let mut tx = app.db.begin().await.unwrap_or_else(|error| fatal(error));
                if delivered_bytes > 0 {
                    db::record_transfer(&mut *tx, &view, delivered_bytes, avg_speed_bps)
                        .await
                        .unwrap_or_else(|error| fatal(error));
                }
                db::upsert_transfer(&mut *tx, &view)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                tx.commit().await.unwrap_or_else(|error| fatal(error));
                if delivered_bytes > 0 && direction == TransferDirection::Upload {
                    let snapshot = &view.snapshot;
                    behavior::upload_delivered(&app, &snapshot.username, &snapshot.virtual_path)
                        .await;
                }
                project(&app, view);
                if app.settings.autoclear(direction) {
                    app.client
                        .clear_transfers(direction, Some(vec![TransferStatus::Finished]))
                        .await;
                }
            }
            TransferWork::Removed { direction, ids } => {
                db::delete_transfers(&app.db, &ids)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                let mut data = app.projection.write();
                for id in &ids {
                    data.transfers.transfers.remove(id);
                }
                data.broadcast(AppEvent::TransfersRemoved { direction, ids });
            }
        }
    }
    tracing::error!("transfer stream ended, client actor is gone");
}
