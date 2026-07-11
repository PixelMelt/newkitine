use sqlx::MySqlPool;
use sqlx::Row;

use crate::types::{FileAttributes, TransferDirection, TransferId, TransferStatus};

use super::TransferView;

pub(super) async fn upsert_transfer(
    pool: &MySqlPool,
    view: &TransferView,
) -> Result<(), sqlx::Error> {
    let attributes = serde_json::to_string(&view.attributes).unwrap();
    sqlx::query(
        "INSERT INTO transfers
            (id, direction, username, virtual_path, size, bytes_done, status, failure_reason, file_path, attributes, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            size = VALUES(size), bytes_done = VALUES(bytes_done), status = VALUES(status),
            failure_reason = VALUES(failure_reason), file_path = VALUES(file_path),
            attributes = VALUES(attributes), updated_at = VALUES(updated_at)",
    )
    .bind(view.id.0)
    .bind(view.direction.as_str())
    .bind(&view.username)
    .bind(&view.virtual_path)
    .bind(view.size)
    .bind(view.bytes_done)
    .bind(view.status.as_str())
    .bind(&view.failure_reason)
    .bind(&view.file_path)
    .bind(attributes)
    .bind(view.updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_transfers(pool: &MySqlPool) -> Vec<TransferView> {
    sqlx::query(
        "SELECT id, direction, username, virtual_path, size, bytes_done, status, failure_reason, file_path, attributes, updated_at
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
        let direction = row
            .get::<String, _>(1)
            .parse::<TransferDirection>()
            .unwrap_or_else(|error| panic!("corrupt transfer row {}: {error}", id.0));
        let status = row
            .get::<String, _>(6)
            .parse::<TransferStatus>()
            .unwrap_or_else(|error| panic!("corrupt transfer row {}: {error}", id.0));
        let attributes = serde_json::from_str::<FileAttributes>(&row.get::<String, _>(9))
            .unwrap_or_else(|error| {
                panic!("corrupt attributes for transfer {} {username} {virtual_path}: {error}", id.0)
            });
        TransferView {
            id,
            direction,
            username,
            virtual_path,
            size: row.get(4),
            bytes_done: row.get(5),
            status,
            failure_reason: row.get(7),
            file_path: row.get(8),
            queue_place: 0,
            speed_bps: 0,
            attributes,
            updated_at: row.get(10),
        }
    })
    .collect()
}

pub(super) async fn delete_transfers(
    pool: &MySqlPool,
    ids: &[TransferId],
) -> Result<(), sqlx::Error> {
    let placeholders = vec!["?"; ids.len()].join(", ");
    let statement = format!("DELETE FROM transfers WHERE id IN ({placeholders})");
    let mut query = sqlx::query(&statement);
    for id in ids {
        query = query.bind(id.0);
    }
    query.execute(pool).await?;
    Ok(())
}

pub(super) async fn record_transfer(
    pool: &MySqlPool,
    direction: TransferDirection,
    username: &str,
    virtual_path: &str,
    size: u64,
    speed_bps: Option<u32>,
    finished_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO transfer_history (direction, username, virtual_path, size, speed_bps, finished_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(direction.as_str())
    .bind(username)
    .bind(virtual_path)
    .bind(size)
    .bind(speed_bps)
    .bind(finished_at)
    .execute(pool)
    .await?;
    Ok(())
}
