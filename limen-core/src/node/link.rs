//! Node graph-link descriptor types.

use crate::{
    edge::Edge,
    errors::{NodeError, NodeErrorKind},
    memory::PlacementAcceptance,
    message::{payload::Payload, Message},
    node::{Node, NodeCapabilities, NodeKind, StepContext, StepResult},
    policy::NodePolicy,
    prelude::{
        NodeStepError, NodeStepTelemetry, PlatformClock, Telemetry, TelemetryEvent, TelemetryKey,
        TelemetryKind,
    },
    types::{NodeIndex, PortId, PortIndex},
};

/// A lightweight descriptor that **links to** a concrete node instance and records its
/// static topology and policy metadata.
///
/// Unlike a pure descriptor, `NodeLink` **owns** the concrete node instance (`N`)
/// and records its identity, kind, port counts, policy, and optional name for graph
/// construction, scheduling, diagnostics, and tooling. It exposes `&N` and `&mut N`
/// accessors so runtimes can operate on the live node.
///
/// # Type Parameters
/// - `'a`: Lifetime of the borrowed node reference. The descriptor cannot outlive the node.
/// - `N`: Concrete node type implementing `Node<IN, OUT, InP, OutP>`.
/// - `IN`: Compile-time number of input ports for the node.
/// - `OUT`: Compile-time number of output ports for the node.
/// - `InP`: Input payload type (must implement `Payload`).
/// - `OutP`: Output payload type (must implement `Payload`).
///
/// # Invariants
/// Callers should ensure `in_ports == IN as u16` and `out_ports == OUT as u16` so the
/// stored counts are consistent with the node’s const-generic port arity.
#[derive(Debug, Clone)]
pub struct NodeLink<N, const IN: usize, const OUT: usize, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
    N: Node<IN, OUT, InP, OutP>,
{
    /// Owned handle to the concrete node instance.
    node: N,

    /// Unique identifier of this node within the graph.
    id: NodeIndex,

    /// Optional static name used for diagnostics or graph tooling.
    name: Option<&'static str>,

    /// Marker to bind `InP` and `OutP` into the type without storing values.
    ///
    /// This has zero runtime cost and exists solely for type tracking.
    _payload_marker: core::marker::PhantomData<(InP, OutP)>,
}

impl<N, const IN: usize, const OUT: usize, InP, OutP> NodeLink<N, IN, OUT, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
    N: Node<IN, OUT, InP, OutP>,
{
    /// Construct a new `NodeLink` that borrows the given node and records its metadata.
    ///
    /// # Parameters
    /// - `node`: Borrowed reference to the concrete node instance.
    /// - `id`: Unique identifier of the node in the graph.
    /// - `name`: Optional static name for diagnostics or tooling.
    pub fn new(node: N, id: NodeIndex, name: Option<&'static str>) -> Self {
        Self {
            node,
            id,
            name,
            _payload_marker: core::marker::PhantomData,
        }
    }

    /// Get a reference to the inner node.
    #[inline]
    pub fn node(&self) -> &N {
        &self.node
    }

    /// Get a mutable reference to the inner node.
    #[inline]
    pub fn node_mut(&mut self) -> &mut N {
        &mut self.node
    }

    /// Get the unique identifier of this node.
    #[inline]
    pub fn id(&self) -> NodeIndex {
        self.id
    }

    /// Returns the input port ids for the node.
    #[inline]
    pub fn input_port_ids(&self) -> [PortId; IN] {
        core::array::from_fn(|i| PortId::new(self.id, PortIndex::new(i)))
    }

    /// Returns the input port ids for the node.
    #[inline]
    pub fn output_port_ids(&self) -> [PortId; OUT] {
        core::array::from_fn(|i| PortId::new(self.id, PortIndex::new(i)))
    }

    /// Return the node's policy bundle.
    pub fn policy(&self) -> NodePolicy {
        self.node.policy()
    }

    /// Get the optional static name of this node.
    #[inline]
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }

    /// Return the `NodeDescriptor` for this `NodeLink`.
    #[inline]
    pub fn descriptor(&self) -> NodeDescriptor {
        NodeDescriptor {
            id: self.id(),
            kind: self.node.node_kind(),
            in_ports: IN as u16,
            out_ports: OUT as u16,
            name: self.name(),
        }
    }
}

impl<N, const IN: usize, const OUT: usize, InP, OutP> Node<IN, OUT, InP, OutP>
    for NodeLink<N, IN, OUT, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
    N: Node<IN, OUT, InP, OutP>,
{
    #[inline]
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node.describe_capabilities()
    }

    #[inline]
    fn input_acceptance(&self) -> [PlacementAcceptance; IN] {
        self.node.input_acceptance()
    }

    #[inline]
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT] {
        self.node.output_acceptance()
    }

    #[inline]
    fn policy(&self) -> NodePolicy {
        self.node.policy()
    }

    #[inline]
    fn node_kind(&self) -> NodeKind {
        self.node.node_kind()
    }

    #[inline]
    fn initialize<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.node.initialize(clock, telemetry)
    }

    #[inline]
    fn start<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.node.start(clock, telemetry)
    }

    #[inline]
    fn step<'graph, 'telemetry, 'clock, InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // If metrics are completely disabled for this Telemetry type, just delegate.
        if !T::METRICS_ENABLED {
            return self.node.step(ctx);
        }

        // For now we keep a single graph instance identifier, as in the runtime.
        const GRAPH_ID: crate::telemetry::GraphInstanceId = 0;

        // Cache static policy (copy) for deadline/budget checks.
        let policy = self.node.policy();
        let budget_policy = policy.budget;
        let deadline_policy = policy.deadline;

        // ---- Execute node step + measure latency ----
        let timestamp_start_ns = ctx.now_nanos();
        let result = self.node.step(ctx);
        let timestamp_end_ns = ctx.now_nanos();
        let duration_ns = timestamp_end_ns.saturating_sub(timestamp_start_ns);

        // ---- Compute deadline budget in nanoseconds (duration-based) ----

        let mut budget_ns_opt: Option<u64> = None;

        if let Some(default_deadline_ns) = deadline_policy.default_deadline_ns {
            budget_ns_opt = Some(default_deadline_ns.as_u64());
        } else if let Some(tick_budget) = budget_policy.tick_budget {
            let budget_ns = ctx.ticks_to_nanos(tick_budget);
            budget_ns_opt = Some(budget_ns);
        }

        let slack_ns: u64 = match deadline_policy.slack_tolerance_ns {
            Some(slack) => slack.as_u64(),
            None => 0,
        };

        let mut deadline_ns: Option<u64> = None;
        let mut deadline_missed = false;

        if let Some(budget_ns) = budget_ns_opt {
            // Represent this as an absolute deadline in the event.
            deadline_ns = Some(timestamp_start_ns.saturating_add(budget_ns));

            // Pure duration-based miss check, incorporating slack.
            if duration_ns > budget_ns.saturating_add(slack_ns) {
                deadline_missed = true;
            }
        }

        // ---- Telemetry updates (latency, processed, deadline, NodeStep event) ----

        // Access the telemetry sink from the context.
        let telemetry = ctx.telemetry_mut();

        // Latency metric (per node, per step).
        // This assumes `NodeIndex` is a tuple struct where `.0` yields a numeric index.
        telemetry.record_latency_ns(
            TelemetryKey::node(self.id.as_usize() as u32, TelemetryKind::Latency),
            duration_ns,
        );

        // Deadline miss counter (only if we computed a budget and exceeded it).
        if deadline_missed {
            telemetry.incr_counter(
                TelemetryKey::node(self.id.as_usize() as u32, TelemetryKind::DeadlineMiss),
                1,
            );
        }

        // Processed counter: count *steps* that actually made progress / completed.
        if let Ok(step_result) = &result {
            use crate::node::StepResult::*;
            match step_result {
                MadeProgress | Terminal | YieldUntil(_) => {
                    telemetry.incr_counter(
                        TelemetryKey::node(self.id.as_usize() as u32, TelemetryKind::Processed),
                        1,
                    );
                }
                NoInput | Backpressured | WaitingOnExternal => {
                    // Not counted as processed.
                }
            }
        }

        // Optional structured NodeStep event.
        if T::EVENTS_STATICALLY_ENABLED && telemetry.events_enabled() {
            let error_kind = match &result {
                Ok(step_result) => {
                    use crate::node::StepResult::*;
                    match step_result {
                        NoInput => Some(NodeStepError::NoInput),
                        Backpressured => Some(NodeStepError::Backpressured),
                        WaitingOnExternal => Some(NodeStepError::ExternalUnavailable),
                        // For progress/terminal/yield, only flag OverBudget if we
                        // actually missed the duration-based deadline.
                        MadeProgress | Terminal | YieldUntil(_) => {
                            if deadline_missed {
                                Some(NodeStepError::OverBudget)
                            } else {
                                None
                            }
                        }
                    }
                }
                Err(error) => {
                    Some(match error.kind() {
                        NodeErrorKind::NoInput => NodeStepError::NoInput,
                        NodeErrorKind::Backpressured => NodeStepError::Backpressured,
                        // Any other error kind is treated as a generic execution failure.
                        _ => NodeStepError::ExecutionFailed,
                    })
                }
            };

            let event = TelemetryEvent::node_step(NodeStepTelemetry::new(
                GRAPH_ID,
                self.id,
                self.name,
                timestamp_start_ns,
                timestamp_end_ns,
                duration_ns,
                deadline_ns,
                deadline_missed,
                error_kind,
            ));

            telemetry.push_event(event);
        }

        result
    }

    #[inline]
    fn on_watchdog_timeout<C, T>(
        &mut self,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        T: Telemetry,
    {
        self.node.on_watchdog_timeout(clock, telemetry)
    }

    #[inline]
    fn stop<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.node.stop(clock, telemetry)
    }
}

/// A node descriptor: topology and policy metadata, without an executable instance.
///
/// `NodeDescriptor` captures static configuration of a node in the graph:
/// its identity, kind, port counts, policy, and an optional name.
/// It does not hold runtime state or implementation details.
#[derive(Debug, Clone)]
pub struct NodeDescriptor {
    /// Unique identifier of this node in the graph.
    pub id: NodeIndex,
    /// High-level category of the node (source, process, sink, etc).
    pub kind: NodeKind,
    /// Number of input ports declared by this node.
    pub in_ports: u16,
    /// Number of output ports declared by this node.
    pub out_ports: u16,
    /// Optional static name (for diagnostics or graph tooling).
    pub name: Option<&'static str>,
}
