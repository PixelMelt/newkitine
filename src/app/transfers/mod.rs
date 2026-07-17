mod db;
mod http;

pub(super) use http::router;

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::MySqlPool;
use tokio::sync::mpsc;

use crate::client::TransferWork;
use crate::types::{
    FileAttributes, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

#[derive(Clone, serde::Serialize)]
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
    pub attributes: FileAttributes,
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
                let direction = view.direction;
                let mut tx = app.db.begin().await.unwrap_or_else(|error| fatal(error));
                db::record_transfer(
                    &mut *tx,
                    view.direction,
                    &view.username,
                    &view.virtual_path,
                    view.size,
                    avg_speed_bps,
                    view.updated_at,
                )
                .await
                .unwrap_or_else(|error| fatal(error));
                db::upsert_transfer(&mut *tx, &view)
                    .await
                    .unwrap_or_else(|error| fatal(error));
                tx.commit().await.unwrap_or_else(|error| fatal(error));
                project(&app, view);
                if app.settings.autoclear(direction) {
                    app.client
                        .clear_transfers(direction, vec![TransferStatus::Finished])
                        .await;
                }
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
