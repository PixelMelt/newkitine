use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::client::{AbortResult, EnqueueResult, RetryResult};
use crate::types::{FileAttributes, TransferDirection, TransferId, TransferStatus};

use crate::app::state::App;

const FOLDER_DOWNLOAD_FILE_LIMIT: usize = 1000;

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(enqueue_download))
        .route("/api/downloads/folder", post(enqueue_folder_download))
        .route("/api/downloads/abort", post(abort_download))
        .route("/api/downloads/retry", post(retry_download))
        .route("/api/downloads/clear", post(clear_downloads))
        .route("/api/downloads/clear_all", post(clear_all_downloads))
        .route("/api/uploads", get(list_uploads))
        .route("/api/uploads/abort", post(abort_upload))
        .route("/api/uploads/clear", post(clear_uploads))
        .route("/api/uploads/clear_all", post(clear_all_uploads))
}

async fn list_downloads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "transfers": data.transfers.list(TransferDirection::Download) }))
}

async fn list_uploads(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "transfers": data.transfers.list(TransferDirection::Upload) }))
}

#[derive(Deserialize)]
struct DownloadBody {
    username: String,
    virtual_path: String,
    size: u64,
    #[serde(default)]
    attributes: FileAttributes,
}

async fn enqueue_download(
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
struct FolderDownloadBody {
    username: String,
    dir: String,
    #[serde(default)]
    recursive: bool,
}

async fn enqueue_folder_download(
    State(app): State<Arc<App>>,
    Json(body): Json<FolderDownloadBody>,
) -> impl IntoResponse {
    let (folders, private_folders) = {
        let data = app.projection.read();
        let Some(browse) = data.users.browse(&body.username) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        (browse.folders.clone(), browse.private_folders.clone())
    };
    let subdir_prefix = format!("{}\\", body.dir);
    let files: Vec<(String, u64, FileAttributes)> = {
        folders
            .iter()
            .chain(private_folders.iter())
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
            .take(FOLDER_DOWNLOAD_FILE_LIMIT + 1)
            .collect()
    };
    if files.len() > FOLDER_DOWNLOAD_FILE_LIMIT {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "folder contains more than {FOLDER_DOWNLOAD_FILE_LIMIT} files",
                ),
            })),
        )
            .into_response();
    }
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
struct TransferRef {
    id: TransferId,
}

async fn abort_download(State(app): State<Arc<App>>, Json(body): Json<TransferRef>) -> StatusCode {
    abort(&app, TransferDirection::Download, body.id).await
}

async fn retry_download(State(app): State<Arc<App>>, Json(body): Json<TransferRef>) -> StatusCode {
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
struct ClearBody {
    statuses: Vec<TransferStatus>,
}

fn clear_statuses(body: ClearBody) -> Option<Vec<TransferStatus>> {
    if body.statuses.is_empty() || body.statuses.iter().any(|status| !status.is_cleared()) {
        return None;
    }
    Some(body.statuses)
}

async fn clear_downloads(State(app): State<Arc<App>>, Json(body): Json<ClearBody>) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else {
        return StatusCode::BAD_REQUEST;
    };
    app.client
        .clear_transfers(TransferDirection::Download, statuses)
        .await;
    StatusCode::ACCEPTED
}

async fn abort_upload(State(app): State<Arc<App>>, Json(body): Json<TransferRef>) -> StatusCode {
    abort(&app, TransferDirection::Upload, body.id).await
}

async fn clear_uploads(State(app): State<Arc<App>>, Json(body): Json<ClearBody>) -> StatusCode {
    let Some(statuses) = clear_statuses(body) else {
        return StatusCode::BAD_REQUEST;
    };
    app.client
        .clear_transfers(TransferDirection::Upload, statuses)
        .await;
    StatusCode::ACCEPTED
}

async fn clear_all_downloads(State(app): State<Arc<App>>) -> StatusCode {
    app.client
        .clear_all_transfers(TransferDirection::Download)
        .await;
    StatusCode::ACCEPTED
}

async fn clear_all_uploads(State(app): State<Arc<App>>) -> StatusCode {
    app.client
        .clear_all_transfers(TransferDirection::Upload)
        .await;
    StatusCode::ACCEPTED
}
