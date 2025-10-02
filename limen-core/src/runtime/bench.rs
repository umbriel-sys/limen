//! (Work)bench [test] Runtime implementation.

use crate::errors::{NodeErrorKind, RuntimeError};
use crate::graph::GraphApi;
use crate::policy::WatermarkState;
use crate::queue::QueueOccupancy;
use core::marker::PhantomData;

use super::LimenRuntime;

/// A tiny, no_std test runtime:
/// - round-robin over nodes
/// - uses a single occupancy array
/// - no heap, no threads, no timers
pub struct TestNoStdRuntime<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    stop: bool,
    next: usize,
    occ: [QueueOccupancy; EDGE_COUNT],
    _clock: PhantomData<()>,
    _telemetry: PhantomData<()>,
}

impl<const NODE_COUNT: usize, const EDGE_COUNT: usize> TestNoStdRuntime<NODE_COUNT, EDGE_COUNT> {
    /// Construct with a pessimistic initial occupancy; `init()` will overwrite it.
    pub const fn new() -> Self {
        const INIT_OCC: QueueOccupancy = QueueOccupancy {
            items: 0,
            bytes: 0,
            // Any value is fine; init() will replace the whole array.
            watermark: WatermarkState::AtOrAboveHard,
        };
        Self {
            stop: false,
            next: 0,
            occ: [INIT_OCC; EDGE_COUNT],
            _clock: PhantomData,
            _telemetry: PhantomData,
        }
    }
}

impl<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT> for TestNoStdRuntime<NODE_COUNT, EDGE_COUNT>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
{
    type Clock = ();
    type Telemetry = ();
    type Error = RuntimeError;

    #[inline]
    fn init(
        &mut self,
        graph: &Graph,
        _clock: Self::Clock,
        _telemetry: Self::Telemetry,
    ) -> Result<(), Self::Error> {
        // Validate (pure, read-only).
        graph.validate_graph().map_err(RuntimeError::from)?;
        // Snapshot occupancies into our persistent buffer.
        graph
            .write_all_edge_occupancies(&mut self.occ)
            .map_err(RuntimeError::from)?;
        self.stop = false;
        self.next = 0;
        Ok(())
    }

    #[inline]
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error> {
        self.stop = false;
        self.next = 0;
        graph
            .write_all_edge_occupancies(&mut self.occ)
            .map_err(RuntimeError::from)?;
        Ok(())
    }

    #[inline]
    fn request_stop(&mut self) {
        self.stop = true;
    }

    #[inline]
    fn is_stopping(&self) -> bool {
        self.stop
    }

    #[inline]
    fn occupancies(&self) -> &[QueueOccupancy; EDGE_COUNT] {
        &self.occ
    }

    #[inline]
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error> {
        if self.stop {
            return Ok(false);
        }

        // Try each node once, starting from `self.next` (round-robin).
        let start = self.next;
        let mut tried = 0usize;

        while tried < NODE_COUNT {
            let i = (start + tried) % NODE_COUNT;

            // Drive one node by index; node type & arity are encoded in the generated match.
            match graph.step_node_by_index(i, &(), &mut ()) {
                Ok(_sr) => {
                    // Consider any Ok(_) as progress; refresh occupancies.
                    graph
                        .write_all_edge_occupancies(&mut self.occ)
                        .map_err(RuntimeError::from)?;
                    self.next = (i + 1) % NODE_COUNT;
                    return Ok(true);
                }
                Err(e) => {
                    // Treat "no work" classes as a soft miss; try next node.
                    match e.kind {
                        NodeErrorKind::NoInput | NodeErrorKind::Backpressured => {
                            tried += 1;
                            continue;
                        }
                        _ => return Err(RuntimeError::from(e)),
                    }
                }
            }
        }

        // We tried all nodes and none made progress.
        Ok(false)
    }
}
