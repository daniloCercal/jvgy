//! Token-bucket rate limiter for the Discord webhook (30 msgs/min). Used by the
//! single output worker, so it is `&mut self` (no locking). The time-dependent
//! core takes an explicit `now` for deterministic tests.

use std::time::{Duration, Instant};

pub struct RateLimiter {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn per_minute(n: u32) -> Self {
        let capacity = n.max(1) as f64;
        Self {
            capacity,
            tokens: capacity,
            refill_per_sec: capacity / 60.0,
            last: Instant::now(),
        }
    }

    fn refill_at(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + dt * self.refill_per_sec).min(self.capacity);
        self.last = now;
    }

    /// Non-blocking attempt; consumes a token if available.
    pub fn try_acquire_at(&mut self, now: Instant) -> bool {
        self.refill_at(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Block until a token is available, then consume it.
    pub async fn acquire(&mut self) {
        loop {
            if self.try_acquire_at(Instant::now()) {
                return;
            }
            let need = 1.0 - self.tokens;
            let wait = (need / self.refill_per_sec).max(0.01);
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_capacity_then_refills() {
        let t0 = Instant::now();
        let mut rl = RateLimiter::per_minute(30);
        // Drain the full burst.
        for _ in 0..30 {
            assert!(rl.try_acquire_at(t0));
        }
        // 31st is denied at the same instant.
        assert!(!rl.try_acquire_at(t0));
        // 30/min = 1 token / 2s; after 2s exactly one token is back.
        assert!(rl.try_acquire_at(t0 + Duration::from_secs(2)));
        assert!(!rl.try_acquire_at(t0 + Duration::from_secs(2)));
    }
}
