use std::collections::HashSet;
use std::sync::Mutex;

use soulseek::types::TransferDirection;
use sqlx::{MySqlPool, Row};

#[derive(Clone, Default)]
pub struct DirectionTotals {
    pub count: u64,
    pub bytes: u64,
    pub avg_speed_bps: u64,
}

#[derive(Clone, Default)]
pub struct TransferTotalsSnapshot {
    pub uploads: DirectionTotals,
    pub downloads: DirectionTotals,
    pub upload_users: u64,
}

#[derive(Default)]
struct DirectionState {
    count: u64,
    bytes: u64,
    speed_sum: u64,
    speed_count: u64,
}

#[derive(Default)]
struct State {
    uploads: DirectionState,
    downloads: DirectionState,
    upload_users: HashSet<String>,
}

pub struct TransferTotals {
    state: Mutex<State>,
}

impl TransferTotals {
    pub async fn load(pool: &MySqlPool) -> Self {
        let mut state = State::default();
        for row in sqlx::query(
            "SELECT direction, COUNT(*), CAST(SUM(COALESCE(bytes, size)) AS UNSIGNED),
                    CAST(COALESCE(SUM(speed_bps), 0) AS UNSIGNED), COUNT(speed_bps)
             FROM transfer_history GROUP BY direction",
        )
        .fetch_all(pool)
        .await
        .expect("transfer totals")
        {
            let totals = match row.get::<String, _>(0).as_str() {
                "upload" => &mut state.uploads,
                "download" => &mut state.downloads,
                direction => panic!("unknown transfer history direction {direction}"),
            };
            totals.count = row.get::<i64, _>(1) as u64;
            totals.bytes = row.get(2);
            totals.speed_sum = row.get(3);
            totals.speed_count = row.get::<i64, _>(4) as u64;
        }
        state.upload_users = sqlx::query(
            "SELECT DISTINCT username FROM transfer_history WHERE direction = 'upload'",
        )
        .fetch_all(pool)
        .await
        .expect("upload users")
        .into_iter()
        .map(|row| row.get(0))
        .collect();
        Self {
            state: Mutex::new(state),
        }
    }

    pub fn record(
        &self,
        direction: TransferDirection,
        username: &str,
        bytes: u64,
        speed_bps: Option<u32>,
    ) {
        let mut state = self.state.lock().unwrap();
        if direction == TransferDirection::Upload {
            state.upload_users.insert(username.to_owned());
        }
        let totals = match direction {
            TransferDirection::Upload => &mut state.uploads,
            TransferDirection::Download => &mut state.downloads,
        };
        totals.count += 1;
        totals.bytes += bytes;
        if let Some(speed) = speed_bps {
            totals.speed_sum += u64::from(speed);
            totals.speed_count += 1;
        }
    }

    pub fn snapshot(&self) -> TransferTotalsSnapshot {
        let state = self.state.lock().unwrap();
        TransferTotalsSnapshot {
            uploads: snapshot_direction(&state.uploads),
            downloads: snapshot_direction(&state.downloads),
            upload_users: state.upload_users.len() as u64,
        }
    }
}

fn snapshot_direction(state: &DirectionState) -> DirectionTotals {
    DirectionTotals {
        count: state.count,
        bytes: state.bytes,
        avg_speed_bps: state
            .speed_sum
            .checked_div(state.speed_count)
            .unwrap_or_default(),
    }
}
