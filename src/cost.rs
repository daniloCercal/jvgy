//! Cost governor: tracks STT + LLM spend, enforces a daily ceiling, and trips a
//! kill-switch. Shared across workers via `Arc`. Resets at UTC midnight.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct CostGovernor {
    ceiling_usd: f64,
    /// Spend in micro-dollars (integer atomics; avoids float atomic).
    spent_micros: AtomicU64,
    tripped: AtomicBool,
    day: Mutex<u64>,
}

impl CostGovernor {
    pub fn new(ceiling_usd: f64) -> Self {
        Self {
            ceiling_usd,
            spent_micros: AtomicU64::new(0),
            tripped: AtomicBool::new(false),
            day: Mutex::new(current_day()),
        }
    }

    /// Whether spending is currently allowed (under ceiling, kill-switch off).
    pub fn allowed(&self) -> bool {
        self.maybe_reset_day();
        !self.tripped.load(Ordering::Relaxed)
    }

    /// Record incurred cost. Trips the kill-switch when the ceiling is crossed.
    /// Returns `true` only on the call that *caused* the trip, so the caller can
    /// emit a single notice.
    pub fn record(&self, usd: f64) -> bool {
        self.maybe_reset_day();
        if usd <= 0.0 || self.ceiling_usd <= 0.0 {
            return false;
        }
        let micros = (usd * 1_000_000.0) as u64;
        let total = self.spent_micros.fetch_add(micros, Ordering::Relaxed) + micros;
        if total as f64 / 1_000_000.0 >= self.ceiling_usd {
            // swap returns the previous value; we caused the trip if it was false.
            !self.tripped.swap(true, Ordering::Relaxed)
        } else {
            false
        }
    }

    pub fn spent_usd(&self) -> f64 {
        self.spent_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    pub fn ceiling_usd(&self) -> f64 {
        self.ceiling_usd
    }

    fn maybe_reset_day(&self) {
        let today = current_day();
        let mut day = self.day.lock().unwrap();
        if *day != today {
            *day = today;
            self.spent_micros.store(0, Ordering::Relaxed);
            self.tripped.store(false, Ordering::Relaxed);
        }
    }
}

fn current_day() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_until_ceiling_then_trips_once() {
        let g = CostGovernor::new(1.0);
        assert!(g.allowed());
        assert!(!g.record(0.4));
        assert!(g.allowed());
        // Crossing 1.0 trips and returns true exactly once.
        let caused = g.record(0.7);
        assert!(caused, "crossing the ceiling should report the trip");
        assert!(!g.allowed());
        // A further charge does not re-report the trip.
        assert!(!g.record(0.5));
    }

    #[test]
    fn zero_ceiling_never_trips() {
        let g = CostGovernor::new(0.0);
        assert!(!g.record(100.0));
        assert!(g.allowed());
        assert!((g.spent_usd() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracks_spend() {
        let g = CostGovernor::new(100.0);
        g.record(1.5);
        g.record(2.25);
        assert!((g.spent_usd() - 3.75).abs() < 1e-6);
    }
}
