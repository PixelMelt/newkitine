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

pub async fn repeat_download_users(pool: &MySqlPool, now: i64) -> Vec<(String, String)> {
    sqlx::query(
        "SELECT r.username, MAX(r.copies)
         FROM (SELECT username, COUNT(*) copies FROM transfer_history
               WHERE direction = 'upload' AND finished_at > ?
               GROUP BY username, virtual_path HAVING COUNT(*) > ?) r
         JOIN users_seen u ON u.username = r.username AND u.verdict = 'clean'
         GROUP BY r.username",
    )
    .bind(now - REPEAT_WINDOW_DAYS * SECS_PER_DAY)
    .bind(REPEAT_DOWNLOAD_LIMIT)
    .fetch_all(pool)
    .await
    .expect("repeat download sweep")
    .into_iter()
    .map(|row| {
        let copies: i64 = row.get(1);
        (row.get(0), format!("repeat-downloads:{copies}"))
    })
    .collect()
}
