use sqlx::{MySqlPool, Row};

pub async fn load_list(pool: &MySqlPool, list: &str) -> Vec<String> {
    sqlx::query("SELECT username FROM user_lists WHERE list = ? ORDER BY username")
        .bind(list)
        .fetch_all(pool)
        .await
        .expect("load user list")
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

pub async fn add_to_list(
    executor: impl sqlx::MySqlExecutor<'_>,
    list: &str,
    username: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_lists (list, username) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE username = username",
    )
    .bind(list)
    .bind(username)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn remove_from_list(
    executor: impl sqlx::MySqlExecutor<'_>,
    list: &str,
    username: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_lists WHERE list = ? AND username = ?")
        .bind(list)
        .bind(username)
        .execute(executor)
        .await?;
    Ok(())
}
