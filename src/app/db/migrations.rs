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
        convicted_at BIGINT NULL,
        counters_reset_at BIGINT NULL,
        KEY idx_seen_last (last_seen)
    )",
];

const LATEST_VERSION: i32 = 10;

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
    if applied > LATEST_VERSION {
        panic!("database schema version {applied} is newer than this binary supports");
    }

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
    if applied < 4 {
        migration_statement(
            pool,
            4,
            "ALTER TABLE chat_messages MODIFY id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT",
        )
        .await;
        record_migration(pool, 4).await;
    }
    if applied < 5 {
        migration_statement(
            pool,
            5,
            "UPDATE users_seen SET verdict = 'clean', evidence = NULL, restriction = 'none'
             WHERE verdict = 'suspect' AND evidence = 'search-flood'",
        )
        .await;
        migration_statement(
            pool,
            5,
            "UPDATE users_seen SET verdict = 'abusive' WHERE verdict = 'suspect'",
        )
        .await;
        record_migration(pool, 5).await;
    }
    if applied < 6 {
        migration_statement(
            pool,
            6,
            "UPDATE settings SET data = JSON_SET(
                JSON_REMOVE(data, '$.denied_message'),
                '$.abusive_message', JSON_UNQUOTE(JSON_EXTRACT(data, '$.denied_message')),
                '$.leech_message', JSON_UNQUOTE(JSON_EXTRACT(data, '$.denied_message')))
             WHERE id = 1 AND JSON_EXTRACT(data, '$.denied_message') IS NOT NULL",
        )
        .await;
        migration_statement(
            pool,
            6,
            "UPDATE settings SET data = JSON_SET(data, '$.description',
                REPLACE(REPLACE(JSON_UNQUOTE(JSON_EXTRACT(data, '$.description')),
                    '\\\\r\\\\n', CHAR(10)), '\\\\n', CHAR(10)))
             WHERE id = 1 AND JSON_EXTRACT(data, '$.description') IS NOT NULL",
        )
        .await;
        record_migration(pool, 6).await;
    }
    if applied < 7 {
        migration_statement(
            pool,
            7,
            "UPDATE users_seen SET verdict = CASE
                WHEN evidence LIKE '%search-flood%' OR evidence LIKE '%repeat-downloads%'
                    THEN 'abusive'
                WHEN evidence LIKE '%zero-share%' OR evidence LIKE '%preset-stats%'
                    OR evidence LIKE '%browse-contradicts-stats%' THEN 'leech'
                ELSE 'clean' END
             WHERE FIND_IN_SET('queue-flood', evidence)",
        )
        .await;
        migration_statement(
            pool,
            7,
            "UPDATE users_seen SET evidence = NULLIF(
                TRIM(BOTH ',' FROM REPLACE(CONCAT(',', evidence, ','), ',queue-flood,', ',')), '')
             WHERE FIND_IN_SET('queue-flood', evidence)",
        )
        .await;
        migration_statement(
            pool,
            7,
            "UPDATE users_seen SET restriction = 'none'
             WHERE verdict = 'clean' AND restriction <> 'none'",
        )
        .await;
        record_migration(pool, 7).await;
    }
    if applied < 8 {
        migration_statement(
            pool,
            8,
            "UPDATE users_seen SET verdict = 'clean', evidence = NULL, restriction = 'none'
             WHERE evidence LIKE 'search-flood:0.0.0.0:%'",
        )
        .await;
        migration_statement(
            pool,
            8,
            "UPDATE users_seen SET last_ip = NULL WHERE last_ip = '0.0.0.0'",
        )
        .await;
        record_migration(pool, 8).await;
    }
    if applied < 9 {
        migration_statement(
            pool,
            9,
            "UPDATE settings SET data = JSON_SET(data, '$.description',
                REPLACE(JSON_UNQUOTE(JSON_EXTRACT(data, '$.description')), '$', '$$'))
             WHERE id = 1 AND JSON_EXTRACT(data, '$.description') IS NOT NULL",
        )
        .await;
        record_migration(pool, 9).await;
    }
    if applied < 10 {
        release_filter_convictions(pool).await;
        record_migration(pool, 10).await;
    }
}

async fn release_filter_convictions(pool: &MySqlPool) {
    if !column_exists(pool, "users_seen", "convicted_at").await {
        migration_statement(
            pool,
            10,
            "ALTER TABLE users_seen
                ADD COLUMN convicted_at BIGINT NULL,
                ADD COLUMN counters_reset_at BIGINT NULL",
        )
        .await;
    }
    migration_statement(
        pool,
        10,
        "DELETE h FROM transfer_history h
         JOIN (SELECT MIN(id) keep_id, username, virtual_path FROM transfer_history
               WHERE direction = 'upload'
               GROUP BY username, virtual_path HAVING COUNT(*) > 1) d
           ON d.username = h.username AND d.virtual_path = h.virtual_path
         JOIN users_seen u ON u.username = h.username
         WHERE h.direction = 'upload' AND h.id > d.keep_id
           AND (u.evidence LIKE 'search-flood%'
                OR u.evidence LIKE 'repeat-downloads%'
                OR u.evidence LIKE 'preset-stats%')",
    )
    .await;
    migration_statement(
        pool,
        10,
        "UPDATE users_seen u
         SET u.verdict = 'clean', u.restriction = 'none', u.searches = 0,
             u.searches_matched = 0, u.counters_reset_at = UNIX_TIMESTAMP()
         WHERE u.verdict <> 'clean'
           AND (u.evidence LIKE 'search-flood%'
                OR u.evidence LIKE 'repeat-downloads%'
                OR u.evidence LIKE 'preset-stats%')",
    )
    .await;
}

async fn migrate_transfer_identity(pool: &MySqlPool) {
    migration_statement(
        pool,
        3,
        "ALTER TABLE transfers MODIFY id BIGINT UNSIGNED NOT NULL",
    )
    .await;
    if index_exists(pool, "transfers", "uniq_transfer").await {
        migration_statement(pool, 3, "ALTER TABLE transfers DROP INDEX uniq_transfer").await;
    }
    if column_exists(pool, "transfers", "path_hash").await {
        migration_statement(pool, 3, "ALTER TABLE transfers DROP COLUMN path_hash").await;
    }
}

async fn column_exists(pool: &MySqlPool, table: &str, column: &str) -> bool {
    let count: i64 = sqlx::query(
        "SELECT COUNT(*) FROM information_schema.columns
         WHERE table_schema = DATABASE() AND table_name = ? AND column_name = ?",
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await
    .expect("schema introspection")
    .get(0);
    count > 0
}

async fn index_exists(pool: &MySqlPool, table: &str, index: &str) -> bool {
    let count: i64 = sqlx::query(
        "SELECT COUNT(*) FROM information_schema.statistics
         WHERE table_schema = DATABASE() AND table_name = ? AND index_name = ?",
    )
    .bind(table)
    .bind(index)
    .fetch_one(pool)
    .await
    .expect("schema introspection")
    .get(0);
    count > 0
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
    if !column_exists(pool, "transfers", "failure_reason").await {
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
