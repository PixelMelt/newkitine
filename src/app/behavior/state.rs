use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::policy::Verdict;

const WINDOW_SECS: i64 = 600;

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
}

#[derive(Default)]
pub struct Behavior {
    pub peers: Mutex<HashMap<String, Peer>>,
}

pub fn prune(window: &mut VecDeque<i64>, timestamp: i64) {
    while window
        .front()
        .is_some_and(|entry| timestamp - entry > WINDOW_SECS)
    {
        window.pop_front();
    }
}
