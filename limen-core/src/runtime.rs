//! Runtime trait and stop-handle for Limen graph executors.
//!
//! [`LimenRuntime`] is the uniform contract that all Limen runtimes implement.
//! It owns a clock and telemetry after [`LimenRuntime::init`] and drives
//! execution through [`LimenRuntime::step`] / [`LimenRuntime::run`].
//!
//! `RuntimeStopHandle` (`std` only) allows cooperative stop requests from
//! outside the runtime loop (e.g. from another thread).

#[cfg(any(test, feature = "bench"))]
pub mod bench;

#[cfg(test)]
mod tests;

use crate::{
    edge::EdgeOccupancy,
    graph::GraphApi,
    prelude::{PlatformClock, Telemetry},
};

/// An opaque, cloneable, thread-safe stop handle for external cooperative stop.
///
/// Wraps an `Arc<AtomicBool>`. Calling `request_stop()` sets the flag;
/// `is_stopping()` reads it. Safe to send to another thread and use while
/// `LimenRuntime::run()` holds `&mut self`.
#[cfg(feature = "std")]
#[derive(Clone)]
pub struct RuntimeStopHandle {
    flag: std::sync::Arc<core::sync::atomic::AtomicBool>,
}

#[cfg(feature = "std")]
impl RuntimeStopHandle {
    /// Create a new stop handle wrapping the given atomic flag.
    pub fn new(flag: std::sync::Arc<core::sync::atomic::AtomicBool>) -> Self {
        Self { flag }
    }

    /// Request cooperative stop (visible to the runtime's scheduler).
    pub fn request_stop(&self) {
        self.flag.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    /// Return `true` if stop has been requested.
    pub fn is_stopping(&self) -> bool {
        self.flag.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// A single, uniform runtime trait that all Limen runtimes (P0, P1, P2, P2Concurrent)
/// can implement. The API is allocation- and threading-agnostic.
///
/// The runtime *owns* its clock & telemetry after `init` and no longer
/// threads them through `step()` / `run()`.
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
    Self::Clock: PlatformClock + Sized,
    Self::Telemetry: Telemetry + Sized,
{
    /// Clock abstraction stored by the runtime. Use `()` if not needed.
    type Clock;

    /// Telemetry collector stored by the runtime. Use `()` if not needed.
    type Telemetry;

    /// Error type produced by the runtime. Use `core::convert::Infallible` if none.
    type Error;

    /// External stop handle type. `Clone + Send + Sync + 'static`.
    /// Only available under `std`.
    #[cfg(feature = "std")]
    type StopHandle: Clone + Send + Sync + 'static;

    /// Initialize internal state and adopt the provided clock & telemetry.
    fn init(
        &mut self,
        graph: &mut Graph,
        clock: Self::Clock,
        telemetry: Self::Telemetry,
    ) -> Result<(), Self::Error>;

    /// Reset internal runtime state (keep graph/node state unless your policy requires otherwise).
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error>;

    /// Request a cooperative stop.
    fn request_stop(&mut self);

    /// Return an external stop handle, if the runtime supports it.
    /// Clone before calling `run()` to enable stopping from another thread.
    #[cfg(feature = "std")]
    fn stop_handle(&self) -> Option<Self::StopHandle> {
        None
    }

    /// Return `true` iff a stop has been requested.
    fn is_stopping(&self) -> bool;

    /// Borrow the runtime’s persistent edge-occupancy buffer.
    fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT];

    /// Execute one scheduler tick. Return `Ok(true)` if more work remains, `Ok(false)` to stop.
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error>;

    /// Drive the runtime until `step()` returns `false` or a stop is requested.
    #[inline]
    fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error> {
        while !self.is_stopping() {
            if !self.step(graph)? {
                break;
            }
        }
        Ok(())
    }
}
