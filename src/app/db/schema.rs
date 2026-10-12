use sqlx::MySqlPool;

const TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS transfers (
        id BIGINT UNSIGNED NOT NULL PRIMARY KEY,
        direction ENUM('download','upload') NOT NULL,
        username VARCHAR(190) NOT NULL,
        virtual_path TEXT NOT NULL,
        folder_path TEXT NULL,
        size BIGINT UNSIGNED NOT NULL,
        bytes_done BIGINT UNSIGNED NOT NULL DEFAULT 0,
        status VARCHAR(32) NOT NULL,
        failure_reason TEXT NULL,
        file_path TEXT NULL,
        attributes TEXT NOT NULL,
        updated_at BIGINT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS chat_messages (
        id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
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
        bytes BIGINT UNSIGNED NULL,
        speed_bps INT UNSIGNED NULL,
        finished_at BIGINT NOT NULL,
        KEY idx_history_time (direction, finished_at),
        KEY idx_history_user (username),
        KEY idx_history_path (username, virtual_path(180))
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
        convicted_at BIGINT NULL,
        counters_reset_at BIGINT NULL,
        KEY idx_seen_last (last_seen)
    )",
];

pub async fn init_schema(pool: &MySqlPool) {
    for statement in TABLES {
        sqlx::query(statement)
            .execute(pool)
            .await
            .expect("create table");
    }
}
