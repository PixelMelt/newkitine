use std::time::Duration;

use sqlx::mysql::MySqlPoolOptions;
use sqlx::{MySqlPool, Row};
use tracing::{info, warn};

use newkitine::types::FileAttributes;

use super::state::{ChatMessage, TransferView};

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS transfers (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        direction ENUM('download','upload') NOT NULL,
        username VARCHAR(190) NOT NULL,
        virtual_path TEXT NOT NULL,
        path_hash CHAR(32) NOT NULL,
        size BIGINT UNSIGNED NOT NULL,
        bytes_done BIGINT UNSIGNED NOT NULL DEFAULT 0,
        status VARCHAR(255) NOT NULL,
        file_path TEXT NULL,
        attributes TEXT NOT NULL,
        updated_at BIGINT NOT NULL,
        UNIQUE KEY uniq_transfer (direction, username, path_hash)
    )",
    "CREATE TABLE IF NOT EXISTS chat_messages (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        kind ENUM('private','room') NOT NULL,
        target VARCHAR(190) NOT NULL,
        sender VARCHAR(190) NOT NULL,
        message TEXT NOT NULL,
        timestamp BIGINT NOT NULL,
        KEY idx_target (kind, target, id)
    )",
    "CREATE TABLE IF NOT EXISTS user_lists (
        list ENUM('buddy','banned','ignored','chat','room','ip_ban') NOT NULL,
        username VARCHAR(190) NOT NULL,
        PRIMARY KEY (list, username)
    )",
    "ALTER TABLE user_lists MODIFY list ENUM('buddy','banned','ignored','chat','room','ip_ban') NOT NULL",
    "CREATE TABLE IF NOT EXISTS user_notes (
        username VARCHAR(190) NOT NULL PRIMARY KEY,
        note TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS wishlist (
        term VARCHAR(190) NOT NULL PRIMARY KEY
    )",
    "CREATE TABLE IF NOT EXISTS interests (
        kind ENUM('liked','hated') NOT NULL,
        thing VARCHAR(190) NOT NULL,
        PRIMARY KEY (kind, thing)
    )",
    "CREATE TABLE IF NOT EXISTS settings (
        id TINYINT NOT NULL PRIMARY KEY,
        data MEDIUMTEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS transfer_history (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        direction ENUM('download','upload') NOT NULL,
        username VARCHAR(190) NOT NULL,
        virtual_path TEXT NOT NULL,
        size BIGINT UNSIGNED NOT NULL,
        speed_bps INT UNSIGNED NULL,
        finished_at BIGINT NOT NULL,
        KEY idx_history_time (direction, finished_at),
        KEY idx_history_user (username)
    )",
    "CREATE TABLE IF NOT EXISTS users_seen (
        username VARCHAR(190) NOT NULL PRIMARY KEY,
        first_seen BIGINT NOT NULL,
        last_seen BIGINT NOT NULL,
        searches INT UNSIGNED NOT NULL DEFAULT 0,
        searches_matched INT UNSIGNED NOT NULL DEFAULT 0,
        queue_requests INT UNSIGNED NOT NULL DEFAULT 0,
        queue_rejected INT UNSIGNED NOT NULL DEFAULT 0,
        browses INT UNSIGNED NOT NULL DEFAULT 0,
        info_requests INT UNSIGNED NOT NULL DEFAULT 0,
        folder_requests INT UNSIGNED NOT NULL DEFAULT 0,
        connections INT UNSIGNED NOT NULL DEFAULT 0,
        last_ip VARCHAR(15) NULL,
        country CHAR(2) NULL,
        shared_files INT UNSIGNED NULL,
        shared_folders INT UNSIGNED NULL,
        stats_seen_at BIGINT NULL,
        verdict VARCHAR(32) NOT NULL DEFAULT 'clean',
        evidence TEXT NULL,
        restriction VARCHAR(32) NOT NULL DEFAULT 'none',
        KEY idx_seen_last (last_seen)
    )",
];

pub fn path_hash(virtual_path: &str) -> String {
    format!("{:x}", md5::compute(virtual_path.as_bytes()))
}

pub async fn connect(url: &str) -> MySqlPool {
    let mut attempts = 0u32;
    loop {
        match MySqlPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await
        {
            Ok(pool) => {
                info!("connected to database");
                return pool;
            }
            Err(error) => {
                attempts += 1;
                if attempts > 30 {
                    panic!("cannot connect to database at {url}: {error}");
                }
                warn!(%error, attempts, "database not ready, retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

pub async fn init_schema(pool: &MySqlPool) {
    for statement in SCHEMA {
        sqlx::query(statement)
            .execute(pool)
            .await
            .expect("schema statement failed");
    }
}

pub async fn load_settings(pool: &MySqlPool) -> Option<String> {
    sqlx::query("SELECT data FROM settings WHERE id = 1")
        .fetch_optional(pool)
        .await
        .expect("load settings")
        .map(|row| row.get(0))
}

pub async fn save_settings(pool: &MySqlPool, data: &str) {
    sqlx::query(
        "INSERT INTO settings (id, data) VALUES (1, ?)
         ON DUPLICATE KEY UPDATE data = VALUES(data)",
    )
    .bind(data)
    .execute(pool)
    .await
    .expect("save settings");
}

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

pub async fn add_to_list(pool: &MySqlPool, list: &str, username: &str) {
    let _ = sqlx::query("INSERT IGNORE INTO user_lists (list, username) VALUES (?, ?)")
        .bind(list)
        .bind(username)
        .execute(pool)
        .await;
}

pub async fn remove_from_list(pool: &MySqlPool, list: &str, username: &str) {
    let _ = sqlx::query("DELETE FROM user_lists WHERE list = ? AND username = ?")
        .bind(list)
        .bind(username)
        .execute(pool)
        .await;
}

pub async fn load_notes(pool: &MySqlPool) -> Vec<(String, String)> {
    sqlx::query("SELECT username, note FROM user_notes")
        .fetch_all(pool)
        .await
        .expect("load user notes")
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect()
}

pub async fn set_note(pool: &MySqlPool, username: &str, note: &str) {
    if note.is_empty() {
        let _ = sqlx::query("DELETE FROM user_notes WHERE username = ?")
            .bind(username)
            .execute(pool)
            .await;
        return;
    }
    let _ = sqlx::query(
        "INSERT INTO user_notes (username, note) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE note = VALUES(note)",
    )
    .bind(username)
    .bind(note)
    .execute(pool)
    .await;
}

pub async fn load_wishlist(pool: &MySqlPool) -> Vec<String> {
    sqlx::query("SELECT term FROM wishlist ORDER BY term")
        .fetch_all(pool)
        .await
        .expect("load wishlist")
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

pub async fn add_wish(pool: &MySqlPool, term: &str) {
    let _ = sqlx::query("INSERT IGNORE INTO wishlist (term) VALUES (?)")
        .bind(term)
        .execute(pool)
        .await;
}

pub async fn remove_wish(pool: &MySqlPool, term: &str) {
    let _ = sqlx::query("DELETE FROM wishlist WHERE term = ?")
        .bind(term)
        .execute(pool)
        .await;
}

pub async fn load_interests(pool: &MySqlPool, kind: &str) -> Vec<String> {
    sqlx::query("SELECT thing FROM interests WHERE kind = ? ORDER BY thing")
        .bind(kind)
        .fetch_all(pool)
        .await
        .expect("load interests")
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

pub async fn add_interest(pool: &MySqlPool, kind: &str, thing: &str) {
    let _ = sqlx::query("INSERT IGNORE INTO interests (kind, thing) VALUES (?, ?)")
        .bind(kind)
        .bind(thing)
        .execute(pool)
        .await;
}

pub async fn remove_interest(pool: &MySqlPool, kind: &str, thing: &str) {
    let _ = sqlx::query("DELETE FROM interests WHERE kind = ? AND thing = ?")
        .bind(kind)
        .bind(thing)
        .execute(pool)
        .await;
}

pub async fn upsert_transfer(pool: &MySqlPool, direction: &str, view: &TransferView) {
    let attributes = serde_json::to_string(&view.attributes).unwrap();
    let _ = sqlx::query(
        "INSERT INTO transfers
            (direction, username, virtual_path, path_hash, size, bytes_done, status, file_path, attributes, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            size = VALUES(size), bytes_done = VALUES(bytes_done), status = VALUES(status),
            file_path = VALUES(file_path), attributes = VALUES(attributes), updated_at = VALUES(updated_at)",
    )
    .bind(direction)
    .bind(&view.username)
    .bind(&view.virtual_path)
    .bind(path_hash(&view.virtual_path))
    .bind(view.size)
    .bind(view.bytes_done)
    .bind(&view.status)
    .bind(&view.file_path)
    .bind(attributes)
    .bind(view.updated_at)
    .execute(pool)
    .await;
}

pub async fn load_transfers(pool: &MySqlPool, direction: &str) -> Vec<TransferView> {
    sqlx::query(
        "SELECT username, virtual_path, size, bytes_done, status, file_path, attributes, updated_at
         FROM transfers WHERE direction = ? ORDER BY updated_at",
    )
    .bind(direction)
    .fetch_all(pool)
    .await
    .expect("load transfers")
    .into_iter()
    .map(|row| TransferView {
        username: row.get(0),
        virtual_path: row.get(1),
        size: row.get(2),
        bytes_done: row.get(3),
        status: row.get(4),
        file_path: row.get(5),
        queue_place: 0,
        attributes: serde_json::from_str::<FileAttributes>(&row.get::<String, _>(6))
            .unwrap_or_default(),
        updated_at: row.get(7),
    })
    .collect()
}

pub async fn insert_chat(pool: &MySqlPool, kind: &str, target: &str, message: &ChatMessage) {
    let _ = sqlx::query(
        "INSERT INTO chat_messages (kind, target, sender, message, timestamp) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(kind)
    .bind(target)
    .bind(&message.sender)
    .bind(&message.message)
    .bind(message.timestamp)
    .execute(pool)
    .await;
}

pub async fn load_chat(pool: &MySqlPool, kind: &str, target: &str, limit: u32) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = sqlx::query(
        "SELECT sender, message, timestamp FROM chat_messages
         WHERE kind = ? AND target = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(kind)
    .bind(target)
    .bind(limit)
    .fetch_all(pool)
    .await
    .expect("load chat")
    .into_iter()
    .map(|row| ChatMessage {
        sender: row.get(0),
        message: row.get(1),
        timestamp: row.get(2),
    })
    .collect();
    messages.reverse();
    messages
}

pub async fn clear_all_transfers(pool: &MySqlPool, direction: &str) {
    let _ = sqlx::query("DELETE FROM transfers WHERE direction = ?")
        .bind(direction)
        .execute(pool)
        .await;
}

pub async fn record_transfer(
    pool: &MySqlPool,
    direction: &str,
    username: &str,
    virtual_path: &str,
    size: u64,
    speed_bps: Option<u32>,
    finished_at: i64,
) {
    let _ = sqlx::query(
        "INSERT INTO transfer_history (direction, username, virtual_path, size, speed_bps, finished_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(direction)
    .bind(username)
    .bind(virtual_path)
    .bind(size)
    .bind(speed_bps)
    .bind(finished_at)
    .execute(pool)
    .await;
}

pub async fn flush_activity(
    pool: &MySqlPool,
    username: &str,
    activity: &super::stats::PeerActivity,
    timestamp: i64,
) {
    let _ = sqlx::query(
        "INSERT INTO users_seen
            (username, first_seen, last_seen, searches, searches_matched, queue_requests,
             queue_rejected, browses, info_requests, folder_requests, connections, last_ip, country)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = VALUES(last_seen),
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
    .await;
}

pub async fn record_user_shares(
    pool: &MySqlPool,
    username: &str,
    files: u32,
    folders: u32,
    timestamp: i64,
) {
    let _ = sqlx::query(
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
    .await;
}

pub async fn set_user_verdict(
    pool: &MySqlPool,
    username: &str,
    verdict: &str,
    evidence: &str,
    restriction: &str,
    timestamp: i64,
) {
    let _ = sqlx::query(
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
    .await;
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

pub async fn last_ip(pool: &MySqlPool, username: &str) -> Option<String> {
    sqlx::query("SELECT last_ip FROM users_seen WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .expect("last ip lookup")
        .and_then(|row| row.get(0))
}

pub async fn verdict_users(pool: &MySqlPool) -> serde_json::Value {
    let users: Vec<serde_json::Value> = sqlx::query(
        "SELECT username, verdict, COALESCE(evidence, ''), restriction, last_ip, country,
                shared_files, shared_folders, last_seen
         FROM users_seen WHERE verdict IN ('suspect', 'leech') ORDER BY last_seen DESC",
    )
    .fetch_all(pool)
    .await
    .expect("verdict users")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "username": row.get::<String, _>(0),
            "verdict": row.get::<String, _>(1),
            "evidence": row.get::<String, _>(2),
            "restriction": row.get::<String, _>(3),
            "last_ip": row.get::<Option<String>, _>(4),
            "country": row.get::<Option<String>, _>(5),
            "shared_files": row.get::<Option<u32>, _>(6),
            "shared_folders": row.get::<Option<u32>, _>(7),
            "last_seen": row.get::<i64, _>(8),
        })
    })
    .collect();
    serde_json::json!({ "users": users })
}

pub async fn transfer_stats(pool: &MySqlPool) -> serde_json::Value {
    let mut totals = serde_json::Map::new();
    for row in sqlx::query(
        "SELECT direction, COUNT(*), CAST(SUM(size) AS UNSIGNED),
                CAST(COALESCE(AVG(speed_bps), 0) AS UNSIGNED)
         FROM transfer_history GROUP BY direction",
    )
    .fetch_all(pool)
    .await
    .expect("transfer totals")
    {
        let direction: String = row.get(0);
        totals.insert(
            direction,
            serde_json::json!({
                "count": row.get::<i64, _>(1),
                "bytes": row.get::<u64, _>(2),
                "avg_speed_bps": row.get::<u64, _>(3),
            }),
        );
    }

    let unique_upload_users: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT username) FROM transfer_history WHERE direction = 'upload'",
    )
    .fetch_one(pool)
    .await
    .expect("unique upload users")
    .get(0);

    let top_files: Vec<serde_json::Value> = sqlx::query(
        "SELECT virtual_path, COUNT(*) AS c, CAST(SUM(size) AS UNSIGNED)
         FROM transfer_history WHERE direction = 'upload'
         GROUP BY virtual_path ORDER BY c DESC LIMIT 10",
    )
    .fetch_all(pool)
    .await
    .expect("top files")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "virtual_path": row.get::<String, _>(0),
            "count": row.get::<i64, _>(1),
            "bytes": row.get::<u64, _>(2),
        })
    })
    .collect();

    let top_users: Vec<serde_json::Value> = sqlx::query(
        "SELECT username, COUNT(*) AS c, CAST(SUM(size) AS UNSIGNED)
         FROM transfer_history WHERE direction = 'upload'
         GROUP BY username ORDER BY c DESC LIMIT 10",
    )
    .fetch_all(pool)
    .await
    .expect("top users")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "username": row.get::<String, _>(0),
            "count": row.get::<i64, _>(1),
            "bytes": row.get::<u64, _>(2),
        })
    })
    .collect();

    let mut weekday_uploads = [0i64; 7];
    for row in sqlx::query(
        "SELECT CAST(WEEKDAY(FROM_UNIXTIME(finished_at)) AS UNSIGNED) AS d, COUNT(*)
         FROM transfer_history WHERE direction = 'upload' GROUP BY d",
    )
    .fetch_all(pool)
    .await
    .expect("weekday uploads")
    {
        weekday_uploads[row.get::<u64, _>(0) as usize] = row.get(1);
    }

    serde_json::json!({
        "totals": totals,
        "unique_upload_users": unique_upload_users,
        "top_files": top_files,
        "top_users": top_users,
        "weekday_uploads": weekday_uploads,
    })
}

pub async fn peers_seen(pool: &MySqlPool, limit: u32) -> serde_json::Value {
    let total: i64 = sqlx::query("SELECT COUNT(*) FROM users_seen")
        .fetch_one(pool)
        .await
        .expect("peers total")
        .get(0);

    let countries: Vec<serde_json::Value> = sqlx::query(
        "SELECT country, COUNT(*) AS c FROM users_seen
         WHERE country IS NOT NULL GROUP BY country ORDER BY c DESC",
    )
    .fetch_all(pool)
    .await
    .expect("peer countries")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "country": row.get::<String, _>(0),
            "count": row.get::<i64, _>(1),
        })
    })
    .collect();

    let users: Vec<serde_json::Value> = sqlx::query(
        "SELECT username, first_seen, last_seen, searches, searches_matched, queue_requests,
                queue_rejected, browses, info_requests, folder_requests, connections, last_ip,
                country, shared_files, shared_folders, verdict, restriction
         FROM users_seen ORDER BY last_seen DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .expect("peers seen")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "username": row.get::<String, _>(0),
            "first_seen": row.get::<i64, _>(1),
            "last_seen": row.get::<i64, _>(2),
            "searches": row.get::<u32, _>(3),
            "searches_matched": row.get::<u32, _>(4),
            "queue_requests": row.get::<u32, _>(5),
            "queue_rejected": row.get::<u32, _>(6),
            "browses": row.get::<u32, _>(7),
            "info_requests": row.get::<u32, _>(8),
            "folder_requests": row.get::<u32, _>(9),
            "connections": row.get::<u32, _>(10),
            "last_ip": row.get::<Option<String>, _>(11),
            "country": row.get::<Option<String>, _>(12),
            "shared_files": row.get::<Option<u32>, _>(13),
            "shared_folders": row.get::<Option<u32>, _>(14),
            "verdict": row.get::<String, _>(15),
            "restriction": row.get::<String, _>(16),
        })
    })
    .collect();

    serde_json::json!({ "total": total, "countries": countries, "users": users })
}

pub async fn clear_transfers(pool: &MySqlPool, direction: &str, statuses: &[&str]) {
    for status in statuses {
        let query = if *status == "failed" {
            "DELETE FROM transfers WHERE direction = ? AND status LIKE 'failed%'"
        } else {
            "DELETE FROM transfers WHERE direction = ? AND status = ?"
        };
        let mut q = sqlx::query(query).bind(direction);
        if *status != "failed" {
            q = q.bind(status);
        }
        let _ = q.execute(pool).await;
    }
}
