mod db;
mod events;
mod http;
mod worker;

pub use http::{
    abort_download, abort_upload, clear_all_downloads, clear_all_uploads, clear_downloads,
    clear_uploads, enqueue_folder_download, http_enqueue_download, http_retry_download,
    list_downloads, list_uploads,
};
pub use worker::{TransferWork, run_worker, submit};

use std::collections::HashMap;

use serde::Serialize;
use sqlx::MySqlPool;

use crate::types::{
    AbortResult, EnqueueResult, FileAttributes, TransferDirection, TransferId, TransferSeed,
    TransferStatus,
};

use super::contract::AppEvent;
use super::db::fatal;
use super::state::{App, now};

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
    pub attributes: FileAttributes,
    pub updated_at: i64,
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

    pub fn get(&self, id: TransferId) -> Option<&TransferView> {
        self.transfers.get(&id)
    }

    fn remove(&mut self, ids: &[TransferId]) {
        for id in ids {
            self.transfers.remove(id);
        }
    }
}

impl From<&TransferView> for TransferSeed {
    fn from(view: &TransferView) -> Self {
        Self {
            id: view.id,
            direction: view.direction,
            username: view.username.clone(),
            virtual_path: view.virtual_path.clone(),
            size: view.size,
            status: view.status,
            failure_reason: view.failure_reason.clone(),
            attributes: view.attributes.clone(),
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

#[derive(Debug)]
pub enum TransferError {
    NotFound,
}

fn project(app: &App, view: TransferView) {
    let mut data = app.data.write().unwrap();
    let event = AppEvent::Transfer {
        direction: view.direction,
        transfer: view.clone(),
    };
    data.transfers.transfers.insert(view.id, view);
    app.broadcast(&mut data, event);
}

async fn commit(app: &App, view: TransferView) -> Result<(), sqlx::Error> {
    db::upsert_transfer(&app.db, &view).await?;
    project(app, view);
    Ok(())
}

pub async fn enqueue_download(
    app: &App,
    username: &str,
    virtual_path: &str,
    size: u64,
    attributes: FileAttributes,
) {
    match app
        .client
        .download(username, virtual_path, size, attributes)
        .await
    {
        EnqueueResult::Enqueued | EnqueueResult::AlreadyActive => {}
    }
}

pub async fn retry_download(app: &App, id: TransferId) -> Result<(), TransferError> {
    let view = {
        let data = app.data.read().unwrap();
        match data.transfers.get(id) {
            Some(view) if view.direction == TransferDirection::Download => view.clone(),
            _ => return Err(TransferError::NotFound),
        }
    };
    enqueue_download(app, &view.username, &view.virtual_path, view.size, view.attributes).await;
    Ok(())
}

pub async fn abort(
    app: &App,
    direction: TransferDirection,
    id: TransferId,
) -> Result<(), TransferError> {
    match app.client.abort_transfer(direction, id).await {
        AbortResult::Aborted => Ok(()),
        AbortResult::NotFound => Err(TransferError::NotFound),
    }
}
