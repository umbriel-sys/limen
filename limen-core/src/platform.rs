//! Platform abstractions.
//!
//! ## Boundary: what enters `StepContext`
//! Only a **monotonic clock** enters `StepContext`. Nodes may **observe** time,
//! but **do not** control scheduling (sleep/yield) or placement (affinity) via the
//! context. This preserves portability, `no_std` viability, determinism in tests,
//! and keeps scheduling policy in the runtime.
//!
//! ## Runtime-level aides (do **not** enter `StepContext`)
//! - [`Timers`]: sleeping/yielding belongs to the runtime/scheduler.
//! - [`Affinity`]: core/NUMA placement is a runtime concern.
//!
//! Implementations of these traits are provided by `limen-platform` or host runtimes.

pub mod linux;

use crate::types::Ticks;

/// A monotonic platform clock.
///
/// The epoch is implementation-defined; monotonicity is required.
/// Conversions allow runtimes to expose a fast tick domain and still
/// provide nanosecond correlation for telemetry and tracing.
pub trait PlatformClock {
    /// Return the current monotonic tick count.
    fn now_ticks(&self) -> Ticks;

    /// Convert ticks to nanoseconds.
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64;

    /// Convert nanoseconds to ticks.
    fn nanos_to_ticks(&self, ns: u64) -> Ticks;
}

/// A no-op clock implementation that always returns tick zero, useful for testing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopClock;

impl PlatformClock for NoopClock {
    #[inline]
    fn now_ticks(&self) -> Ticks {
        Ticks::new(0)
    }

    #[inline]
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
        *ticks.as_u64()
    }

    #[inline]
    fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        Ticks::new(ns)
    }
}

impl PlatformClock for () {
    #[inline]
    fn now_ticks(&self) -> Ticks {
        Ticks::new(0)
    }

    #[inline]
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
        *ticks.as_u64()
    }

    #[inline]
    fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        Ticks::new(ns)
    }
}

/// Timing span helper backed by a `PlatformClock`.
///
/// A span records a start tick from the provided clock and can be closed to
/// obtain an elapsed duration in nanoseconds.
pub struct Span<'a, C: PlatformClock> {
    /// Clock used to obtain ticks and convert them to nanoseconds.
    clk: &'a C,
    /// Tick value at the start of the span.
    start: Ticks,
}

impl<'a, C: PlatformClock> Span<'a, C> {
    /// Start a new span using the given platform clock.
    #[inline]
    pub fn start(clk: &'a C) -> Self {
        Self {
            clk,
            start: clk.now_ticks(),
        }
    }

    /// End the span and return the elapsed time in nanoseconds.
    #[inline]
    pub fn end_ns(self) -> u64 {
        let end = self.clk.now_ticks();
        let t0 = self.clk.ticks_to_nanos(self.start);
        let t1 = self.clk.ticks_to_nanos(end);
        t1.saturating_sub(t0)
    }
}

/// Optional timers service (P1/P2).
///
/// **Does not** enter `StepContext`. If a node requires timers, the host
/// should inject a handle at construction time (out-of-band), not via the context.
pub trait Timers {
    /// Sleep/yield until the given tick, if supported.
    fn sleep_until(&self, ticks: Ticks);

    /// Sleep/yield for the given number of ticks.
    fn sleep_for(&self, ticks: Ticks);
}

/// Optional affinities/NUMA hints (P2).
///
/// **Does not** enter `StepContext`. Placement and topology hints are owned
/// by the runtime/scheduler, not by nodes.
pub trait Affinity {
    /// Pin the current worker to a logical core or group.
    fn pin_to_core(&self, core_id: u32);

    /// Provide NUMA node hint.
    fn set_numa_node(&self, node_id: u32);
}
