use std::time::{Duration, Instant};

const SAMPLE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub(crate) struct SpeedMeter {
    last: Option<(Instant, u64)>,
    bps: u32,
}

impl SpeedMeter {
    pub(crate) fn sample(&mut self, bytes_done: u64) -> u32 {
        let now = Instant::now();
        match self.last {
            None => self.last = Some((now, bytes_done)),
            Some((at, bytes)) => {
                let elapsed = now.duration_since(at);
                if elapsed >= SAMPLE_WINDOW && bytes_done >= bytes {
                    self.bps = ((bytes_done - bytes) as f64 / elapsed.as_secs_f64()) as u32;
                    self.last = Some((now, bytes_done));
                }
            }
        }
        self.bps
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
        self.bps = 0;
    }
}
