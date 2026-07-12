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

pub async fn set_user_verdict(
    pool: &sqlx::MySqlPool,
    username: &str,
    verdict: &str,
    evidence: &str,
    restriction: &str,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users_seen (username, first_seen, last_seen, verdict, evidence, restriction)
         VALUES (?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = VALUES(last_seen),
            verdict = VALUES(verdict),
            evidence = VALUES(evidence),
            restriction = VALUES(restriction)",
    )
    .bind(username)
    .bind(timestamp)
    .bind(timestamp)
    .bind(verdict)
    .bind(evidence)
    .bind(restriction)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_verdicts(pool: &sqlx::MySqlPool) -> Vec<(String, String, String)> {
    use sqlx::Row;
    sqlx::query(
        "SELECT username, verdict, COALESCE(evidence, '') FROM users_seen WHERE verdict != 'clean'",
    )
    .fetch_all(pool)
    .await
    .expect("load verdicts")
    .into_iter()
    .map(|row| (row.get(0), row.get(1), row.get(2)))
    .collect()
}
