use std::collections::HashSet;

use sqlx::{MySqlPool, Row};

use super::policy::{
    PeerCounters, REPEAT_DOWNLOAD_LIMIT, REPEAT_WINDOW_DAYS, SEARCH_FLOOR, SECS_PER_DAY,
    is_search_scraper,
};

pub async fn has_downloaded_from(pool: &MySqlPool, username: &str) -> bool {
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

pub async fn downloaded_from_any(pool: &MySqlPool, usernames: &[String]) -> HashSet<String> {
    if usernames.is_empty() {
        return HashSet::new();
    }
    let placeholders = vec!["?"; usernames.len()].join(", ");
    let statement = format!(
        "SELECT DISTINCT username FROM transfer_history
         WHERE direction = 'download' AND username IN ({placeholders})"
    );
    let mut query = sqlx::query(&statement);
    for username in usernames {
        query = query.bind(username);
    }
    query
        .fetch_all(pool)
        .await
        .expect("downloaded-from batch")
        .into_iter()
        .map(|row| row.get("username"))
        .collect()
}

pub async fn search_scrape_users(pool: &MySqlPool) -> Vec<(String, String)> {
    sqlx::query(
        "SELECT username, searches, queue_requests, browses,
                CAST(last_seen - GREATEST(first_seen, COALESCE(counters_reset_at, 0)) AS SIGNED) window_secs
         FROM users_seen
         WHERE verdict = 'clean' AND searches >= ?",
    )
    .bind(SEARCH_FLOOR)
    .fetch_all(pool)
    .await
    .expect("search scrape sweep")
    .into_iter()
    .filter_map(|row| {
        let counters = PeerCounters {
            searches: row.get("searches"),
            queue_requests: row.get("queue_requests"),
            browses: row.get("browses"),
            window_secs: row.get("window_secs"),
        };
        if !is_search_scraper(&counters) {
            return None;
        }
        let searches = counters.searches;
        let days = counters.window_secs / SECS_PER_DAY;
        Some((
            row.get("username"),
            format!("search-scrape:{searches}in{days}d:no-transfers"),
        ))
    })
    .collect()
}

const REPEAT_SCOPE: &str = "FROM transfer_history h
     LEFT JOIN users_seen u ON u.username = h.username
     WHERE h.direction = 'upload'
       AND h.finished_at > GREATEST(?, COALESCE(u.counters_reset_at, 0))";

pub struct RepeatDelivery {
    pub username: String,
    pub virtual_path: String,
    pub last_at: i64,
}

fn window_start(now: i64) -> i64 {
    now - REPEAT_WINDOW_DAYS * SECS_PER_DAY
}

pub async fn repeat_deliveries(pool: &MySqlPool, now: i64) -> Vec<RepeatDelivery> {
    sqlx::query(&format!(
        "SELECT h.username, h.virtual_path, MAX(h.finished_at) {REPEAT_SCOPE}
         GROUP BY h.username, h.virtual_path
         HAVING SUM(h.bytes) >= ? * MAX(h.size)"
    ))
    .bind(window_start(now))
    .bind(REPEAT_DOWNLOAD_LIMIT)
    .fetch_all(pool)
    .await
    .expect("repeat deliveries")
    .into_iter()
    .map(|row| RepeatDelivery {
        username: row.get(0),
        virtual_path: row.get(1),
        last_at: row.get(2),
    })
    .collect()
}

pub async fn repeat_delivery(
    pool: &MySqlPool,
    username: &str,
    virtual_path: &str,
    now: i64,
) -> Option<i64> {
    sqlx::query(&format!(
        "SELECT MAX(h.finished_at) {REPEAT_SCOPE}
           AND h.username = ? AND h.virtual_path = ?
         HAVING SUM(h.bytes) >= ? * MAX(h.size)"
    ))
    .bind(window_start(now))
    .bind(username)
    .bind(virtual_path)
    .bind(REPEAT_DOWNLOAD_LIMIT)
    .fetch_optional(pool)
    .await
    .expect("repeat delivery check")
    .map(|row| row.get(0))
}

pub async fn reset_counters(
    pool: &MySqlPool,
    username: &str,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users_seen SET counters_reset_at = ? WHERE username = ?")
        .bind(timestamp)
        .bind(username)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_user_verdict(
    pool: &MySqlPool,
    username: &str,
    verdict: &str,
    evidence: &str,
    restriction: &str,
    timestamp: i64,
    convicted_at: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users_seen
            (username, first_seen, last_seen, verdict, evidence, restriction, convicted_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = GREATEST(last_seen, VALUES(last_seen)),
            verdict = VALUES(verdict),
            evidence = VALUES(evidence),
            restriction = VALUES(restriction),
            convicted_at = COALESCE(convicted_at, VALUES(convicted_at))",
    )
    .bind(username)
    .bind(timestamp)
    .bind(timestamp)
    .bind(verdict)
    .bind(evidence)
    .bind(restriction)
    .bind(convicted_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_user_verdict(
    pool: &MySqlPool,
    username: &str,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users_seen
         SET verdict = 'clean', restriction = 'none', searches = 0, searches_matched = 0,
             convicted_at = NULL, counters_reset_at = ?
         WHERE username = ?",
    )
    .bind(timestamp)
    .bind(username)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_verdicts(pool: &MySqlPool) -> Vec<(String, String, String)> {
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
