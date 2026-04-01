//! Inference node (1×in → 1×out) that runs a generic `ComputeBackend` model.
//!
//! # Design
//! - **No dynamic dispatch**: backend and model are monomorphized by generics.
//! - **No `unsafe`** in the hot path.
//! - **Batching**:
//!   - `no_std` / no-`alloc`: stack-bounded batching up to `MAX_BATCH`.
//!   - `alloc`: uses `Vec` for flexible batch sizing.
//! - **Queues/telemetry** are accessed only via `StepContext`.
//! - **Zero-copy** preferences are expressed through `PlacementAcceptance`.
//!
//! This node delegates inference to the model (`infer_one` / `infer_batch`), and
//! pushes outputs directly to the provided output edge. It never copies unless
//! required by payload semantics or batch buffering.

use crate::compute::{BackendCapabilities, ComputeBackend, ComputeModel, ModelMetadata};
use crate::edge::Edge;
use crate::errors::{InferenceError, NodeError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::node::{Node, NodeCapabilities, NodeKind, ProcessResult, StepContext, StepResult};
use crate::policy::NodePolicy;
use crate::prelude::{MemoryManager, PlatformClock, Telemetry};

// --- local helpers: map backend/queue errors into NodeError (no From impls required)
#[inline]
fn map_inference_err(e: InferenceError) -> NodeError {
    NodeError::execution_failed().with_code(*e.code())
}

/// Generic 1×1 inference node for any backend (dyn-free).
///
/// - `MAX_BATCH` is a compile-time cap used for the no-alloc path.
/// - When `alloc` is enabled, the batched path uses `Vec` (still no unsafe).
pub struct InferenceModel<B, InP, OutP, const MAX_BATCH: usize>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload,
    OutP: Payload + Default + Copy,
{
    /// Backend instance used solely for model creation and capability query.
    /// Kept to preserve type ownership and avoid dynamic dispatch.
    #[allow(dead_code)]
    backend: B,
    /// The loaded model instance that performs inference.
    model: B::Model,
    /// Backend capabilities snapshot (e.g., max batch size, streams).
    backend_caps: BackendCapabilities,
    /// Model metadata snapshot (I/O placement, size hints).
    model_meta: ModelMetadata,

    /// Declared capabilities of this node (streams, degrade tiers).
    node_caps: NodeCapabilities,
    /// Node policy bundle (batching, budget, deadlines).
    node_policy: NodePolicy,
    /// Zero-copy placement acceptance for the input port.
    input_acceptance: [PlacementAcceptance; 1],
    /// Zero-copy placement acceptance for the output port.
    output_acceptance: [PlacementAcceptance; 1],

    /// Reusable output for the 1× fast path (constructed once).
    scratch_out: OutP,

    _pd: core::marker::PhantomData<InP>,
}

impl<B, InP, OutP, const MAX_BATCH: usize> InferenceModel<B, InP, OutP, MAX_BATCH>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload,
    OutP: Payload + Default + Copy,
{
    /// Construct a new `InferenceModel` node.
    ///
    /// - `backend`: concrete compute backend (e.g., Tract, TFLM adapter).
    /// - `desc`: backend-specific, borrowed model descriptor (e.g., bytes, artifact).
    /// - `node_policy`: batching/budget/deadline policies for the node.
    /// - `node_caps`: advertised capabilities (e.g., device streams).
    /// - `input_acceptance` / `output_acceptance`: zero-copy placement preferences.
    pub fn new<'desc>(
        backend: B,
        desc: B::ModelDescriptor<'desc>,
        node_policy: NodePolicy,
        node_caps: NodeCapabilities,
        input_acceptance: [PlacementAcceptance; 1],
        output_acceptance: [PlacementAcceptance; 1],
    ) -> Result<Self, B::Error> {
        let backend_caps = backend.capabilities();
        let model = backend.load_model(desc)?;
        let model_meta = model.metadata();

        Ok(Self {
            backend,
            model,
            backend_caps,
            model_meta,
            node_caps,
            node_policy,
            input_acceptance,
            output_acceptance,
            scratch_out: OutP::default(),
            _pd: core::marker::PhantomData,
        })
    }

    /// Return cached backend capabilities for this node.
    #[inline]
    pub fn backend_capabilities(&self) -> BackendCapabilities {
        self.backend_caps
    }

    /// Return cached model metadata for this node.
    #[inline]
    pub fn model_metadata(&self) -> ModelMetadata {
        self.model_meta
    }
}

impl<B, InP, OutP, const MAX_BATCH: usize> Node<1, 1, InP, OutP>
    for InferenceModel<B, InP, OutP, MAX_BATCH>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload + Default + Copy,
    OutP: Payload + Default + Copy,
{
    #[inline]
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node_caps
    }

    #[inline]
    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.input_acceptance
    }

    #[inline]
    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.output_acceptance
    }

    #[inline]
    fn policy(&self) -> NodePolicy {
        self.node_policy
    }

    /// **TEST ONLY** method used to override batching policis for node contract tests.
    #[cfg(any(test, feature = "bench"))]
    fn set_policy(&mut self, policy: NodePolicy) {
        self.node_policy = policy;
    }

    #[inline]
    fn node_kind(&self) -> NodeKind {
        NodeKind::Model
    }

    #[inline]
    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        Ok(())
    }

    #[inline]
    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.model.init().map_err(map_inference_err)
    }

    #[inline]
    fn process_message<C>(
        &mut self,
        msg: &Message<InP>,
        _sys_clock: &C,
    ) -> Result<ProcessResult<OutP>, NodeError>
    where
        C: PlatformClock + Sized,
    {
        // Run single-item inference into the reusable scratch output.
        let inp: &InP = msg.payload();
        self.model
            .infer_one(inp, &mut self.scratch_out)
            .map_err(map_inference_err)?;

        // Build output message reusing header from input.
        let hdr = *msg.header();
        let out_msg = Message::new(hdr, core::mem::take(&mut self.scratch_out));

        Ok(ProcessResult::Output(out_msg))
    }

    #[inline]
    fn step<'g, 't, 'c, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, InM, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        ctx.pop_and_process(0, |msg| self.process_message(msg, ctx.clock))
    }

    #[inline]
    fn step_batch<'g, 't, 'c, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, InM, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        let want = self.node_policy.batching().fixed_n().unwrap_or(1);
        let backend_cap = self.backend_caps.max_batch().unwrap_or(usize::MAX);
        let nmax = core::cmp::min(core::cmp::min(want, backend_cap), MAX_BATCH);

        if nmax <= 1 {
            return self.step(ctx);
        }

        let node_policy = self.node_policy;
        let clock = ctx.clock;

        ctx.pop_batch_and_process(0, nmax, &node_policy, |msg| {
            self.process_message(msg, clock)
        })
    }

    #[inline]
    fn on_watchdog_timeout<C, Tel>(
        &mut self,
        clock: &C,
        _telemetry: &mut Tel,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        Tel: Telemetry,
    {
        if let Some(backoff) = self.node_policy.budget().watchdog_ticks() {
            let until = clock.now_ticks().saturating_add(*backoff);
            Ok(StepResult::YieldUntil(until))
        } else {
            Ok(StepResult::YieldUntil(clock.now_ticks()))
        }
    }

    #[inline]
    fn stop<C, Tel>(&mut self, _clock: &C, _telemetry: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry,
    {
        self.model.drain().map_err(map_inference_err)?;
        self.model.reset().map_err(map_inference_err)
    }
}
