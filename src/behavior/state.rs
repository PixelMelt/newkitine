use std::collections::HashMap;
use std::sync::Mutex;

use tokio::time::Instant;

use super::policy::Verdict;

const WINDOW_SECS: i64 = 600;
const PEER_STATE_LIMIT: usize = 4096;

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum Check {
    #[default]
    Idle,
    AwaitingStats(Instant),
    AwaitingBrowse(Instant),
}

impl Check {
    fn deadline(self) -> Option<Instant> {
        match self {
            Check::Idle => None,
            Check::AwaitingStats(deadline) | Check::AwaitingBrowse(deadline) => Some(deadline),
        }
    }
}

#[derive(Default)]
pub struct Peer {
    pub stats: Option<(u32, u32)>,
    pub verdict: Verdict,
    pub evidence: Vec<String>,
    pub check: Check,
    pub last_activity: i64,
}

impl Peer {
    pub fn awaits_release(&self) -> bool {
        self.verdict >= Verdict::Leech || self.check != Check::Idle
    }

    pub fn abandon_check(&mut self) {
        self.check = Check::Idle;
        self.stats = None;
    }

    pub fn expire_check(&mut self, deadline: Instant) -> bool {
        if self.check.deadline() != Some(deadline) {
            return false;
        }
        self.abandon_check();
        true
    }

    pub fn fail_browse(&mut self) -> bool {
        if !matches!(self.check, Check::AwaitingBrowse(_)) {
            return false;
        }
        self.abandon_check();
        true
    }
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
            peer.awaits_release() || timestamp - peer.last_activity <= WINDOW_SECS
        });
    }
    let peer = peers.entry(username.to_owned()).or_default();
    peer.last_activity = timestamp;
    peer
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn pending(check: Check) -> Peer {
        Peer {
            stats: Some((0, 0)),
            check,
            ..Default::default()
        }
    }

    #[test]
    fn only_the_matching_deadline_expires_a_check() {
        let stale = Instant::now();
        let current = stale + Duration::from_secs(1);
        let mut peer = pending(Check::AwaitingStats(current));
        assert!(!peer.expire_check(stale));
        assert_eq!(peer.check, Check::AwaitingStats(current));
        assert!(peer.expire_check(current));
        assert_eq!(peer.check, Check::Idle);
        assert_eq!(peer.stats, None);
        assert!(!peer.expire_check(current));
    }

    #[test]
    fn a_browse_check_expires_like_a_stats_check() {
        let deadline = Instant::now();
        let mut peer = pending(Check::AwaitingBrowse(deadline));
        assert!(peer.expire_check(deadline));
        assert_eq!(peer.check, Check::Idle);
    }

    #[test]
    fn browse_failure_only_ends_a_browse_check() {
        let deadline = Instant::now();
        let mut peer = pending(Check::AwaitingStats(deadline));
        assert!(!peer.fail_browse());
        assert_eq!(peer.check, Check::AwaitingStats(deadline));
        let mut peer = pending(Check::AwaitingBrowse(deadline));
        assert!(peer.fail_browse());
        assert_eq!(peer.check, Check::Idle);
        assert_eq!(peer.stats, None);
    }

    #[test]
    fn pending_and_convicted_peers_await_release() {
        assert!(!Peer::default().awaits_release());
        assert!(pending(Check::AwaitingStats(Instant::now())).awaits_release());
        let convicted = Peer {
            verdict: Verdict::Leech,
            ..Default::default()
        };
        assert!(convicted.awaits_release());
        let verified = Peer {
            verdict: Verdict::Verified,
            ..Default::default()
        };
        assert!(!verified.awaits_release());
    }

    #[test]
    fn eviction_keeps_peers_that_await_release() {
        let mut peers = HashMap::new();
        for index in 0..PEER_STATE_LIMIT {
            touch(&mut peers, &format!("idle{index}"), 0);
        }
        touch(&mut peers, "held", 0).check = Check::AwaitingStats(Instant::now());
        touch(&mut peers, "fresh", WINDOW_SECS * 10);
        assert!(peers.contains_key("held"));
        assert!(peers.contains_key("fresh"));
        assert!(!peers.contains_key("idle0"));
    }
}
