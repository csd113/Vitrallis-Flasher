//! Monotonic time seam so session/TTL logic is deterministic in tests.
use std::{
    fmt::Debug,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// A monotonic time source. Session logic never reads the wall clock directly.
pub trait Clock: Send + Sync + Debug {
    fn now(&self) -> Instant;
}

/// Production clock backed by the OS monotonic clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Test clock with explicit, deterministic advancement.
#[derive(Debug)]
pub struct TestClock {
    base: Instant,
    offset: Mutex<Duration>,
}
impl TestClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        }
    }
    /// Advances the clock by `duration`; only ever forward.
    pub fn advance(&self, duration: Duration) {
        let mut offset = self.offset.lock().unwrap_or_else(PoisonError::into_inner);
        *offset = offset.saturating_add(duration);
    }
    /// Total simulated time since construction.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}
impl Clock for TestClock {
    fn now(&self) -> Instant {
        let offset = *self.offset.lock().unwrap_or_else(PoisonError::into_inner);
        self.base.checked_add(offset).unwrap_or(self.base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_clock_advances_monotonically() {
        let clock = TestClock::new();
        let start = clock.now();
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.now().duration_since(start), Duration::from_secs(3));
        clock.advance(Duration::from_secs(4));
        assert_eq!(clock.elapsed(), Duration::from_secs(7));
        assert!(clock.now() > start);
    }
}
