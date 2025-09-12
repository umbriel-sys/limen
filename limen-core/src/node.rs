//! Uniform Node contract and lifecycle.
//!
//! Nodes are monomorphized by generics and const generics. There is **no dynamic
//! dispatch** in the hot path. Port schemas and policies are encoded on the Node.

use crate::errors::NodeError;
use crate::memory::PlacementAcceptance;
use crate::message::{Message, Payload};
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy};
use crate::telemetry::Telemetry;
use crate::types::Ticks;

/// Node capability descriptor (ops, dtypes, layouts, streams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeCapabilities {
    /// Whether the node can execute on device streams (P2).
    pub device_streams: bool,
    /// Whether mixed-precision or degrade tiers are available.
    pub degrade_tiers: bool,
}

/// Policy bundle attached to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodePolicy {
    /// Batch formation policy.
    pub batching: BatchingPolicy,
    /// Budget policy for execution steps.
    pub budget: BudgetPolicy,
    /// Deadline policy for inputs/outputs.
    pub deadline: DeadlinePolicy,
}

/// Result of a `step` call indicating progress and scheduling hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// Work was performed (messages consumed and/or produced).
    MadeProgress,
    /// No inputs were available to make progress.
    NoInput,
    /// Backpressure prevented progress.
    Backpressured,
    /// Waiting on external completion (device, transport).
    WaitingOnExternal,
    /// Yield until provided tick (cooperative scheduling hint).
    YieldUntil(Ticks),
    /// Node has completed and will not produce further outputs.
    Terminal,
}

/// A context provided to nodes during `step`, abstracting queues and services.
///
/// The context is generic over input/output payload and queue types to avoid
/// trait objects. Implementations in runtimes will construct instances of this
/// context and pass them to nodes.
pub struct StepContext<'a, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
{
    /// Arrays of inbound queues by input port index.
    pub inputs: [&'a mut InQ; IN],
    /// Arrays of outbound queues by output port index.
    pub outputs: [&'a mut OutQ; OUT],
    /// Edge policies for each input.
    pub in_policies: [EdgePolicy; IN],
    /// Edge policies for each output.
    pub out_policies: [EdgePolicy; OUT],
    /// Platform clock or timer services.
    pub clock: &'a C,
    /// Telemetry sink for counters/histograms.
    pub telemetry: &'a mut T,
    /// Phantom type markers to keep payload types visible to the compiler.
    _marker: core::marker::PhantomData<(InP, OutP)>,
}

impl<'a, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
    StepContext<'a, IN, OUT, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
{
    /// Create a new step context from queues, policies, and services.
    pub fn new(
        inputs: [&'a mut InQ; IN],
        outputs: [&'a mut OutQ; OUT],
        in_policies: [EdgePolicy; IN],
        out_policies: [EdgePolicy; OUT],
        clock: &'a C,
        telemetry: &'a mut T,
    ) -> Self {
        Self {
            inputs,
            outputs,
            in_policies,
            out_policies,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        }
    }
}

/// The uniform node contract.
///
/// Nodes are parameterized by:
/// - `IN`: number of input ports; `OUT`: number of output ports;
/// - `InP`: input payload type; `OutP`: output payload type.
pub trait Node<const IN: usize, const OUT: usize, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
{
    /// Return the node's capability descriptor.
    fn describe_capabilities(&self) -> NodeCapabilities;

    /// Return the node's port placement acceptances (zero-copy compatibility).
    fn input_acceptance(&self) -> [PlacementAcceptance; IN];

    /// Return the node's output placement preferences (zero-copy compatibility).
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT];

    /// Return the node's policy bundle.
    fn policy(&self) -> NodePolicy;

    /// Prepare internal state, acquire buffers, and register telemetry series.
    fn initialize<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>;

    /// Optional warm-up (e.g., compile kernels, prime pools). Default: no-op.
    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>;

    /// Execute one cooperative step using the provided context.
    ///
    /// The input and output queues are exposed through the context, along with
    /// per-edge policies and services. Implementations should honor the node
    /// policy (batching, budgets, deadlines) and return a `StepResult` to help
    /// the scheduler make progress decisions.
    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<IN, OUT, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>;

    /// Handle watchdog timeouts by applying over-budget policy (degrade/default/skip).
    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError>;

    /// Flush and release resources, if any. Default: no-op.
    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>;
}
