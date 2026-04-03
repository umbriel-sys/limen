//! (Work)bench [test] Runtime implementation.

use crate::edge::EdgeOccupancy;
use crate::errors::{NodeErrorKind, RuntimeError, RuntimeInvariantError};
use crate::event_message;
use crate::graph::GraphApi;
use crate::node::StepResult;
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, NodePolicy, WatermarkState};
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
        const INIT_OCC: EdgeOccupancy = EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard);
        const INIT_POLICY: NodePolicy = NodePolicy::new(
            BatchingPolicy::none(),
            BudgetPolicy::new(None, None),
            DeadlinePolicy::new(false, None, None),
        );

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

    #[cfg(feature = "std")]
    type StopHandle = crate::runtime::RuntimeStopHandle;

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

/// ===== std test runtime: scheduler-driven concurrent execution =====
#[cfg(feature = "std")]
pub mod concurrent_runtime {
    use crate::edge::EdgeOccupancy;
    use crate::errors::{RuntimeError, RuntimeInvariantError};
    use crate::event_message;
    use crate::graph::{GraphApi, ScopedGraphApi};
    use crate::node::StepResult;
    use crate::policy::WatermarkState;
    use crate::prelude::{PlatformClock, Readiness, Telemetry};
    use crate::runtime::LimenRuntime;
    use crate::scheduling::{WorkerDecision, WorkerScheduler, WorkerState};
    use crate::types::NodeIndex;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // ------------------------------------------------------------------
    // SimpleBackoffScheduler
    // ------------------------------------------------------------------

    /// Simple backoff scheduler for test/bench use.
    ///
    /// - Steps immediately when ready or when there's no prior result.
    /// - Backs off on backpressure (longer wait).
    /// - Idles on no-input/waiting (shorter wait).
    /// - Stops when the shared `AtomicBool` is set.
    pub struct SimpleBackoffScheduler {
        stop: Arc<AtomicBool>,
        idle_micros: u64,
        backpressure_micros: u64,
    }

    impl SimpleBackoffScheduler {
        /// Create a new scheduler with the given stop flag and backoff durations.
        pub fn new(stop: Arc<AtomicBool>, idle_micros: u64, backpressure_micros: u64) -> Self {
            Self {
                stop,
                idle_micros,
                backpressure_micros,
            }
        }
    }

    impl WorkerScheduler for SimpleBackoffScheduler {
        fn decide(&self, state: &WorkerState) -> WorkerDecision {
            // Honor immediate stop.
            if self.stop.load(Ordering::Relaxed) {
                return WorkerDecision::Stop;
            }

            // If we have a last step result, handle the authoritative cases first.
            if let Some(last) = state.last_step {
                match last {
                    StepResult::Terminal => return WorkerDecision::Stop,
                    StepResult::Backpressured => {
                        return WorkerDecision::WaitMicros(self.backpressure_micros)
                    }
                    StepResult::MadeProgress => {}
                    // For NoInput / WaitingOnExternal / YieldUntil(_) we'll consult readiness below.
                    StepResult::NoInput
                    | StepResult::WaitingOnExternal
                    | StepResult::YieldUntil(_) => {}
                }
            }

            // Either last_step was None, or the last step did not make progress.
            // Use the node's computed readiness (set by the worker loop) to decide.
            match state.readiness {
                Readiness::Ready | Readiness::ReadyUnderPressure => WorkerDecision::Step,
                Readiness::NotReady => WorkerDecision::WaitMicros(self.idle_micros),
            }
        }
    }

    // ------------------------------------------------------------------
    // TestScopedRuntime
    // ------------------------------------------------------------------

    /// Concurrent runtime that delegates to `ScopedGraphApi::run_scoped`.
    ///
    /// Implements `LimenRuntime`. The runtime is the orchestrator:
    /// - `step()`: sequential round-robin via `GraphApi::step_node_by_index`.
    ///   For debug/test single-tick use.
    /// - `run()`: overrides default. Creates a `SimpleBackoffScheduler` and calls
    ///   `graph.run_scoped(clock, telemetry, scheduler)` — one scoped thread per
    ///   node, true concurrent execution, scheduler-controlled stepping.
    pub struct TestScopedRuntime<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
    where
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
    {
        stop: Arc<AtomicBool>,
        occ: [EdgeOccupancy; EDGE_COUNT],
        /// Per-node last step result, used by `step()` to make scheduler decisions.
        node_last_step: [Option<StepResult>; NODE_COUNT],
        clock: Option<C>,
        telemetry: Option<T>,
    }

    impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize>
        TestScopedRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
    {
        /// Construct with pessimistic initial occupancy; `init()` will overwrite.
        pub fn new() -> Self {
            const INIT_OCC: EdgeOccupancy = EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard);
            Self {
                stop: Arc::new(AtomicBool::new(false)),
                occ: [INIT_OCC; EDGE_COUNT],
                node_last_step: [None; NODE_COUNT],
                clock: None,
                telemetry: None,
            }
        }

        /// Internal helper for a monotonic nanosecond timestamp.
        #[inline]
        fn now_nanos(clock: &C) -> u64 {
            let ticks = clock.now_ticks();
            clock.ticks_to_nanos(ticks)
        }

        /// Safely access telemetry by mutable reference, if present.
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
        LimenRuntime<Graph, NODE_COUNT, EDGE_COUNT>
        for TestScopedRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        Graph: GraphApi<NODE_COUNT, EDGE_COUNT> + ScopedGraphApi<NODE_COUNT, EDGE_COUNT>,
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
    {
        type Clock = C;
        type Telemetry = T;
        type Error = RuntimeError;

        #[cfg(feature = "std")]
        type StopHandle = crate::runtime::RuntimeStopHandle;

        fn init(
            &mut self,
            graph: &mut Graph,
            clock: Self::Clock,
            mut telemetry: Self::Telemetry,
        ) -> Result<(), Self::Error> {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            graph.validate_graph().map_err(RuntimeError::from)?;

            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;

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
            self.stop.store(false, Ordering::Relaxed);
            self.node_last_step = [None; NODE_COUNT];

            Ok(())
        }

        fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error> {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            self.stop.store(false, Ordering::Relaxed);
            self.node_last_step = [None; NODE_COUNT];
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

        fn request_stop(&mut self) {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            self.stop.store(true, Ordering::Relaxed);

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

        #[cfg(feature = "std")]
        fn stop_handle(&self) -> Option<Self::StopHandle> {
            Some(crate::runtime::RuntimeStopHandle::new(self.stop.clone()))
        }

        #[inline]
        fn is_stopping(&self) -> bool {
            self.stop.load(Ordering::Relaxed)
        }

        #[inline]
        fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT] {
            &self.occ
        }

        /// Sequential scheduler-driven step for debug/test single-tick use.
        ///
        /// For each node, consults `SimpleBackoffScheduler` using the stored
        /// per-node `last_step` to decide whether to step. This ensures the
        /// same scheduling policy governs both `step()` and `run()`.
        ///
        /// `WaitMicros` decisions are treated as "skip this tick" — the
        /// `last_step` is then cleared so the scheduler returns `Step` on
        /// the next `step()` call. This avoids permanent starvation in
        /// sequential mode where there is no real time passage between ticks.
        fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error> {
            if <TestScopedRuntime<C, T, NODE_COUNT, EDGE_COUNT> as LimenRuntime<
                Graph,
                NODE_COUNT,
                EDGE_COUNT,
            >>::is_stopping(self)
            {
                return Ok(false);
            }

            let clock = match self.clock.take() {
                Some(c) => c,
                None => {
                    return Err(RuntimeError::RuntimeInvariant(
                        RuntimeInvariantError::UninitializedClock,
                    ))
                }
            };
            let mut telemetry = match self.telemetry.take() {
                Some(t) => t,
                None => {
                    self.clock = Some(clock);
                    return Err(RuntimeError::RuntimeInvariant(
                        RuntimeInvariantError::UninitializedTelemetry,
                    ));
                }
            };

            let scheduler = SimpleBackoffScheduler::new(self.stop.clone(), 50, 200);
            let mut any_progress = false;

            // Refresh a cheap occupancy snapshot for this tick so we can compute
            // inexpensive readiness for each node (same idea as run_scoped workers).
            graph
                .write_all_edge_occupancies(&mut self.occ)
                .map_err(RuntimeError::from)?;

            for i in 0..NODE_COUNT {
                let mut state = WorkerState::new(i, NODE_COUNT, clock.now_ticks());
                state.last_step = self.node_last_step[i];

                // Compute max output backpressure and whether any input has items
                // by scanning the graph's edge descriptors and the occupancy
                // snapshot `self.occ`.
                let mut _max_wm = WatermarkState::BelowSoft;
                let mut any_input_has_items = false;
                let node_idx = NodeIndex::from(i);

                for ed in graph.get_edge_descriptors().iter() {
                    let eid = *ed.id().as_usize();
                    // Upstream edges contribute to output backpressure.
                    if ed.upstream().node() == &node_idx {
                        let occ = &self.occ[eid];
                        if *occ.watermark() > _max_wm {
                            _max_wm = *occ.watermark();
                        }
                    }
                    // Downstream edges determine whether this node has any input items.
                    if ed.downstream().node() == &node_idx {
                        let occ = &self.occ[eid];
                        if *occ.items() > 0 {
                            any_input_has_items = true;
                        }
                    }
                }
                state.backpressure = _max_wm;

                // Compatibility: if we have no prior step (first probe) let the
                // scheduler see Ready so the node gets probed at least once.
                if state.last_step.is_none() {
                    state.readiness = Readiness::Ready;
                } else {
                    // Otherwise use the cheap pre-check: if there are items on any
                    // input edge (ingress included) we are Ready; otherwise NotReady.
                    state.readiness = if any_input_has_items {
                        if _max_wm >= WatermarkState::BetweenSoftAndHard {
                            Readiness::ReadyUnderPressure
                        } else {
                            Readiness::Ready
                        }
                    } else {
                        Readiness::NotReady
                    };
                }

                // Ask the scheduler and log the decision for debugging.
                let decision = scheduler.decide(&state);
                // ::std::eprintln!(
                //     "sched-debug: node={} last_step={:?} readiness={:?} => decision={:?}",
                //     i,
                //     state.last_step,
                //     state.readiness,
                //     decision
                // );

                match decision {
                    WorkerDecision::Step => {
                        match graph.step_node_by_index(i, &clock, &mut telemetry) {
                            Ok(sr) => {
                                // Record last step result and note progress.
                                self.node_last_step[i] = Some(sr);
                                if matches!(sr, StepResult::MadeProgress | StepResult::Terminal) {
                                    any_progress = true;
                                }

                                // **Crucial:** update occupancy snapshot immediately so
                                // downstream nodes in this same step() iteration see the
                                // newly-produced items and compute readiness correctly.
                                graph
                                    .write_all_edge_occupancies(&mut self.occ)
                                    .map_err(RuntimeError::from)?;
                            }
                            Err(e) => {
                                ::std::eprintln!("sched-debug: node={} step error: {:?}", i, e);
                                // NodeError is not fatal; clear state so we retry.
                                self.node_last_step[i] = None;
                            }
                        }
                    }
                    WorkerDecision::WaitMicros(_) => {
                        // In sequential mode, skip this node for this tick.
                        // Clear last_step so the scheduler returns Step next tick,
                        // preventing permanent starvation.
                        self.node_last_step[i] = None;
                    }
                    WorkerDecision::Stop => {
                        // Node terminated or runtime stopping; do not step.
                    }
                }
            }

            // No final write_all_edge_occupancies here: we update occupancies
            // right after each successful step above, so self.occ is up-to-date.

            self.telemetry = Some(telemetry);
            self.clock = Some(clock);

            Ok(any_progress)
        }

        /// Concurrent execution via `ScopedGraphApi::run_scoped`.
        ///
        /// Creates a `SimpleBackoffScheduler` and delegates to the graph's
        /// scoped thread infrastructure. Each node gets its own thread;
        /// the scheduler controls per-worker stepping.
        ///
        /// After all threads join, refreshes the occupancy snapshot.
        fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error> {
            const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

            let clock = self.clock.clone().ok_or(RuntimeError::RuntimeInvariant(
                RuntimeInvariantError::UninitializedClock,
            ))?;
            let telemetry = self
                .telemetry
                .clone()
                .ok_or(RuntimeError::RuntimeInvariant(
                    RuntimeInvariantError::UninitializedTelemetry,
                ))?;

            let scheduler = SimpleBackoffScheduler::new(
                self.stop.clone(),
                50,  // idle_micros
                200, // backpressure_micros
            );

            graph.run_scoped(clock, telemetry, scheduler);

            // After scope exits (all threads joined): refresh occupancies.
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
                            crate::telemetry::RuntimeTelemetryEventKind::GraphStopped,
                            None,
                        ),
                    );
                    telemetry.push_event(event);
                }
            }

            Ok(())
        }
    }

    impl<C, T, const NODE_COUNT: usize, const EDGE_COUNT: usize> Default
        for TestScopedRuntime<C, T, NODE_COUNT, EDGE_COUNT>
    where
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
    {
        #[inline]
        fn default() -> Self {
            Self::new()
        }
    }
}
