//! Std-based platform adapters for P2 (clock, timers, affinity stubs).
use std::time::{Instant, Duration};

use limen_core::platform::{PlatformClock, Timers, Affinity};
use limen_core::types::Ticks;

/// A std-based clock using `Instant` as the monotonic source.
#[derive(Debug, Clone)]
pub struct StdClock {
    zero: Instant,
    ticks_per_ns: f64,
}

impl StdClock {
    /// Create a new clock epoch.
    pub fn new() -> Self {
        Self { zero: Instant::now(), ticks_per_ns: 1.0 }
    }
}

impl PlatformClock for StdClock {
    fn now_ticks(&self) -> Ticks {
        let dur = self.zero.elapsed();
        Ticks(dur.as_nanos() as u64)
    }

    fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
        ticks.0
    }

    fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        Ticks(ns)
    }
}

/// A timers stub that sleeps the current thread; suitable for profiling but not RT.
pub struct StdTimers;

impl Timers for StdTimers {
    fn sleep_until(&self, ticks: Ticks) {
        let now = Instant::now();
        // Best-effort: treat ticks as ns since boot; we cannot compute absolute here,
        // so a real implementation would use a shared clock epoch. This stub uses sleep_for.
        let _ = ticks;
        let _ = now;
    }

    fn sleep_for(&self, ticks: Ticks) {
        let ns = ticks.0;
        let dur = Duration::from_nanos(ns);
        std::thread::sleep(dur);
    }
}

/// A no-op affinity implementation; real pinning is platform-specific.
pub struct NoopAffinity;

impl Affinity for NoopAffinity {
    fn pin_to_core(&self, _core_id: u32) {}
    fn set_numa_node(&self, _node_id: u32) {}
}
