//! Platform abstractions (clock, timers, affinities).
//!
//! Implementations are provided by `limen-platform` or the runtimes.

use crate::types::{DeadlineNs, Ticks};

/// A monotonic platform clock.
pub trait PlatformClock {
    /// Return the current monotonic tick count.
    fn now_ticks(&self) -> Ticks;

    /// Convert ticks to nanoseconds.
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64;

    /// Convert nanoseconds to ticks.
    fn nanos_to_ticks(&self, ns: u64) -> Ticks;
}

/// Optional timers service (P1/P2).
pub trait Timers {
    /// Sleep/yield until the given tick, if supported.
    fn sleep_until(&self, ticks: Ticks);

    /// Sleep/yield for the given number of ticks.
    fn sleep_for(&self, ticks: Ticks);
}

/// Optional affinities/NUMA hints (P2).
pub trait Affinity {
    /// Pin the current worker to a logical core or group.
    fn pin_to_core(&self, core_id: u32);

    /// Provide NUMA node hint.
    fn set_numa_node(&self, node_id: u32);
}
