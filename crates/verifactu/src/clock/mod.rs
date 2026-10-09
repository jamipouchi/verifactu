//! The `Clock` port: production code reads time exclusively through it,
//! tests inject [`FakeClock`]. Timestamps are UTC epoch seconds
//! everywhere; the zone is presentation only.

pub mod civil;
pub mod zone;

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

pub trait Clock: Send + Sync {
    fn now_utc(&self) -> Timestamp;
}

/// Real wall clock over `std::time::SystemTime` — the only production
/// implementation. Deliberately untested: a test would itself read the
/// wall clock.
#[derive(Copy, Clone, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc(&self) -> Timestamp {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock reports a time before the Unix epoch — host misconfiguration");
        Timestamp(elapsed.as_secs())
    }
}

/// A settable fixed clock: tests reproduce byte-identical emissions,
/// and wasm builds (no `SystemTime`) bring their own tiny clock over
/// the runtime's date API instead.
#[derive(Debug)]
pub struct FakeClock(std::sync::Mutex<Timestamp>);

impl FakeClock {
    #[must_use]
    pub fn new(start: Timestamp) -> Self {
        Self(std::sync::Mutex::new(start))
    }

    /// # Panics
    /// If the state lock is poisoned.
    pub fn set(&self, instant: Timestamp) {
        *self.0.lock().expect("fake clock lock") = instant;
    }
}

impl Clock for FakeClock {
    fn now_utc(&self) -> Timestamp {
        *self.0.lock().expect("fake clock lock")
    }
}
