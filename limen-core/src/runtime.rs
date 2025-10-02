#![cfg_attr(not(feature = "std"), no_std)]

pub mod bench;

use crate::{graph::GraphApi, queue::QueueOccupancy};

/// A single, uniform runtime trait that all Limen runtimes (P0, P1, P2, P2Concurrent)
/// can implement. The API is allocation- and threading-agnostic.
///
/// The runtime *owns* its clock & telemetry after `init` and no longer
/// threads them through `step()` / `run()`.
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
{
    /// Clock abstraction stored by the runtime. Use `()` if not needed.
    type Clock;

    /// Telemetry collector stored by the runtime. Use `()` if not needed.
    type Telemetry;

    /// Error type produced by the runtime. Use `core::convert::Infallible` if none.
    type Error;

    /// Initialize internal state and adopt the provided clock & telemetry.
    fn init(
        &mut self,
        graph: &Graph,
        clock: Self::Clock,
        telemetry: Self::Telemetry,
    ) -> Result<(), Self::Error>;

    /// Reset internal runtime state (keep graph/node state unless your policy requires otherwise).
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error>;

    /// Request a cooperative stop.
    fn request_stop(&mut self);

    /// Return `true` iff a stop has been requested.
    fn is_stopping(&self) -> bool;

    /// Borrow the runtime’s persistent edge-occupancy buffer.
    fn occupancies(&self) -> &[QueueOccupancy; EDGE_COUNT];

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
