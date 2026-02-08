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
                Err(error) => match error.kind() {
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
            let event = crate::telemetry::TelemetryEvent::runtime(
                crate::telemetry::RuntimeTelemetryEvent::new(
                    GRAPH_ID,
                    timestamp_ns,
                    crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                    None,
                ),
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
                let event = crate::telemetry::TelemetryEvent::runtime(
                    crate::telemetry::RuntimeTelemetryEvent::new(
                        GRAPH_ID,
                        timestamp_ns,
                        crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                        Some(event_message!("graph reset")),
                    ),
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
                let event = crate::telemetry::TelemetryEvent::runtime(
                    crate::telemetry::RuntimeTelemetryEvent::new(
                        GRAPH_ID,
                        timestamp_ns,
                        crate::telemetry::RuntimeTelemetryEventKind::GraphStopped,
                        None,
                    ),
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

/// ===== std test runtime: one worker thread per node (concurrent) =====
#[cfg(feature = "std")]
pub mod concurrent_runtime {
    use crate::edge::EdgeOccupancy;
    use crate::errors::{GraphError, NodeError, RuntimeError};
    use crate::event_message;
    use crate::graph::GraphApi;
    use crate::node::StepResult;
    use crate::policy::WatermarkState;
    use crate::prelude::{PlatformClock, Telemetry};

    use std::sync::mpsc::{self, TryRecvError};
    use std::thread;
    use std::time::Duration;

    /// Commands sent to each worker.
    ///
    /// `Bundle` is the concrete `Graph::OwnedBundle` type for this graph.
    enum WorkerCmd<Bundle> {
        /// Execute exactly one cooperative step and reply with the result.
        StepOnce {
            reply: mpsc::Sender<Result<StepResult, NodeError>>,
        },
        /// Switch this worker into continuous-stepping mode.
        StartContinuous,
        /// Return the owned bundle back to the main thread and exit.
        ReturnBundle { reply: mpsc::Sender<Bundle> },
    }

    /// One worker thread per node; owns the node's bundle while running.
    ///
    /// NOTE: this runtime is generic over `Graph` so that we can hold
    /// `Graph::OwnedBundle` directly and avoid trait objects.
    pub struct TestStdRuntime<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        stop: bool,
        occ: [EdgeOccupancy; EDGE_COUNT],

        workers: Vec<mpsc::Sender<WorkerCmd<Graph::OwnedBundle>>>,
        handles: Vec<std::thread::JoinHandle<()>>,
        /// `true` once workers are spawned and own their bundles.
        workers_running: bool,
        /// `true` once we've told workers to run continuously (used by `run()`).
        continuous_mode: bool,

        clock: Option<C>,
        telemetry: Option<T>,
    }

    impl<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        TestStdRuntime<Graph, C, T, NODE_COUNT, EDGE_COUNT>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        /// Construct with a pessimistic initial occupancy; `init()` will overwrite it.
        pub fn new() -> Self {
            const INIT_OCC: EdgeOccupancy = EdgeOccupancy {
                items: 0,
                bytes: 0,
                // Any value is fine; init()/step() will replace the whole array.
                watermark: WatermarkState::AtOrAboveHard,
            };

            Self {
                stop: false,
                occ: [INIT_OCC; EDGE_COUNT],

                workers: Vec::new(),
                handles: Vec::new(),
                workers_running: false,
                continuous_mode: false,

                clock: None,
                telemetry: None,
            }
        }

        /// Decide whether a `StepResult` constitutes "progress".
        ///
        /// Only used in `step()` to compute the boolean return.
        #[inline]
        fn made_progress(sr: &StepResult) -> bool {
            match sr {
                StepResult::MadeProgress => true,
                StepResult::Terminal => true, // node finished = progress
                // TODO: refine once YieldUntil semantics are better specified.
                StepResult::YieldUntil(_) => true,
                StepResult::NoInput | StepResult::Backpressured | StepResult::WaitingOnExternal => {
                    false
                }
            }
        }

        /// Internal helper for a monotonic nanosecond timestamp (if you want telemetry).
        #[inline]
        fn now_nanos(clock: &C) -> u64 {
            let ticks = clock.now_ticks();
            clock.ticks_to_nanos(ticks)
        }

        /// Safely access telemetry by mutable reference, if it is present.
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

        /// Spawn one worker thread per node, moving each node bundle out of the graph.
        ///
        /// Workers start in *manual* mode: they only step when they receive `StepOnce`.
        fn spawn_workers(&mut self, graph: &mut Graph) -> Result<(), RuntimeError> {
            self.workers.clear();
            self.handles.clear();
            self.workers.reserve_exact(NODE_COUNT);
            self.handles.reserve_exact(NODE_COUNT);

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
                // Move the bundle out of the graph exactly once.
                let bundle = graph
                    .take_owned_bundle_by_index(index)
                    .map_err(RuntimeError::from)?;

                // Control channel for this worker.
                let (tx, rx) = mpsc::channel::<
                    WorkerCmd<<Graph as GraphApi<NODE_COUNT, EDGE_COUNT>>::OwnedBundle>,
                >();
                self.workers.push(tx);

                let clk_local = clk_template.clone();
                let telem_local = tel_template.clone();

                // Spawn worker thread owning the typed bundle.
                let handle = thread::spawn(move || {
                    let mut bundle_local: Graph::OwnedBundle = bundle;
                    let mut telem: T = telem_local;
                    let clk: C = clk_local;

                    // Per-node backoff parameters.
                    let idle_sleep = Duration::from_micros(50);
                    let backpressure_sleep = Duration::from_micros(200);

                    // Two modes:
                    // - Manual: block on commands; only step on `StepOnce`.
                    // - Continuous: keep stepping locally with backoff, independent
                    //   of other nodes, until a ReturnBundle arrives.
                    let mut continuous = false;
                    // Per-node "fatal" flag: once we see Terminal, we never call
                    // `step_owned_bundle` again for this node.
                    let mut terminated = false;

                    loop {
                        if !continuous {
                            // ========== MANUAL MODE ==========
                            match rx.recv() {
                                Ok(WorkerCmd::StepOnce { reply }) => {
                                    let res = if terminated {
                                        // The only fatal step result is Terminal:
                                        // once seen, we stop stepping this node.
                                        Ok(StepResult::Terminal)
                                    } else {
                                        Graph::step_owned_bundle::<C, T>(
                                            &mut bundle_local,
                                            &clk,
                                            &mut telem,
                                        )
                                    };

                                    if let Ok(StepResult::Terminal) = res {
                                        terminated = true;
                                    }

                                    // Per requirements: NodeError is NEVER fatal to
                                    // the runtime; it's just reported back.
                                    let _ = reply.send(res);
                                }
                                Ok(WorkerCmd::StartContinuous) => {
                                    continuous = true;
                                }
                                Ok(WorkerCmd::ReturnBundle { reply }) => {
                                    let _ = reply.send(bundle_local);
                                    break;
                                }
                                Err(_) => {
                                    // Runtime dropped the channel; exit worker.
                                    break;
                                }
                            }
                        } else {
                            // ========== CONTINUOUS MODE ==========
                            // First, handle any high-priority control messages
                            // non-blockingly (especially ReturnBundle).
                            match rx.try_recv() {
                                Ok(WorkerCmd::ReturnBundle { reply }) => {
                                    let _ = reply.send(bundle_local);
                                    break;
                                }
                                Ok(WorkerCmd::StartContinuous) => {
                                    // Already continuous; ignore.
                                }
                                Ok(WorkerCmd::StepOnce { reply }) => {
                                    // Rare: debug/manual step while in continuous mode / switch back to step mode.
                                    continuous = false;

                                    let res = if terminated {
                                        Ok(StepResult::Terminal)
                                    } else {
                                        Graph::step_owned_bundle::<C, T>(
                                            &mut bundle_local,
                                            &clk,
                                            &mut telem,
                                        )
                                    };

                                    if let Ok(StepResult::Terminal) = res {
                                        terminated = true;
                                    }
                                    let _ = reply.send(res);
                                }
                                Err(TryRecvError::Empty) => {
                                    // No control message pending.
                                }
                                Err(TryRecvError::Disconnected) => {
                                    break;
                                }
                            }

                            if terminated {
                                // Node is finished; do not step it again.
                                // Just idle and wait for ReturnBundle.
                                thread::sleep(idle_sleep);
                                continue;
                            }

                            // One cooperative step.
                            let res = Graph::step_owned_bundle::<C, T>(
                                &mut bundle_local,
                                &clk,
                                &mut telem,
                            );

                            match res {
                                Ok(sr) => match sr {
                                    StepResult::MadeProgress => {
                                        // Immediately loop and step again.
                                        continue;
                                    }
                                    StepResult::Terminal => {
                                        // The ONLY fatal step result: stop stepping this node.
                                        terminated = true;
                                        continue;
                                    }
                                    StepResult::Backpressured => {
                                        // Outputs are full / backpressured: per-node backoff.
                                        thread::sleep(backpressure_sleep);
                                    }
                                    StepResult::NoInput
                                    | StepResult::WaitingOnExternal
                                    | StepResult::YieldUntil(_) => {
                                        // Nothing useful to do right now: short idle sleep.
                                        thread::sleep(idle_sleep);
                                    }
                                },
                                Err(_e) => {
                                    // Per requirements: NodeError is not fatal.
                                    // Treat as "no progress" with idle sleep.
                                    thread::sleep(idle_sleep);
                                }
                            }
                        }
                    }
                });

                self.handles.push(handle);
            }

            self.workers_running = true;
            Ok(())
        }

        /// Request bundles back from workers, reattach into the graph, and join threads.
        fn shutdown_workers(&mut self, graph: &mut Graph) -> Result<(), RuntimeError> {
            if !self.workers_running {
                return Ok(());
            }

            for tx in self.workers.iter() {
                let (reply_tx, reply_rx) =
                    mpsc::channel::<<Graph as GraphApi<NODE_COUNT, EDGE_COUNT>>::OwnedBundle>();

                if tx
                    .send(WorkerCmd::ReturnBundle { reply: reply_tx })
                    .is_err()
                {
                    // Channel closed: worker likely died; treat as fatal for this test runtime.
                    return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                }

                let bundle = match reply_rx.recv() {
                    Ok(b) => b,
                    Err(_) => return Err(RuntimeError::from(GraphError::InvalidEdgeIndex)),
                };

                graph
                    .put_owned_bundle_by_index(bundle)
                    .map_err(RuntimeError::from)?;
            }

            // Join threads.
            for handle in self.handles.drain(..) {
                let _ = handle.join();
            }
            self.workers.clear();
            self.workers_running = false;
            self.continuous_mode = false;

            Ok(())
        }
    }

    impl<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        super::LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT>
        for TestStdRuntime<Graph, C, T, NODE_COUNT, EDGE_COUNT>
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
            graph: &mut Graph,
            clock: Self::Clock,
            telemetry: Self::Telemetry,
        ) -> Result<(), Self::Error> {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            // Validate graph topology first (pure, read-only).
            graph.validate_graph().map_err(RuntimeError::from)?;

            self.clock = Some(clock);
            self.telemetry = Some(telemetry);

            // Move each node bundle into its dedicated worker thread (manual mode).
            self.spawn_workers(graph)?;

            // Initial occupancy snapshot.
            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;

            if let (Some(ref clock), Some(telemetry)) = (&self.clock, self.telemetry.as_mut()) {
                if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
                    let timestamp_ns = Self::now_nanos(clock);
                    let event = crate::telemetry::TelemetryEvent::runtime(
                        crate::telemetry::RuntimeTelemetryEvent::new(
                            GRAPH_ID,
                            timestamp_ns,
                            crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                            None,
                        ),
                    );
                    telemetry.push_event(event);
                }
            }

            self.stop = false;
            self.continuous_mode = false;

            Ok(())
        }

        #[inline]
        fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error> {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            // Do not touch worker lifecycle here; just refresh snapshot + stop flag.
            self.stop = false;
            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;

            if let (Some(ref clock), Some(telemetry)) = (&self.clock, self.telemetry.as_mut()) {
                if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
                    let timestamp_ns = Self::now_nanos(clock);
                    let event = crate::telemetry::TelemetryEvent::runtime(
                        crate::telemetry::RuntimeTelemetryEvent::new(
                            GRAPH_ID,
                            timestamp_ns,
                            crate::telemetry::RuntimeTelemetryEventKind::GraphStarted,
                            Some(event_message!("graph reset")),
                        ),
                    );
                    telemetry.push_event(event);
                }
            }

            Ok(())
        }

        #[inline]
        fn request_stop(&mut self) {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            self.stop = true;

            if let (Some(ref clock), Some(telemetry)) = (&self.clock, self.telemetry.as_mut()) {
                if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
                    let timestamp_ns = Self::now_nanos(clock);
                    let event = crate::telemetry::TelemetryEvent::runtime(
                        crate::telemetry::RuntimeTelemetryEvent::new(
                            GRAPH_ID,
                            timestamp_ns,
                            crate::telemetry::RuntimeTelemetryEventKind::GraphStopped,
                            None,
                        ),
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

        /// One scheduler tick in manual mode:
        ///
        /// - Send `StepOnce` to all workers (nodes).
        /// - Wait for all results.
        /// - Refresh occupancy snapshot once.
        /// - Return `true` if at least one node made progress on this tick.
        ///
        /// IMPORTANT:
        /// - `StepResult::Terminal` is treated as progress and *only* stops stepping
        ///   that node (inside the worker); it is not fatal to the runtime.
        /// - `NodeError` is *never* fatal here; it is treated as "no progress".
        #[inline]
        fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error> {
            // If we've been asked to stop, reattach bundles and stop stepping.
            if self.stop {
                if self.workers_running {
                    self.shutdown_workers(graph)?;
                }
                return Ok(false);
            }

            // Defensive: if a harness calls `step()` without `init()`, start workers.
            if !self.workers_running {
                self.spawn_workers(graph)?;
                graph
                    .write_all_edge_occupancies(&mut self.occ)
                    .map_err(RuntimeError::from)?;
            }

            let (reply_tx, reply_rx) = mpsc::channel::<Result<StepResult, NodeError>>();

            // Tell every worker to step once.
            for tx in self.workers.iter() {
                if tx
                    .send(WorkerCmd::StepOnce {
                        reply: reply_tx.clone(),
                    })
                    .is_err()
                {
                    return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                }
            }
            drop(reply_tx);

            let mut any_progress = false;

            // Collect one result per worker.
            for _ in 0..self.workers.len() {
                match reply_rx.recv() {
                    Ok(Ok(sr)) => {
                        if Self::made_progress(&sr) {
                            any_progress = true;
                        }
                    }
                    Ok(Err(_e)) => {
                        // Per requirements: NodeError is not fatal; we treat it as
                        // "no progress" for this node on this tick.
                        // You can optionally push telemetry here.
                    }
                    Err(_) => {
                        return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                    }
                }
            }

            // After the global concurrent tick, refresh the occupancy snapshot once.
            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;

            Ok(any_progress)
        }

        /// `run` mode: nodes step independently in their own threads.
        ///
        /// - Switch all workers into continuous-stepping mode.
        /// - Do *not* globally sleep based on any single edge's watermark.
        /// - Each worker decides when to back off based on its own StepResult:
        ///   - `Backpressured` → longer per-node sleep.
        ///   - `NoInput`/`WaitingOnExternal`/`YieldUntil` → shorter per-node sleep.
        /// - The runtime just waits until `request_stop()` is called, then
        ///   reattaches bundles and returns.
        #[inline]
        fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error> {
            // Ensure workers exist.
            if !self.workers_running {
                self.spawn_workers(graph)?;
                graph
                    .write_all_edge_occupancies(&mut self.occ)
                    .map_err(RuntimeError::from)?;
            }

            // Switch all workers into continuous mode exactly once.
            if !self.continuous_mode {
                for tx in self.workers.iter() {
                    if tx.send(WorkerCmd::StartContinuous).is_err() {
                        return Err(RuntimeError::from(GraphError::InvalidEdgeIndex));
                    }
                }
                self.continuous_mode = true;
            }

            // The runtime itself does not schedule steps anymore:
            // workers run independently. We can optionally refresh
            // occupancy periodically for telemetry, but we do NOT gate
            // other nodes based on a single edge's watermark.
            let poll_sleep = Duration::from_millis(1);

            while !self.is_stopping() {
                // Optional: refresh occupancies at a low rate for observability.
                graph
                    .write_all_edge_occupancies(&mut self.occ)
                    .map_err(RuntimeError::from)?;

                thread::sleep(poll_sleep);
            }

            // Stop requested: get bundles back & join threads.
            if self.workers_running {
                self.shutdown_workers(graph)?;
            }

            Ok(())
        }
    }

    impl<Graph, C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize> Default
        for TestStdRuntime<Graph, C, T, NODE_COUNT, EDGE_COUNT>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + 'static,
        C: PlatformClock + Sized + Clone + Send + 'static,
        T: Telemetry + Sized + Clone + Send + 'static,
    {
        #[inline]
        fn default() -> Self {
            Self::new()
        }
    }
}
