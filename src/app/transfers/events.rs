use crate::types::{TransferDirection, TransferEvent, TransferId, TransferStatus, TransferUpdate};

use super::{TransferView, commit, db, project};
use crate::app::contract::AppEvent;
use crate::app::db::fatal;
use crate::app::state::{App, now};

fn view_of(app: &App, id: TransferId) -> TransferView {
    let data = app.data.read().unwrap();
    match data.transfers.get(id) {
        Some(view) => view.clone(),
        None => {
            tracing::error!(
                id = id.0,
                "transfer update for a transfer the projection does not know, exiting"
            );
            std::process::exit(1);
        }
    }
}

pub async fn apply(app: &App, event: TransferEvent) {
    let TransferEvent {
        id,
        direction,
        update,
    } = event;
    match update {
        TransferUpdate::Queued {
            username,
            virtual_path,
            size,
            attributes,
        } => {
            let view = {
                let data = app.data.read().unwrap();
                data.transfers.get(id).cloned()
            };
            let view = match view {
                Some(mut view) => {
                    view.status = TransferStatus::Queued;
                    view.failure_reason = None;
                    view.size = size;
                    view.speed_bps = 0;
                    view.attributes = attributes;
                    view.updated_at = now();
                    view
                }
                None => TransferView {
                    id,
                    direction,
                    username,
                    virtual_path,
                    size,
                    bytes_done: 0,
                    status: TransferStatus::Queued,
                    failure_reason: None,
                    file_path: None,
                    queue_place: 0,
                    speed_bps: 0,
                    attributes,
                    updated_at: now(),
                },
            };
            commit(app, view).await.unwrap_or_else(|error| fatal(error));
        }
        TransferUpdate::Started { file_path } => {
            let mut view = view_of(app, id);
            view.status = TransferStatus::Transferring;
            view.queue_place = 0;
            view.speed_bps = 0;
            if let Some(path) = file_path {
                view.file_path = Some(path.display().to_string());
            }
            view.updated_at = now();
            commit(app, view).await.unwrap_or_else(|error| fatal(error));
        }
        TransferUpdate::Progress {
            bytes_done,
            size,
            speed_bps,
        } => {
            let mut view = view_of(app, id);
            view.status = TransferStatus::Transferring;
            view.bytes_done = bytes_done;
            view.size = size;
            view.speed_bps = speed_bps;
            view.updated_at = now();
            project(app, view);
        }
        TransferUpdate::QueuePlace { place } => {
            let mut view = view_of(app, id);
            view.queue_place = place;
            project(app, view);
        }
        TransferUpdate::Finished {
            file_path,
            size,
            speed_bps,
        } => {
            let mut view = view_of(app, id);
            view.status = TransferStatus::Finished;
            view.failure_reason = None;
            view.size = size;
            view.bytes_done = size;
            view.speed_bps = 0;
            if let Some(path) = file_path {
                view.file_path = Some(path.display().to_string());
            }
            view.updated_at = now();
            db::record_transfer(
                &app.db,
                direction,
                &view.username,
                &view.virtual_path,
                size,
                speed_bps,
                view.updated_at,
            )
            .await
            .unwrap_or_else(|error| fatal(error));
            commit(app, view).await.unwrap_or_else(|error| fatal(error));
        }
        TransferUpdate::Failed { reason } => {
            let mut view = view_of(app, id);
            view.status = TransferStatus::Failed;
            view.failure_reason = Some(reason);
            view.speed_bps = 0;
            view.updated_at = now();
            commit(app, view).await.unwrap_or_else(|error| fatal(error));
        }
        TransferUpdate::Aborted => {
            let mut view = view_of(app, id);
            view.status = TransferStatus::Aborted;
            view.failure_reason = None;
            view.speed_bps = 0;
            view.updated_at = now();
            commit(app, view).await.unwrap_or_else(|error| fatal(error));
        }
    }
}

pub async fn remove(app: &App, direction: TransferDirection, ids: Vec<TransferId>) {
    db::delete_transfers(&app.db, &ids)
        .await
        .unwrap_or_else(|error| fatal(error));
    let mut data = app.data.write().unwrap();
    data.transfers.remove(&ids);
    app.broadcast(&mut data, AppEvent::TransfersRemoved { direction, ids });
}
