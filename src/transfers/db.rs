use soulseek::types::{FileAttributes, TransferId, TransferSnapshot};
use sqlx::MySqlPool;
use sqlx::Row;

use super::TransferView;

const DELETE_BATCH_SIZE: usize = 1000;

fn column(value: impl serde::Serialize) -> String {
    match serde_json::to_value(value).unwrap() {
        serde_json::Value::String(name) => name,
        other => unreachable!("transfer column {other} is not a name"),
    }
}

fn parse_column<T: serde::de::DeserializeOwned>(id: TransferId, value: String) -> T {
    serde_json::from_value(serde_json::Value::String(value))
        .unwrap_or_else(|error| panic!("corrupt transfer row {}: {error}", id.0))
}

pub(super) async fn upsert_transfer(
    executor: impl sqlx::MySqlExecutor<'_>,
    view: &TransferView,
) -> Result<(), sqlx::Error> {
    let snapshot = &view.snapshot;
    let attributes = serde_json::to_string(&snapshot.attributes).unwrap();
    sqlx::query(
        "INSERT INTO transfers
            (id, direction, username, virtual_path, folder_path, size, bytes_done, status, failure_reason, file_path, attributes, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            folder_path = VALUES(folder_path), size = VALUES(size), bytes_done = VALUES(bytes_done), status = VALUES(status),
            failure_reason = VALUES(failure_reason), file_path = VALUES(file_path),
            attributes = VALUES(attributes), updated_at = VALUES(updated_at)",
    )
    .bind(snapshot.id.0)
    .bind(column(snapshot.direction))
    .bind(&snapshot.username)
    .bind(&snapshot.virtual_path)
    .bind(&snapshot.destination)
    .bind(snapshot.size)
    .bind(snapshot.bytes_done)
    .bind(column(snapshot.status))
    .bind(&snapshot.failure_reason)
    .bind(&snapshot.file_path)
    .bind(attributes)
    .bind(view.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn load_transfers(pool: &MySqlPool) -> Vec<TransferView> {
    sqlx::query(
        "SELECT id, direction, username, virtual_path, size, bytes_done, status, failure_reason, file_path, attributes, updated_at, folder_path
         FROM transfers ORDER BY updated_at",
    )
    .fetch_all(pool)
    .await
    .expect("load transfers")
    .into_iter()
    .map(|row| {
        let id = TransferId(row.get(0));
        let username: String = row.get(2);
        let virtual_path: String = row.get(3);
        let direction = parse_column(id, row.get(1));
        let status = parse_column(id, row.get(6));
        let attributes = serde_json::from_str::<FileAttributes>(&row.get::<String, _>(9))
            .unwrap_or_else(|error| {
                panic!("corrupt attributes for transfer {} {username} {virtual_path}: {error}", id.0)
            });
        TransferView {
            snapshot: TransferSnapshot {
                id,
                direction,
                username,
                virtual_path,
                destination: row.get(11),
                size: row.get(4),
                bytes_done: row.get(5),
                status,
                failure_reason: row.get(7),
                file_path: row.get(8),
                queue_place: 0,
                speed_bps: 0,
                attributes,
            },
            updated_at: row.get(10),
        }
    })
    .collect()
}

pub(super) async fn delete_transfers(
    pool: &MySqlPool,
    ids: &[TransferId],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for chunk in ids.chunks(DELETE_BATCH_SIZE) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let statement = format!("DELETE FROM transfers WHERE id IN ({placeholders})");
        let mut query = sqlx::query(&statement);
        for id in chunk {
            query = query.bind(id.0);
        }
        query.execute(&mut *tx).await?;
    }
    tx.commit().await
}

pub(super) async fn record_transfer(
    executor: impl sqlx::MySqlExecutor<'_>,
    view: &TransferView,
    bytes: u64,
    speed_bps: Option<u32>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO transfer_history
            (direction, username, virtual_path, size, bytes, speed_bps, finished_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(column(view.snapshot.direction))
    .bind(&view.snapshot.username)
    .bind(&view.snapshot.virtual_path)
    .bind(view.snapshot.size)
    .bind(bytes)
    .bind(speed_bps)
    .bind(view.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}
