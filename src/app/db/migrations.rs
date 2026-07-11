use sqlx::{MySqlPool, Row};
use tracing::info;

const BASELINE: &[&str] = &[
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

pub async fn init_schema(pool: &MySqlPool) {
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_version (version INT NOT NULL PRIMARY KEY)")
        .execute(pool)
        .await
        .expect("create schema_version");
    let applied: Option<i32> = sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(pool)
        .await
        .expect("read schema version")
        .get(0);
    let applied = applied.unwrap_or(0);

    if applied < 1 {
        for statement in BASELINE {
            migration_statement(pool, 1, statement).await;
        }
        record_migration(pool, 1).await;
    }
    if applied < 2 {
        migrate_transfer_status(pool).await;
        record_migration(pool, 2).await;
    }
    if applied < 3 {
        migrate_transfer_identity(pool).await;
        record_migration(pool, 3).await;
    }
}

async fn migrate_transfer_identity(pool: &MySqlPool) {
    migration_statement(
        pool,
        3,
        "ALTER TABLE transfers MODIFY id BIGINT UNSIGNED NOT NULL",
    )
    .await;
    migration_statement(pool, 3, "ALTER TABLE transfers DROP INDEX uniq_transfer").await;
    migration_statement(pool, 3, "ALTER TABLE transfers DROP COLUMN path_hash").await;
}

async fn migration_statement(pool: &MySqlPool, version: i32, statement: &str) {
    sqlx::query(statement)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("schema migration {version} failed: {error}"));
}

async fn record_migration(pool: &MySqlPool, version: i32) {
    sqlx::query("INSERT INTO schema_version (version) VALUES (?)")
        .bind(version)
        .execute(pool)
        .await
        .expect("record schema version");
    info!(version, "applied schema migration");
}

async fn migrate_transfer_status(pool: &MySqlPool) {
    migration_statement(
        pool,
        2,
        "ALTER TABLE user_lists MODIFY list ENUM('buddy','banned','ignored','chat','room','ip_ban') NOT NULL",
    )
    .await;
    let column_exists: i64 = sqlx::query(
        "SELECT COUNT(*) FROM information_schema.columns
         WHERE table_schema = DATABASE() AND table_name = 'transfers'
           AND column_name = 'failure_reason'",
    )
    .fetch_one(pool)
    .await
    .expect("schema introspection")
    .get(0);
    if column_exists == 0 {
        migration_statement(
            pool,
            2,
            "ALTER TABLE transfers ADD COLUMN failure_reason TEXT NULL AFTER status",
        )
        .await;
    }
    migration_statement(
        pool,
        2,
        "UPDATE transfers SET failure_reason = SUBSTRING(status, 9), status = 'failed'
         WHERE status LIKE 'failed: %'",
    )
    .await;
    migration_statement(
        pool,
        2,
        "ALTER TABLE transfers MODIFY status VARCHAR(32) NOT NULL",
    )
    .await;
}
