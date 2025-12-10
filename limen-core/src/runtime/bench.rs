//! (Work)bench [test] Runtime implementation.

use crate::edge::EdgeOccupancy;
use crate::errors::{NodeErrorKind, RuntimeError, RuntimeInvariantError};
use crate::event_message;
use crate::graph::GraphApi;
use crate::node::StepResult;
use crate::policy::{BudgetPolicy, DeadlinePolicy, NodePolicy, WatermarkState};
use crate::prelude::{PlatformClock, Telemetry};

use super::LimenRuntime;

/// A tiny, no_std test runtime:
/// - round-robin over nodes
/// - uses a single occupancy array
/// - no heap, no threads, no timers
pub struct TestNoStdRuntime<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    stop: bool,
    next: usize,
    occ: [EdgeOccupancy; EDGE_COUNT],
    node_policies: [NodePolicy; NODE_COUNT],
    clock: Option<C>,
    telemetry: Option<T>,
}

impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    TestNoStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
where
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Construct with a pessimistic initial occupancy; `init()` will overwrite it.
    pub const fn new() -> Self {
        const INIT_OCC: EdgeOccupancy = EdgeOccupancy {
            items: 0,
            bytes: 0,
            // Any value is fine; init() will replace the whole array.
            watermark: WatermarkState::AtOrAboveHard,
        };
        const INIT_POLICY: NodePolicy = NodePolicy {
            batching: crate::policy::BatchingPolicy {
                fixed_n: Some(1),
                max_delta_t: None,
            },
            budget: BudgetPolicy {
                tick_budget: None,
                watchdog_ticks: None,
            },
            deadline: DeadlinePolicy {
                require_absolute_deadline: false,
                slack_tolerance_ns: None,
                default_deadline_ns: None,
            },
        };

        Self {
            stop: false,
            next: 0,
            occ: [INIT_OCC; EDGE_COUNT],
            node_policies: [INIT_POLICY; NODE_COUNT],
            clock: None,
            telemetry: None,
        }
    }

    /// Decide whether a node's `StepResult` constitutes "progress".
    /// Currently conservative: treat any `Ok(_)` as progress to keep the runtime simple.
    /// If/when `StepResult` exposes a richer API (e.g., `is_progress()`), update this.
    #[inline]
    fn made_progress(sr: &StepResult) -> bool {
        match sr {
            StepResult::MadeProgress => true,
            StepResult::Terminal => true,
            // TODO: Handle this.
            StepResult::YieldUntil(_) => true,
            StepResult::NoInput | StepResult::Backpressured | StepResult::WaitingOnExternal => {
                false
            }
        }
    }

    /// Internal helper for a monotonic nanosecond timestamp.
    #[inline]
    fn now_nanos(clock: &C) -> u64 {
        let ticks = clock.now_ticks();
        clock.ticks_to_nanos(ticks)
    }

    /// Hot-path inner step: requires `&mut self`, `&C`, and `&mut T`.
    #[inline]
    fn step_inner<Graph>(
        &mut self,
        graph: &mut Graph,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<bool, RuntimeError>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
    {
        // Try each node once, starting from `self.next` (round-robin).
        let start = self.next;
        let mut tried = 0usize;

        while tried < NODE_COUNT {
            let node_index = (start + tried) % NODE_COUNT;

            // Execute the node step. All node-level telemetry (latency, processed,
            // deadline, NodeStep events) is now handled in NodeLink::step via
            // the graph implementation, not here.
            let result = graph.step_node_by_index(node_index, clock, telemetry);

            // ---- Scheduler logic (unchanged) ----
            match result {
                Ok(step_result) => {
                    if Self::made_progress(&step_result) {
                        // Keep this as the canonical place where the runtime refreshes the
                        // occupancy buffer for scheduling. This remains separate from
                        // telemetry, which is handled in StepContext/NodeLink.
                        graph
                            .write_all_edge_occupancies(&mut self.occ)
                            .map_err(RuntimeError::from)?;

                        self.next = (node_index + 1) % NODE_COUNT;
                        return Ok(true);
                    } else {
                        tried += 1;
                        continue;
                    }
                }
                Err(error) => match error.kind {
                    NodeErrorKind::NoInput | NodeErrorKind::Backpressured => {
                        tried += 1;
                        continue;
                    }
                    _ => return Err(RuntimeError::from(error)),
                },
            }
        }

        // We tried all nodes and none made progress.
        Ok(false)
    }

    /// Safely access telemetry by mutable reference, if it is present.
    ///
    /// Returns:
    /// - `Ok(Some(r))` if telemetry is present and `f` ran.
    /// - `Ok(None)` if telemetry is currently absent (e.g. not yet initialized).
    ///
    /// This never panics and never moves telemetry out of the runtime.
    #[inline]
    pub fn with_telemetry<F, R>(&mut self, f: F) -> Result<Option<R>, RuntimeError>
    where
        F: FnOnce(&mut T) -> R,
    {
        if let Some(t) = self.telemetry.as_mut() {
            let r = f(t);
            Ok(Some(r))
        } else {
            Ok(None)
        }
    }
}

impl<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT> for TestNoStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    type Clock = C;
    type Telemetry = T;
    type Error = RuntimeError;

    #[inline]
    fn init(
        &mut self,
        graph: &mut Graph,
        clock: Self::Clock,
        mut telemetry: Self::Telemetry,
    ) -> Result<(), Self::Error> {
        const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

        // Validate (pure, read-only).
        graph.validate_graph().map_err(RuntimeError::from)?;

        // Snapshot occupancies into our persistent buffer.
        graph
            .write_all_edge_occupancies(&mut self.occ)
            .map_err(RuntimeError::from)?;

        // Cache NodePolicy for every node using the GraphApi hook.
        self.node_policies = graph.get_node_policies();

        if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
            let timestamp_ns = Self::now_nanos(&clock);
            let event = crate::telemetry::TelemetryEvent::Runtime(
                crate::telemetry::RuntimeTelemetryEvent {
                    graph_id: GRAPH_ID,
                    timestamp_ns,
                    event_kind: crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                    message: None,
                },
            );
            telemetry.push_event(event);
        }

        self.clock = Some(clock);
        self.telemetry = Some(telemetry);

        self.stop = false;
        self.next = 0;

        Ok(())
    }

    #[inline]
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error> {
        const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

        self.stop = false;
        self.next = 0;
        graph
            .write_all_edge_occupancies(&mut self.occ)
            .map_err(RuntimeError::from)?;

        if let (Some(ref clock), Some(telemetry)) = (&self.clock, self.telemetry.as_mut()) {
            if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
                let timestamp_ns = Self::now_nanos(clock);
                let event = crate::telemetry::TelemetryEvent::Runtime(
                    crate::telemetry::RuntimeTelemetryEvent {
                        graph_id: GRAPH_ID,
                        timestamp_ns,
                        event_kind: crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                        message: Some(event_message!("graph reset")),
                    },
                );
                telemetry.push_event(event);
            }
        }

        Ok(())
    }

    #[inline]
    fn request_stop(&mut self) {
        self.stop = true;

        const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

        if let (Some(ref clock), Some(telemetry)) = (&self.clock, self.telemetry.as_mut()) {
            if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
                let timestamp_ns = Self::now_nanos(clock);
                let event = crate::telemetry::TelemetryEvent::Runtime(
                    crate::telemetry::RuntimeTelemetryEvent {
                        graph_id: GRAPH_ID,
                        timestamp_ns,
                        event_kind: crate::telemetry::RuntimeTelemetryEventKind::GraphStopped,
                        message: None,
                    },
                );
                telemetry.push_event(event);
            }
        }
    }

    #[inline]
    fn is_stopping(&self) -> bool {
        self.stop
    }

    #[inline]
    fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT] {
        &self.occ
    }

    #[inline]
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error> {
        if self.stop {
            return Ok(false);
        }

        // Safely take the clock; error if missing.
        let clock = match self.clock.take() {
            Some(c) => c,
            None => {
                return Err(RuntimeError::RuntimeInvariant(
                    RuntimeInvariantError::UninitializedClock,
                ))
            }
        };

        // Safely take telemetry; if missing, put clock back before returning.
        let mut telemetry = match self.telemetry.take() {
            Some(t) => t,
            None => {
                self.clock = Some(clock);
                return Err(RuntimeError::RuntimeInvariant(
                    RuntimeInvariantError::UninitializedTelemetry,
                ));
            }
        };

        let result = self.step_inner(graph, &clock, &mut telemetry);

        // Put telemetry and clock back.
        self.telemetry = Some(telemetry);
        self.clock = Some(clock);

        result
    }
}

impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize> Default
    for TestNoStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
where
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// ===== std test runtime: one worker thread per node =====
#[cfg(feature = "std")]
pub mod concurrent_runtime {
    use crate::edge::EdgeOccupancy;
    use crate::errors::{GraphError, NodeErrorKind, RuntimeError};
    use crate::graph::GraphApi;
    use crate::node::StepResult;
    use crate::prelude::{PlatformClock, Telemetry};
    use std::any::Any;
    use std::sync::mpsc;
    use std::thread;

    /// One worker per node; owns the node's bundle while running.
    pub struct TestStdRuntime<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    where
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        stop: bool,
        next: usize,
        occ: [EdgeOccupancy; EDGE_COUNT],
        workers: Vec<mpsc::Sender<WorkerCmd>>,
        handles: Vec<std::thread::JoinHandle<()>>,
        // simple flag to know if workers are currently running
        running: bool,
        clock: Option<C>,
        telemetry: Option<T>,
    }

    enum WorkerCmd {
        Step {
            reply: mpsc::Sender<Result<StepResult, crate::errors::NodeError>>,
        },
        ReturnBundle {
            reply: mpsc::Sender<Result<Box<dyn Any + Send>, ()>>,
        },
    }

    impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        TestStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        /// Construct with a pessimistic initial occupancy; `init()` will overwrite it.
        pub fn new() -> Self {
            use crate::policy::WatermarkState;
            let init_occ = EdgeOccupancy {
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
                clock: None,
                telemetry: None,
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

            // Get templates to clone per worker
            let clk_template = self
                .clock
                .as_ref()
                .expect("TestStdRuntime used before init()")
                .clone();
            let tel_template = self
                .telemetry
                .as_ref()
                .expect("TestStdRuntime used before init()")
                .clone();

            for index in 0..NODE_COUNT {
                // Move the bundle out
                let bundle = graph
                    .take_owned_bundle_by_index(index)
                    .map_err(RuntimeError::from)?;

                // Control channel for this worker
                let (tx, rx) = mpsc::channel::<WorkerCmd>();
                self.workers.push(tx);

                let clk_local = clk_template.clone();
                let telem_local = tel_template.clone();

                // Spawn thread; it owns the typed bundle
                let handle = thread::spawn(move || {
                    let mut bundle_local: Graph::OwnedBundle = bundle;
                    let mut telem: T = telem_local;
                    let clk: C = clk_local;

                    while let Ok(cmd) = rx.recv() {
                        match cmd {
                            WorkerCmd::Step { reply } => {
                                let res = Graph::step_owned_bundle::<C, T>(
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

    impl<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        super::LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT>
        for TestStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        type Clock = C;
        type Telemetry = T;
        type Error = RuntimeError;

        #[inline]
        fn init(
            &mut self,
            graph: &mut Graph, // <-- requires the trait change shown above
            clock: Self::Clock,
            telemetry: Self::Telemetry,
        ) -> Result<(), Self::Error> {
            // Validate + start workers + take first occupancy snapshot.
            graph.validate_graph().map_err(RuntimeError::from)?;
            self.clock = Some(clock);
            self.telemetry = Some(telemetry);
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
        fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT] {
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

    impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize> Default
        for TestStdRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        #[inline]
        fn default() -> Self {
            Self::new()
        }
    }
}
