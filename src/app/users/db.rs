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
    executor: impl sqlx::MySqlExecutor<'_>,
    username: &str,
    note: &str,
) -> Result<(), sqlx::Error> {
    if note.is_empty() {
        sqlx::query("DELETE FROM user_notes WHERE username = ?")
            .bind(username)
            .execute(executor)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO user_notes (username, note) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE note = VALUES(note)",
    )
    .bind(username)
    .bind(note)
    .execute(executor)
    .await?;
    Ok(())
}
