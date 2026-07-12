use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::types::{
    AbortResult, EnqueueResult, FileAttributes, RetryResult, TransferDirection, TransferId,
    TransferStatus,
};

use crate::app::state::App;

pub async fn list_downloads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "transfers": data.transfers.list(TransferDirection::Download) }))
}

pub async fn list_uploads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
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

pub async fn enqueue_download(
    State(app): State<Arc<App>>,
    Json(body): Json<DownloadBody>,
) -> StatusCode {
    match app
        .client
        .download(
            &body.username,
            &body.virtual_path,
            body.size,
            body.attributes,
        )
        .await
    {
        EnqueueResult::Enqueued => StatusCode::ACCEPTED,
        EnqueueResult::AlreadyActive => StatusCode::CONFLICT,
    }
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
        let data = app.projection.read();
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
    let (mut enqueued, mut already_active) = (0usize, 0usize);
    for (virtual_path, size, attributes) in files {
        match app
            .client
            .download(&body.username, &virtual_path, size, attributes)
            .await
        {
            EnqueueResult::Enqueued => enqueued += 1,
            EnqueueResult::AlreadyActive => already_active += 1,
        }
    }
    Json(json!({ "enqueued": enqueued, "already_active": already_active })).into_response()
}

#[derive(Deserialize)]
pub struct TransferRef {
    id: TransferId,
}

pub async fn abort_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    abort(&app, TransferDirection::Download, body.id).await
}

pub async fn retry_download(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
) -> StatusCode {
    match app.client.retry_download(body.id).await {
        RetryResult::Requeued => StatusCode::ACCEPTED,
        RetryResult::AlreadyActive => StatusCode::CONFLICT,
        RetryResult::NotFound => StatusCode::NOT_FOUND,
    }
}

async fn abort(app: &App, direction: TransferDirection, id: TransferId) -> StatusCode {
    match app.client.abort_transfer(direction, id).await {
        AbortResult::Aborted => StatusCode::ACCEPTED,
        AbortResult::NotFound => StatusCode::NOT_FOUND,
    }
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
    abort(&app, TransferDirection::Upload, body.id).await
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
