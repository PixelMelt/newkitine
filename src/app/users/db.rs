use sqlx::MySqlPool;
use sqlx::Row;

pub async fn load_notes(pool: &MySqlPool) -> Vec<(String, String)> {
    sqlx::query("SELECT username, note FROM user_notes")
        .fetch_all(pool)
        .await
        .expect("load user notes")
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect()
}

pub(super) async fn set_note(
    pool: &MySqlPool,
    username: &str,
    note: &str,
) -> Result<(), sqlx::Error> {
    if note.is_empty() {
        sqlx::query("DELETE FROM user_notes WHERE username = ?")
            .bind(username)
            .execute(pool)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO user_notes (username, note) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE note = VALUES(note)",
    )
    .bind(username)
    .bind(note)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_user_shares(
    pool: &MySqlPool,
    username: &str,
    files: u32,
    folders: u32,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users_seen (username, first_seen, last_seen, shared_files, shared_folders, stats_seen_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = VALUES(last_seen),
            shared_files = VALUES(shared_files),
            shared_folders = VALUES(shared_folders),
            stats_seen_at = VALUES(stats_seen_at)",
    )
    .bind(username)
    .bind(timestamp)
    .bind(timestamp)
    .bind(files)
    .bind(folders)
    .bind(timestamp)
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn last_ip(pool: &MySqlPool, username: &str) -> Option<String> {
    sqlx::query("SELECT last_ip FROM users_seen WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .expect("last ip lookup")
        .and_then(|row| row.get(0))
}
