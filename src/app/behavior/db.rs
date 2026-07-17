pub async fn has_downloaded_from(pool: &sqlx::MySqlPool, username: &str) -> bool {
    use sqlx::Row;
    let count: i64 = sqlx::query(
        "SELECT COUNT(*) FROM transfer_history WHERE direction = 'download' AND username = ?",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("downloaded-from check")
    .get(0);
    count > 0
}
