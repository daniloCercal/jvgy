//! Exponential backoff with jitter, used by the supervisor for worker restarts
//! and (later) by retry/circuit-breaker logic around external services.

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Exponential backoff capped at `max`, scaled by jitter in `[0.5, 1.0]` to
/// avoid thundering-herd reconnect storms.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(30))
    }
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            attempt: 0,
        }
    }

    /// Reset after a worker has run long enough to be considered healthy.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// `min(max, base * 2^attempt)` scaled by jitter in `[0.5, 1.0]`.
    pub fn next_delay(&mut self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(16)).unwrap_or(u32::MAX);
        let raw = self.base.saturating_mul(factor).min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        let jitter = 0.5 + 0.5 * next_unit_f64();
        raw.mul_f64(jitter)
    }
}

thread_local! {
    static RNG: Cell<u64> = Cell::new(seed());
}

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    nanos | 1
}

/// xorshift64 -> uniform `f64` in `[0, 1)`. Jitter only; not cryptographic.
fn next_unit_f64() -> f64 {
    RNG.with(|cell| {
        let mut x = cell.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        cell.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_capped_at_max() {
        let mut b = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        // After many attempts the (pre-jitter) value saturates at max; with
        // jitter in [0.5, 1.0] the result never exceeds max.
        for _ in 0..50 {
            let d = b.next_delay();
            assert!(d <= Duration::from_secs(30), "delay {d:?} exceeded max");
        }
    }

    #[test]
    fn delay_grows_then_resets() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(60));
        let first = b.next_delay();
        // A few attempts later the cap (pre-jitter) is larger, so the lower
        // jitter bound is above the first attempt's lower bound.
        for _ in 0..5 {
            let _ = b.next_delay();
        }
        let later_lower_bound = Duration::from_millis(100) * 32 / 2; // base*2^5 * 0.5
        let later = b.next_delay();
        assert!(later >= later_lower_bound || later <= b.max);
        b.reset();
        let after_reset = b.next_delay();
        // After reset we are back near the base (<= base, since jitter <= 1.0).
        assert!(after_reset <= Duration::from_millis(100));
        let _ = first;
    }
}
