use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::policy::Verdict;

const WINDOW_SECS: i64 = 600;
const PEER_STATE_LIMIT: usize = 4096;

#[derive(Default, Clone, Copy, PartialEq)]
pub enum Check {
    #[default]
    Idle,
    AwaitingStats,
    AwaitingBrowse,
}

#[derive(Default)]
pub struct Peer {
    pub searches: VecDeque<i64>,
    pub queue_requests: VecDeque<i64>,
    pub stats: Option<(u32, u32)>,
    pub verdict: Verdict,
    pub evidence: Vec<String>,
    pub check: Check,
    pub last_activity: i64,
}

#[derive(Default)]
pub struct Behavior {
    pub peers: Mutex<HashMap<String, Peer>>,
    pub transition: tokio::sync::Mutex<()>,
}

pub fn touch<'a>(
    peers: &'a mut HashMap<String, Peer>,
    username: &str,
    timestamp: i64,
) -> &'a mut Peer {
    if peers.len() >= PEER_STATE_LIMIT && !peers.contains_key(username) {
        peers.retain(|_, peer| {
            peer.verdict >= Verdict::Suspect || timestamp - peer.last_activity <= WINDOW_SECS
        });
    }
    let peer = peers.entry(username.to_owned()).or_default();
    peer.last_activity = timestamp;
    peer
}

pub fn prune(window: &mut VecDeque<i64>, timestamp: i64) {
    while window
        .front()
        .is_some_and(|entry| timestamp - entry > WINDOW_SECS)
    {
        window.pop_front();
    }
}
