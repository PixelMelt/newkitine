use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use newkitine::types::Observation;

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
            db::flush_activity(&app.db, &username, &activity, timestamp).await;
        }
    }
}
