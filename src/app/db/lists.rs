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

pub async fn add_to_list(pool: &MySqlPool, list: &str, username: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT IGNORE INTO user_lists (list, username) VALUES (?, ?)")
        .bind(list)
        .bind(username)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_from_list(
    pool: &MySqlPool,
    list: &str,
    username: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_lists WHERE list = ? AND username = ?")
        .bind(list)
        .bind(username)
        .execute(pool)
        .await?;
    Ok(())
}
