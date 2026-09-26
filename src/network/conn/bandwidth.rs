use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;

const CHUNKS_PER_SECOND: u64 = 4;

pub struct Bandwidth {
    limit_bps: AtomicU64,
    paid_until: Mutex<Instant>,
}

impl Default for Bandwidth {
    fn default() -> Self {
        Self {
            limit_bps: AtomicU64::new(0),
            paid_until: Mutex::new(Instant::now()),
        }
    }
}

impl Bandwidth {
    pub fn set_limit(&self, limit_bps: u64) {
        self.limit_bps.store(limit_bps, Ordering::Relaxed);
    }

    pub(super) fn chunk_len(&self, buffer_len: usize) -> usize {
        match self.limit_bps.load(Ordering::Relaxed) {
            0 => buffer_len,
            limit => buffer_len.min((limit / CHUNKS_PER_SECOND).max(1) as usize),
        }
    }

    pub(super) fn charge(&self, count: u64) -> Option<Instant> {
        let limit = self.limit_bps.load(Ordering::Relaxed);
        if limit == 0 {
            return None;
        }
        let mut paid_until = self.paid_until.lock().unwrap();
        let start = (*paid_until).max(Instant::now());
        *paid_until = start + Duration::from_secs_f64(count as f64 / limit as f64);
        Some(*paid_until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits_and_reads_full_buffers() {
        let bandwidth = Bandwidth::default();
        assert_eq!(bandwidth.charge(1 << 20), None);
        assert_eq!(bandwidth.chunk_len(65536), 65536);
    }

    #[test]
    fn chunks_are_sized_to_a_fraction_of_the_limit() {
        let bandwidth = Bandwidth::default();
        bandwidth.set_limit(1024);
        assert_eq!(bandwidth.chunk_len(65536), 256);
        bandwidth.set_limit(10 << 20);
        assert_eq!(bandwidth.chunk_len(65536), 65536);
    }

    #[test]
    fn concurrent_transfers_share_one_budget() {
        let bandwidth = Bandwidth::default();
        bandwidth.set_limit(1000);
        let before = Instant::now();
        let first = bandwidth.charge(500).unwrap();
        let second = bandwidth.charge(500).unwrap();
        let third = bandwidth.charge(1000).unwrap();
        assert!(first >= before + Duration::from_millis(500));
        assert_eq!(second - first, Duration::from_millis(500));
        assert_eq!(third - second, Duration::from_secs(1));
    }

    #[test]
    fn limit_changes_apply_to_the_next_charge() {
        let bandwidth = Bandwidth::default();
        bandwidth.set_limit(1000);
        let first = bandwidth.charge(1000).unwrap();
        bandwidth.set_limit(4000);
        let second = bandwidth.charge(1000).unwrap();
        assert_eq!(second - first, Duration::from_millis(250));
        bandwidth.set_limit(0);
        assert_eq!(bandwidth.charge(1000), None);
    }
}
