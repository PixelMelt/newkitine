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
        list ENUM('buddy','banned','ignored','chat','room') NOT NULL,
        username VARCHAR(190) NOT NULL,
        PRIMARY KEY (list, username)
    )",
    "ALTER TABLE user_lists MODIFY list ENUM('buddy','banned','ignored','chat','room') NOT NULL",
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
];

pub fn path_hash(virtual_path: &str) -> String {
    format!("{:x}", md5::compute(virtual_path.as_bytes()))
}

pub async fn connect(url: &str) -> MySqlPool {
    let mut attempts = 0u32;
    loop {
        match MySqlPoolOptions::new().max_connections(5).connect(url).await {
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
        sqlx::query(statement).execute(pool).await.expect("schema statement failed");
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
    let _ = sqlx::query("DELETE FROM wishlist WHERE term = ?").bind(term).execute(pool).await;
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

pub async fn insert_chat(
    pool: &MySqlPool,
    kind: &str,
    target: &str,
    message: &ChatMessage,
) {
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

pub async fn load_chat(
    pool: &MySqlPool,
    kind: &str,
    target: &str,
    limit: u32,
) -> Vec<ChatMessage> {
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
