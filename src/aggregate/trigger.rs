//! Summary trigger policy: fire on a time interval, a token-volume threshold, or
//! an explicit manual request (`!summary`).

use std::time::{Duration, Instant};

pub struct Trigger {
    interval: Duration,
    token_threshold: usize,
    last_fire: Instant,
    manual: bool,
}

impl Trigger {
    pub fn new(interval: Duration, token_threshold: usize) -> Self {
        Self {
            interval,
            token_threshold,
            last_fire: Instant::now(),
            manual: false,
        }
    }

    /// Request an out-of-band summary on the next evaluation.
    pub fn request_manual(&mut self) {
        self.manual = true;
    }

    pub fn should_fire(&self, unsummarized_tokens: usize, now: Instant) -> bool {
        self.manual
            || (unsummarized_tokens > 0 && now.duration_since(self.last_fire) >= self.interval)
            || (self.token_threshold > 0 && unsummarized_tokens >= self.token_threshold)
    }

    pub fn record_fire(&mut self, now: Instant) {
        self.last_fire = now;
        self.manual = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_volume_threshold() {
        let t = Trigger::new(Duration::from_secs(3600), 100);
        assert!(!t.should_fire(50, Instant::now()));
        assert!(t.should_fire(100, Instant::now()));
    }

    #[test]
    fn fires_on_manual_request_even_when_empty() {
        let mut t = Trigger::new(Duration::from_secs(3600), 100);
        assert!(!t.should_fire(0, Instant::now()));
        t.request_manual();
        assert!(t.should_fire(0, Instant::now()));
    }

    #[test]
    fn fires_on_interval_only_with_content() {
        let t = Trigger::new(Duration::from_secs(0), 0);
        // Interval elapsed (0s) but no content -> no fire.
        assert!(!t.should_fire(0, Instant::now()));
        // Interval elapsed with content -> fire.
        assert!(t.should_fire(1, Instant::now()));
    }

    #[test]
    fn record_clears_manual() {
        let mut t = Trigger::new(Duration::from_secs(3600), 0);
        t.request_manual();
        t.record_fire(Instant::now());
        assert!(!t.should_fire(0, Instant::now()));
    }
}
