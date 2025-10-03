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

    /// Decide whether a node's `StepResult` constitutes "progress".
    /// Currently conservative: treat any `Ok(_)` as progress to keep the runtime simple.
    /// If/when `StepResult` exposes a richer API (e.g., `is_progress()`), update this.
    #[inline]
    fn made_progress(sr: &crate::node::StepResult) -> bool {
        // TODO: narrow when StepResult variants are available (e.g., matches!(sr, StepResult::Progress | StepResult::Output))
        let _ = sr; // silence unused for now
        true
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
        graph: &mut Graph,
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
                Ok(sr) => {
                    if Self::made_progress(&sr) {
                        // Consider progress and refresh the full snapshot.
                        graph
                            .write_all_edge_occupancies(&mut self.occ)
                            .map_err(RuntimeError::from)?;
                        self.next = (i + 1) % NODE_COUNT;
                        return Ok(true);
                    } else {
                        // No progress reported — try the next node.
                        tried += 1;
                        continue;
                    }
                }
                Err(e) => {
                    // Treat "no work" classes as a soft miss; try next node.
                    match e.kind {
                        NodeErrorKind::NoInput | NodeErrorKind::Backpressured => {
                            // Optional refinement:
                            // If you can map `i` → (IN, OUT) at compile time in the generated runtime,
                            // you can call `refresh_occupancies_for_node::<I, IN, OUT>(&mut self.occ)`
                            // here instead of refreshing all edges. Kept simple for now.
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

// ===== std test runtime: one worker thread per node =====
#[cfg(feature = "std")]
pub mod concurrent_runtime {
    use super::*;
    use crate::errors::{GraphError, NodeErrorKind, RuntimeError};
    use crate::graph::GraphApi;
    use crate::node::StepResult;
    use crate::queue::QueueOccupancy;
    use std::any::Any;
    use std::sync::mpsc;
    use std::thread;

    /// One worker per node; owns the node's bundle while running.
    pub struct TestStdRuntime<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
        stop: bool,
        next: usize,
        occ: [QueueOccupancy; EDGE_COUNT],
        workers: Vec<mpsc::Sender<WorkerCmd>>,
        handles: Vec<std::thread::JoinHandle<()>>,
        // simple flag to know if workers are currently running
        running: bool,
    }

    enum WorkerCmd {
        Step {
            reply: mpsc::Sender<Result<StepResult, crate::errors::NodeError>>,
        },
        ReturnBundle {
            reply: mpsc::Sender<Result<Box<dyn Any + Send>, ()>>,
        },
    }

    impl<const NODE_COUNT: usize, const EDGE_COUNT: usize> TestStdRuntime<NODE_COUNT, EDGE_COUNT> {
        pub fn new() -> Self {
            use crate::policy::WatermarkState;
            let init_occ = QueueOccupancy {
                items: 0,
                bytes: 0,
                watermark: WatermarkState::AtOrAboveHard,
            };
            Self {
                stop: false,
                next: 0,
                occ: [init_occ; EDGE_COUNT],
                workers: Vec::new(),
                handles: Vec::new(),
                running: false,
            }
        }

        #[inline]
        fn made_progress(sr: &StepResult) -> bool {
            let _ = sr;
            true
        }

        fn spawn_workers<Graph>(&mut self, graph: &mut Graph) -> Result<(), RuntimeError>
        where
            Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        {
            self.workers.clear();
            self.handles.clear();
            self.workers.reserve_exact(NODE_COUNT);
            self.handles.reserve_exact(NODE_COUNT);

            for index in 0..NODE_COUNT {
                // Move the bundle out
                let bundle = graph
                    .take_owned_bundle_by_index(index)
                    .map_err(RuntimeError::from)?;

                // Control channel for this worker
                let (tx, rx) = mpsc::channel::<WorkerCmd>();
                self.workers.push(tx);

                // Spawn thread; it owns the typed bundle
                let handle = thread::spawn(move || {
                    let mut bundle_local: Graph::OwnedBundle = bundle;
                    let mut telem = ();
                    let clk = ();

                    while let Ok(cmd) = rx.recv() {
                        match cmd {
                            WorkerCmd::Step { reply } => {
                                let res = Graph::step_owned_bundle::<(), ()>(
                                    &mut bundle_local,
                                    &clk,
                                    &mut telem,
                                );
                                let _ = reply.send(res);
                            }
                            WorkerCmd::ReturnBundle { reply } => {
                                let erased: Box<dyn Any + Send> = Box::new(bundle_local);
                                let _ = reply.send(Ok(erased));
                                break; // exit thread
                            }
                        }
                    }
                });

                self.handles.push(handle);
            }

            self.running = true;
            Ok(())
        }

        fn shutdown_workers<Graph>(&mut self, graph: &mut Graph) -> Result<(), RuntimeError>
        where
            Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        {
            if !self.running {
                return Ok(());
            }

            // Ask each worker to return its bundle and reattach it.
            for tx in self.workers.iter() {
                let (reply_tx, reply_rx) = mpsc::channel::<Result<Box<dyn Any + Send>, ()>>();
                if tx
                    .send(WorkerCmd::ReturnBundle { reply: reply_tx })
                    .is_err()
                {
                    // Channel closed: worker likely died. Treat as fatal for test runtime.
                    return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                }
                let erased = match reply_rx.recv() {
                    Ok(Ok(b)) => b,
                    _ => return Err(RuntimeError::from(GraphError::InvalidEdgeIndex)),
                };
                // Downcast back to the Graph's bundle type
                match erased.downcast::<Graph::OwnedBundle>() {
                    Ok(typed) => {
                        graph
                            .put_owned_bundle_by_index(*typed)
                            .map_err(RuntimeError::from)?;
                    }
                    Err(_) => return Err(RuntimeError::from(GraphError::InvalidEdgeIndex)),
                }
            }

            // Join threads
            for h in self.handles.drain(..) {
                let _ = h.join();
            }
            self.workers.clear();
            self.running = false;
            Ok(())
        }
    }

    impl<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        super::LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT>
        for TestStdRuntime<NODE_COUNT, EDGE_COUNT>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
    {
        type Clock = ();
        type Telemetry = ();
        type Error = RuntimeError;

        #[inline]
        fn init(
            &mut self,
            graph: &mut Graph, // <-- requires the trait change shown above
            _clock: Self::Clock,
            _telemetry: Self::Telemetry,
        ) -> Result<(), Self::Error> {
            // Validate + start workers + take first occupancy snapshot.
            graph.validate_graph().map_err(RuntimeError::from)?;
            self.spawn_workers(graph)?;
            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;
            self.stop = false;
            self.next = 0;
            Ok(())
        }

        #[inline]
        fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error> {
            // Do not touch worker lifecycle here; just refresh snapshot + pointer.
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
                // Put bundles back into the graph and stop.
                self.shutdown_workers(graph)?;
                return Ok(false);
            }

            if !self.running {
                // If a harness calls step() without calling init(), be defensive.
                self.spawn_workers(graph)?;
                graph
                    .write_all_edge_occupancies(&mut self.occ)
                    .map_err(RuntimeError::from)?;
            }

            // Round-robin try once over all nodes.
            let start = self.next;
            let mut tried = 0usize;

            while tried < NODE_COUNT {
                let i = (start + tried) % NODE_COUNT;

                let (reply_tx, reply_rx) =
                    mpsc::channel::<Result<StepResult, crate::errors::NodeError>>();
                if self.workers[i]
                    .send(WorkerCmd::Step { reply: reply_tx })
                    .is_err()
                {
                    return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                }

                match reply_rx.recv() {
                    Ok(Ok(sr)) => {
                        if Self::made_progress(&sr) {
                            graph
                                .write_all_edge_occupancies(&mut self.occ)
                                .map_err(RuntimeError::from)?;
                            self.next = (i + 1) % NODE_COUNT;
                            return Ok(true);
                        } else {
                            tried += 1;
                            continue;
                        }
                    }
                    Ok(Err(e)) => match e.kind {
                        NodeErrorKind::NoInput | NodeErrorKind::Backpressured => {
                            tried += 1;
                            continue;
                        }
                        _ => return Err(RuntimeError::from(e)),
                    },
                    Err(_recv_err) => {
                        return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                    }
                }
            }

            // No node made progress this round.
            Ok(false)
        }
    }
}
