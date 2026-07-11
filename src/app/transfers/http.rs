use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::types::{FileAttributes, TransferDirection, TransferId, TransferStatus};

use super::{TransferError, abort, enqueue_download, retry_download};
use crate::app::state::App;

fn transfer_status(result: Result<(), TransferError>) -> StatusCode {
    match result {
        Ok(()) => StatusCode::ACCEPTED,
        Err(TransferError::NotFound) => StatusCode::NOT_FOUND,
    }
}

pub async fn list_downloads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "transfers": data.transfers.list(TransferDirection::Download) }))
}

pub async fn list_uploads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "transfers": data.transfers.list(TransferDirection::Upload) }))
}

#[derive(Deserialize)]
pub struct DownloadBody {
    username: String,
    virtual_path: String,
    size: u64,
    #[serde(default)]
    attributes: FileAttributes,
}

pub async fn http_enqueue_download(
    State(app): State<Arc<App>>,
    Json(body): Json<DownloadBody>,
) -> StatusCode {
    enqueue_download(
        &app,
        &body.username,
        &body.virtual_path,
        body.size,
        body.attributes,
    )
    .await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct FolderDownloadBody {
    username: String,
    dir: String,
    #[serde(default)]
    recursive: bool,
}

pub async fn enqueue_folder_download(
    State(app): State<Arc<App>>,
    Json(body): Json<FolderDownloadBody>,
) -> impl IntoResponse {
    let files: Vec<(String, u64, FileAttributes)> = {
        let data = app.data.read().unwrap();
        let Some(browse) = data.users.browse(&body.username) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let subdir_prefix = format!("{}\\", body.dir);
        browse
            .folders
            .iter()
            .chain(browse.private_folders.iter())
            .filter(|folder| {
                folder.directory == body.dir
                    || (body.recursive && folder.directory.starts_with(&subdir_prefix))
            })
            .flat_map(|folder| {
                folder.files.iter().map(|file| {
                    (
                        format!("{}\\{}", folder.directory, file.name),
                        file.size,
                        file.attributes.clone(),
                    )
                })
            })
            .collect()
    };
    let count = files.len();
    for (virtual_path, size, attributes) in files {
        enqueue_download(&app, &body.username, &virtual_path, size, attributes).await;
    }
    Json(json!({ "enqueued": count })).into_response()
}

#[derive(Deserialize)]
pub struct TransferRef {
    id: TransferId,
}

pub async fn abort_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    transfer_status(abort(&app, TransferDirection::Download, body.id).await)
}

pub async fn http_retry_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    transfer_status(retry_download(&app, body.id).await)
}

#[derive(Deserialize)]
pub struct ClearBody {
    statuses: Vec<TransferStatus>,
}

fn clear_statuses(body: ClearBody) -> Option<Vec<TransferStatus>> {
    if body.statuses.is_empty() || body.statuses.iter().any(|status| !status.is_cleared()) {
        return None;
    }
    Some(body.statuses)
}

pub async fn clear_downloads(
    State(app): State<Arc<App>>,
    Json(body): Json<ClearBody>,
) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else {
        return StatusCode::BAD_REQUEST;
    };
    app.client
        .clear_transfers(TransferDirection::Download, statuses)
        .await;
    StatusCode::ACCEPTED
}

pub async fn abort_upload(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    transfer_status(abort(&app, TransferDirection::Upload, body.id).await)
}

pub async fn clear_uploads(State(app): State<Arc<App>>, Json(body): Json<ClearBody>) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else {
        return StatusCode::BAD_REQUEST;
    };
    app.client
        .clear_transfers(TransferDirection::Upload, statuses)
        .await;
    StatusCode::ACCEPTED
}

pub async fn clear_all_downloads(State(app): State<Arc<App>>) -> StatusCode {
    app.client
        .clear_all_transfers(TransferDirection::Download)
        .await;
    StatusCode::ACCEPTED
}

pub async fn clear_all_uploads(State(app): State<Arc<App>>) -> StatusCode {
    app.client
        .clear_all_transfers(TransferDirection::Upload)
        .await;
    StatusCode::ACCEPTED
}
