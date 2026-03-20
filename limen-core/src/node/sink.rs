//! Sink node trait and adapter.
//!
//! A `Sink` has ≥1 inputs and **0 outputs**. It consumes messages from one input
//! per `step()` and commits them to an external side effect (file, stdout, GPIO,
//! network, etc.). No dynamic dispatch in the hot path; everything is monomorphized.
//!
//! Design goals:
//! - Minimal trait to implement a new sink.
//! - Default input-selection strategy (first non-empty), overridable per sink.
//! - Adapter `SinkNode<S, InP, IN>` that implements `Node<IN, 0, InP, ()>`.
//! - Implicit `From<S>` so graphs can take `impl Into<SinkNode<...>>` and users
//!   never have to mention the adapter type.

use crate::edge::{Edge, EdgeOccupancy};
use crate::errors::NodeError;
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::node::{Node, NodeCapabilities, NodeKind, OutStepContext, StepContext, StepResult};
use crate::policy::NodePolicy;
use crate::prelude::{MemoryManager, PlatformClock, Telemetry};

use core::marker::PhantomData;

/// Uniform contract for sink implementations (≥1 inputs / 0 outputs).
///
/// # Type Parameters
/// * `InP` — Payload type consumed by the sink.
/// * `IN`  — Number of input ports on the sink node.
pub trait Sink<InP, const IN: usize>
where
    InP: Payload,
{
    /// Sink-specific error type for `open()` or `consume()`.
    type Error;

    /// Prepare the sink for consumption (open file/device, connect network, etc.).
    ///
    /// Called from `Node::initialize`. Must be idempotent or fail safely if called
    /// multiple times by a higher layer.
    fn open(&mut self) -> Result<(), Self::Error>;

    /// Consume a single message pulled from `port`.
    ///
    /// This is where side effects happen (write, print, publish). Return `Ok(())`
    /// on success. Errors are mapped to `NodeError::execution_failed()`.
    fn consume(&mut self, msg: &Message<InP>) -> Result<(), Self::Error>;

    /// Input placement acceptances for zero-copy compatibility.
    fn input_acceptance(&self) -> [PlacementAcceptance; IN];

    /// Describe sink capabilities (device streams, degrade tiers, etc.).
    fn capabilities(&self) -> NodeCapabilities;

    /// Provide the node policy bundle (batching/budget/deadlines).
    fn policy(&self) -> NodePolicy;

    /// Optional: choose which input to read this step based on occupancies.
    ///
    /// Default strategy: first input with `items > 0`. Return `None` to indicate
    /// "no input available now".
    #[inline]
    fn select_input(&mut self, occ: &[EdgeOccupancy; IN]) -> Option<usize> {
        occ.iter().position(|o| *o.items() > 0)
    }
}

/// A thin adapter that exposes a `Sink` as a `Node<IN, 0, InP, ()>`.
///
/// Owns the sink and forwards lifecycle calls. Users do **not** construct this
/// directly — graphs can accept `impl Into<SinkNode<...>>` and rely on `From<S>`.
pub struct SinkNode<S, InP, const IN: usize>
where
    S: Sink<InP, IN>,
    InP: Payload,
{
    sink: S,
    policy: NodePolicy,
    _pd: PhantomData<InP>,
}

impl<S, InP, const IN: usize> SinkNode<S, InP, IN>
where
    S: Sink<InP, IN>,
    InP: Payload,
{
    /// Construct a `SinkNode` from a sink and a static policy bundle.
    #[inline]
    pub const fn new(sink: S, policy: NodePolicy) -> Self {
        Self {
            sink,
            policy,
            _pd: PhantomData,
        }
    }

    /// Borrow the underlying sink.
    #[inline]
    pub fn sink_ref(&self) -> &S {
        &self.sink
    }

    /// Mutably borrow the underlying sink.
    #[inline]
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}

/// Allow graphs to accept any `Sink` and convert implicitly.
impl<S, InP, const IN: usize> From<S> for SinkNode<S, InP, IN>
where
    S: Sink<InP, IN>,
    InP: Payload,
{
    #[inline]
    fn from(sink: S) -> Self {
        let policy = sink.policy();
        SinkNode::new(sink, policy)
    }
}

impl<S, InP, const IN: usize> Node<IN, 0, InP, ()> for SinkNode<S, InP, IN>
where
    S: Sink<InP, IN>,
    InP: Payload + Copy,
{
    #[inline]
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.sink.capabilities()
    }

    #[inline]
    fn input_acceptance(&self) -> [PlacementAcceptance; IN] {
        self.sink.input_acceptance()
    }

    #[inline]
    fn output_acceptance(&self) -> [PlacementAcceptance; 0] {
        []
    }

    #[inline]
    fn policy(&self) -> NodePolicy {
        self.policy
    }

    /// **TEST ONLY** method used to override batching policies for node contract tests.
    #[cfg(any(test, feature = "bench"))]
    fn set_policy(&mut self, policy: NodePolicy) {
        self.policy = policy;
    }

    #[inline]
    fn node_kind(&self) -> NodeKind {
        NodeKind::Sink
    }

    #[inline]
    fn initialize<C, T>(&mut self, _c: &C, _t: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.sink
            .open()
            .map_err(|_| NodeError::external_unavailable())
    }

    #[inline]
    fn start<C, T>(&mut self, _c: &C, _t: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        Ok(())
    }

    #[inline]
    fn process_message<'graph, 'clock, OutQ, OutM, C, Tel>(
        &mut self,
        msg: &Message<InP>,
        _out_ctx: &mut OutStepContext<'graph, '_, 'clock, 0, (), OutQ, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        OutQ: Edge,
        OutM: MemoryManager<()>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        self.sink
            .consume(msg)
            .map(|_| StepResult::MadeProgress)
            .map_err(|_| NodeError::execution_failed())
    }

    #[inline]
    fn step<'g, 't, 'ck, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        cx: &mut StepContext<'g, 't, 'ck, IN, 0, InP, (), InQ, OutQ, InM, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<()>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        // Snapshot occupancies and let the sink choose an input.
        let occ: [EdgeOccupancy; IN] = core::array::from_fn(|i| cx.in_occupancy(i));
        let port = match self.sink.select_input(&occ) {
            Some(i) => i,
            None => return Ok(StepResult::NoInput),
        };

        cx.pop_and_process(port, |msg, _out| {
            self.sink
                .consume(msg)
                .map(|_| StepResult::MadeProgress)
                .map_err(|_| NodeError::execution_failed())
        })
    }

    #[inline]
    fn step_batch<'graph, 'telemetry, 'clock, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<
            'graph,
            'telemetry,
            'clock,
            IN,
            0,
            InP,
            (),
            InQ,
            OutQ,
            InM,
            OutM,
            C,
            Tel,
        >,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<()>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        let node_policy = self.policy();
        let port = match (0..IN).find(|&p| ctx.input_edge_has_batch(p, &node_policy)) {
            Some(p) => p,
            None => return Ok(StepResult::NoInput),
        };
        let nmax = node_policy.batching().fixed_n().unwrap_or(1);

        ctx.pop_batch_and_process(port, nmax, &node_policy, |iter, out| {
            let mut any_made = false;
            for guard in iter {
                match self.process_message(&*guard, out)? {
                    StepResult::MadeProgress => any_made = true,
                    StepResult::NoInput => {}
                    StepResult::Backpressured => return Ok(StepResult::Backpressured),
                    StepResult::WaitingOnExternal => {
                        return Ok(StepResult::WaitingOnExternal);
                    }
                    StepResult::YieldUntil(t) => return Ok(StepResult::YieldUntil(t)),
                    StepResult::Terminal => return Ok(StepResult::Terminal),
                }
            }
            if any_made {
                Ok(StepResult::MadeProgress)
            } else {
                Ok(StepResult::NoInput)
            }
        })
    }

    #[inline]
    fn on_watchdog_timeout<C, Tel>(
        &mut self,
        clock: &C,
        _t: &mut Tel,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        Tel: Telemetry,
    {
        Ok(StepResult::YieldUntil(clock.now_ticks()))
    }

    #[inline]
    fn stop<C, Tel>(&mut self, _c: &C, _t: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry,
    {
        Ok(())
    }
}
