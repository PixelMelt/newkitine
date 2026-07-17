use sqlx::MySqlPool;
use sqlx::Row;

use super::stats::PeerActivity;

pub(super) async fn flush_activity(
    pool: &MySqlPool,
    username: &str,
    activity: &PeerActivity,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users_seen
            (username, first_seen, last_seen, searches, searches_matched, queue_requests,
             queue_rejected, browses, info_requests, folder_requests, connections, last_ip, country)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = GREATEST(last_seen, VALUES(last_seen)),
            searches = searches + VALUES(searches),
            searches_matched = searches_matched + VALUES(searches_matched),
            queue_requests = queue_requests + VALUES(queue_requests),
            queue_rejected = queue_rejected + VALUES(queue_rejected),
            browses = browses + VALUES(browses),
            info_requests = info_requests + VALUES(info_requests),
            folder_requests = folder_requests + VALUES(folder_requests),
            connections = connections + VALUES(connections),
            last_ip = COALESCE(VALUES(last_ip), last_ip),
            country = COALESCE(VALUES(country), country)",
    )
    .bind(username)
    .bind(timestamp)
    .bind(timestamp)
    .bind(activity.searches)
    .bind(activity.searches_matched)
    .bind(activity.queue_requests)
    .bind(activity.queue_rejected)
    .bind(activity.browses)
    .bind(activity.info_requests)
    .bind(activity.folder_requests)
    .bind(activity.connections)
    .bind(&activity.last_ip)
    .bind(&activity.country)
    .execute(pool)
    .await?;
    Ok(())
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
            last_seen = GREATEST(last_seen, VALUES(last_seen)),
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
            last_seen = GREATEST(last_seen, VALUES(last_seen)),
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
