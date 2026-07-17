use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::MySqlPool;
use sqlx::Row;

use crate::client::Observation;

use super::api;
use super::db;
use super::geo::Geo;
use super::state::{App, now};

#[derive(Default)]
pub struct StatsSink {
    pending: Mutex<HashMap<String, PeerActivity>>,
}

#[derive(Default, Clone)]
pub struct PeerActivity {
    pub searches: u32,
    pub searches_matched: u32,
    pub queue_requests: u32,
    pub queue_rejected: u32,
    pub browses: u32,
    pub info_requests: u32,
    pub folder_requests: u32,
    pub connections: u32,
    pub last_ip: Option<String>,
    pub country: Option<String>,
}

impl StatsSink {
    pub fn record(&self, observation: &Observation, geo: Option<&Geo>) {
        let mut pending = self.pending.lock().unwrap();
        match observation {
            Observation::SearchSeen {
                username, matched, ..
            } => {
                let entry = pending.entry(username.clone()).or_default();
                entry.searches += 1;
                if *matched {
                    entry.searches_matched += 1;
                }
            }
            Observation::QueueRequest {
                username, accepted, ..
            } => {
                let entry = pending.entry(username.clone()).or_default();
                entry.queue_requests += 1;
                if !accepted {
                    entry.queue_rejected += 1;
                }
            }
            Observation::BrowseRequest { username } => {
                pending.entry(username.clone()).or_default().browses += 1;
            }
            Observation::UserInfoRequest { username } => {
                pending.entry(username.clone()).or_default().info_requests += 1;
            }
            Observation::FolderContentsRequest { username } => {
                pending.entry(username.clone()).or_default().folder_requests += 1;
            }
            Observation::PeerConnected { username, ip, .. } => {
                let entry = pending.entry(username.clone()).or_default();
                entry.connections += 1;
                if let Some(ip) = ip {
                    entry.last_ip = Some(ip.to_string());
                    if let Some(geo) = geo {
                        entry.country = geo.country(*ip);
                    }
                }
            }
        }
    }

    pub fn drain(&self) -> HashMap<String, PeerActivity> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

pub async fn flush_loop(app: Arc<App>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let drained = app.stats.drain();
        if drained.is_empty() {
            continue;
        }
        let timestamp = now();
        for (username, activity) in drained {
            super::peer_history::flush_activity(&app.db, &username, &activity, timestamp)
                .await
                .unwrap_or_else(|error| db::fatal(error));
        }
    }
}

async fn transfer_stats(pool: &MySqlPool) -> serde_json::Value {
    let mut totals = serde_json::Map::new();
    for direction in ["download", "upload"] {
        totals.insert(
            direction.into(),
            serde_json::json!({ "count": 0, "bytes": 0, "avg_speed_bps": 0 }),
        );
    }
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

async fn peers_seen(pool: &MySqlPool, limit: u32) -> serde_json::Value {
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

async fn verdict_users(pool: &MySqlPool) -> serde_json::Value {
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

pub(super) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/stats/transfers", get(stats_transfers))
        .route("/api/stats/peers", get(stats_peers))
        .route("/api/stats/verdicts", get(stats_verdicts))
}

async fn stats_transfers(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(transfer_stats(&app.db).await)
}

#[derive(Deserialize)]
struct PeersQuery {
    #[serde(default = "default_peers_limit")]
    limit: u32,
}

fn default_peers_limit() -> u32 {
    100
}

const PEERS_LIMIT_MAX: u32 = 1000;

async fn stats_peers(
    State(app): State<Arc<App>>,
    Query(query): Query<PeersQuery>,
) -> axum::response::Response {
    if query.limit > PEERS_LIMIT_MAX {
        return api::limit_rejected(query.limit, PEERS_LIMIT_MAX);
    }
    Json(peers_seen(&app.db, query.limit).await).into_response()
}

async fn stats_verdicts(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(verdict_users(&app.db).await)
}
