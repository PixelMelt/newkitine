use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::sync::futures::Notified;
use tokio::time::Instant;

const CHUNKS_PER_SECOND: u64 = 4;

pub struct Bandwidth {
    limit_bps: AtomicU64,
    paid_until: Mutex<Instant>,
    changed: Notify,
    active: AtomicU64,
}

pub(super) struct Active<'a>(&'a Bandwidth);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) struct Grant {
    pub(super) len: usize,
    limit_bps: u64,
}

impl Default for Bandwidth {
    fn default() -> Self {
        Self {
            limit_bps: AtomicU64::new(0),
            paid_until: Mutex::new(Instant::now()),
            changed: Notify::new(),
            active: AtomicU64::new(0),
        }
    }
}

impl Bandwidth {
    pub fn set_limit(&self, limit_bps: u64) {
        self.limit_bps.store(limit_bps, Ordering::Relaxed);
        *self.paid_until.lock().unwrap() = Instant::now();
        self.changed.notify_waiters();
    }

    pub(super) fn join(&self) -> Active<'_> {
        self.active.fetch_add(1, Ordering::Relaxed);
        Active(self)
    }

    pub(super) fn changed(&self) -> Notified<'_> {
        self.changed.notified()
    }

    pub(super) fn wait_until(&self) -> Option<Instant> {
        if self.limit_bps.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let paid_until = *self.paid_until.lock().unwrap();
        (paid_until > Instant::now()).then_some(paid_until)
    }

    pub(super) fn grant(&self, max_len: u64) -> Grant {
        let limit_bps = self.limit_bps.load(Ordering::Relaxed);
        let len = match limit_bps {
            0 => max_len,
            limit => {
                let active = self.active.load(Ordering::Relaxed);
                max_len.min((limit / (CHUNKS_PER_SECOND * active)).max(1))
            }
        };
        Grant {
            len: len as usize,
            limit_bps,
        }
    }

    pub(super) fn charge(&self, grant: &Grant, count: u64) {
        if grant.limit_bps == 0 {
            return;
        }
        let mut paid_until = self.paid_until.lock().unwrap();
        let start = (*paid_until).max(Instant::now());
        *paid_until = start + Duration::from_secs_f64(count as f64 / grant.limit_bps as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits_and_grants_the_full_request() {
        let bandwidth = Bandwidth::default();
        let grant = bandwidth.grant(65536);
        assert_eq!(grant.len, 65536);
        bandwidth.charge(&grant, 65536);
        assert_eq!(bandwidth.wait_until(), None);
    }

    #[test]
    fn grants_are_sized_to_a_fraction_of_each_transfers_share() {
        let bandwidth = Bandwidth::default();
        bandwidth.set_limit(1024);
        let first = bandwidth.join();
        assert_eq!(bandwidth.grant(65536).len, 256);
        assert_eq!(bandwidth.grant(100).len, 100);
        let second = bandwidth.join();
        assert_eq!(bandwidth.grant(65536).len, 128);
        let crowd: Vec<_> = (0..1022).map(|_| bandwidth.join()).collect();
        assert_eq!(bandwidth.grant(65536).len, 1);
        drop(crowd);
        drop(second);
        assert_eq!(bandwidth.grant(65536).len, 256);
        drop(first);
        let _only = bandwidth.join();
        bandwidth.set_limit(u32::MAX as u64 * 1024);
        assert_eq!(bandwidth.grant(65536).len, 65536);
    }

    #[test]
    fn concurrent_transfers_share_one_budget() {
        let bandwidth = Bandwidth::default();
        bandwidth.set_limit(1000);
        let _first = bandwidth.join();
        let _second = bandwidth.join();
        let before = Instant::now();
        let first = bandwidth.grant(65536);
        let second = bandwidth.grant(65536);
        bandwidth.charge(&first, 250);
        let after_first = bandwidth.wait_until().unwrap();
        bandwidth.charge(&second, 250);
        let after_second = bandwidth.wait_until().unwrap();
        assert!(after_first >= before + Duration::from_millis(250));
        assert_eq!(after_second - after_first, Duration::from_millis(250));
    }

    #[test]
    fn a_limit_change_forgives_debt_and_prices_in_flight_grants_at_their_own_rate() {
        let bandwidth = Bandwidth::default();
        let _active = bandwidth.join();
        let unlimited = bandwidth.grant(65536);
        bandwidth.set_limit(1000);
        bandwidth.charge(&unlimited, 65536);
        assert_eq!(bandwidth.wait_until(), None);
        let slow = bandwidth.grant(65536);
        bandwidth.charge(&slow, slow.len as u64);
        assert!(bandwidth.wait_until().is_some());
        bandwidth.set_limit(4000);
        assert_eq!(bandwidth.wait_until(), None);
        bandwidth.set_limit(0);
        let fast = bandwidth.grant(65536);
        bandwidth.charge(&fast, 65536);
        assert_eq!(bandwidth.wait_until(), None);
    }
}
