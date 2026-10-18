use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::client::{AbortResult, EnqueueResult, FOLDER_DOWNLOAD_FILE_LIMIT, RetryResult};
use crate::types::{FileAttributes, TransferDirection, TransferId, TransferStatus, USERNAME_LIMIT};

use crate::app::api;
use crate::app::state::App;
use crate::app::users;

pub(in crate::app) fn router() -> Router<Arc<App>> {
    let mut router = Router::new()
        .route("/api/downloads", post(enqueue_download))
        .route("/api/downloads/folder", post(enqueue_folder_download))
        .route(
            "/api/downloads/folder/request",
            post(request_folder_download),
        )
        .route("/api/downloads/retry", post(retry_download));
    for (direction, path) in [
        (TransferDirection::Download, "/api/downloads"),
        (TransferDirection::Upload, "/api/uploads"),
    ] {
        router = router
            .route(path, get(move |app| list(app, direction)))
            .route(
                &format!("{path}/abort"),
                post(move |app, body| abort(app, body, direction)),
            )
            .route(
                &format!("{path}/clear"),
                post(move |app, body| clear(app, body, direction)),
            )
            .route(
                &format!("{path}/clear_all"),
                post(move |app| clear_all(app, direction)),
            );
    }
    router
}

async fn list(
    State(app): State<Arc<App>>,
    direction: TransferDirection,
) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "transfers": data.transfers.list(direction) }))
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
    if body.username.len() > USERNAME_LIMIT {
        return StatusCode::BAD_REQUEST;
    }
    match app
        .client
        .download(
            &body.username,
            &body.virtual_path,
            body.size,
            body.attributes,
            None,
        )
        .await
    {
        EnqueueResult::Enqueued => StatusCode::ACCEPTED,
        EnqueueResult::AlreadyActive => StatusCode::CONFLICT,
        EnqueueResult::PathTooLong => StatusCode::BAD_REQUEST,
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
    let Some(browse) = users::browse(&app, &body.username) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let subdir_prefix = format!("{}\\", body.dir);
    let files: Vec<(String, u64, FileAttributes)> = {
        browse
            .all()
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
            .download(
                &body.username,
                &virtual_path,
                size,
                attributes,
                Some(&body.dir),
            )
            .await
        {
            EnqueueResult::Enqueued => enqueued += 1,
            EnqueueResult::AlreadyActive => already_active += 1,
            EnqueueResult::PathTooLong => {}
        }
    }
    Json(json!({ "enqueued": enqueued, "already_active": already_active })).into_response()
}

#[derive(Deserialize)]
struct FolderRequestBody {
    username: String,
    dir: String,
}

async fn request_folder_download(
    State(app): State<Arc<App>>,
    Json(body): Json<FolderRequestBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    app.client.request_folder(&body.username, &body.dir).await;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct TransferRef {
    id: TransferId,
}

async fn retry_download(State(app): State<Arc<App>>, Json(body): Json<TransferRef>) -> StatusCode {
    match app.client.retry_download(body.id).await {
        RetryResult::Requeued => StatusCode::ACCEPTED,
        RetryResult::AlreadyActive => StatusCode::CONFLICT,
        RetryResult::NotFound => StatusCode::NOT_FOUND,
    }
}

async fn abort(
    State(app): State<Arc<App>>,
    Json(body): Json<TransferRef>,
    direction: TransferDirection,
) -> StatusCode {
    match app.client.abort_transfer(direction, body.id).await {
        AbortResult::Aborted => StatusCode::ACCEPTED,
        AbortResult::NotFound => StatusCode::NOT_FOUND,
    }
}

#[derive(Deserialize)]
struct ClearBody {
    statuses: Vec<TransferStatus>,
}

async fn clear(
    State(app): State<Arc<App>>,
    Json(body): Json<ClearBody>,
    direction: TransferDirection,
) -> StatusCode {
    if body.statuses.is_empty() || body.statuses.iter().any(|status| !status.is_terminal()) {
        return StatusCode::BAD_REQUEST;
    }
    app.client
        .clear_transfers(direction, Some(body.statuses))
        .await;
    StatusCode::ACCEPTED
}

async fn clear_all(State(app): State<Arc<App>>, direction: TransferDirection) -> StatusCode {
    app.client.clear_transfers(direction, None).await;
    StatusCode::ACCEPTED
}
