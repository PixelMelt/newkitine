use sqlx::MySqlPool;
use sqlx::Row;

pub async fn load_settings(pool: &MySqlPool) -> Option<String> {
    sqlx::query("SELECT data FROM settings WHERE id = 1")
        .fetch_optional(pool)
        .await
        .expect("load settings")
        .map(|row| row.get(0))
}

pub async fn save_settings(pool: &MySqlPool, data: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (id, data) VALUES (1, ?)
         ON DUPLICATE KEY UPDATE data = VALUES(data)",
    )
    .bind(data)
    .execute(pool)
    .await?;
    Ok(())
}
