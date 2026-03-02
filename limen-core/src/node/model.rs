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
use crate::edge::{Edge, EnqueueResult};
use crate::errors::{InferenceError, NodeError, QueueError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message, MessageHeader};
use crate::node::{Node, NodeCapabilities, NodeKind, OutStepContext, StepContext, StepResult};
use crate::policy::NodePolicy;
use crate::prelude::{PlatformClock, Telemetry};

// alloc-backed buffers only when the feature is enabled
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

// --- local helpers: map backend/queue errors into NodeError (no From impls required)
#[inline]
fn map_inference_err(e: InferenceError) -> NodeError {
    NodeError::execution_failed().with_code(*e.code())
}
#[inline]
fn map_queue_err(e: QueueError) -> NodeError {
    match e {
        QueueError::Empty => NodeError::no_input(),
        QueueError::Backpressured | QueueError::AtOrAboveHardCap => NodeError::backpressured(),
        QueueError::Unsupported | QueueError::Poisoned => NodeError::execution_failed(),
    }
}

/// Generic 1×1 inference node for any backend (dyn-free).
///
/// - `MAX_BATCH` is a compile-time cap used for the no-alloc path.
/// - When `alloc` is enabled, the batched path uses `Vec` (still no unsafe).
pub struct InferenceModel<B, InP, OutP, const MAX_BATCH: usize>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload,
    OutP: Payload + Default,
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
    OutP: Payload + Default,
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
    InP: Payload,
    OutP: Payload + Default,
{
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node_caps
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.input_acceptance
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.output_acceptance
    }

    fn policy(&self) -> NodePolicy {
        self.node_policy
    }

    /// **TEST ONLY** method used to override batching policis for node contract tests.
    #[cfg(any(test, feature = "bench"))]
    fn set_policy(&mut self, policy: NodePolicy) {
        self.node_policy = policy;
    }

    fn node_kind(&self) -> NodeKind {
        NodeKind::Model
    }

    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        Ok(())
    }

    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.model.init().map_err(map_inference_err)
    }

    fn process_message<'graph, 'telemetry, 'clock, OutQ, C, T>(
        &mut self,
        msg: &Message<InP>,
        out_ctx: &mut OutStepContext<'graph, '_, 'clock, 1, OutP, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Run single-item inference into the reusable scratch output.
        let inp: &InP = msg.payload();
        self.model
            .infer_one(inp, &mut self.scratch_out)
            .map_err(map_inference_err)?;

        // Build output message reusing header from input (clone the header).
        let hdr = *msg.header();
        let out_msg = Message::new(hdr, core::mem::take(&mut self.scratch_out));

        // Push to output 0 and map enqueue result to StepResult.
        match out_ctx.out_try_push(0, out_msg) {
            EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => Ok(StepResult::MadeProgress),
            EnqueueResult::Rejected => Ok(StepResult::Backpressured),
        }
    }

    fn step<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Pop a single message (map queue errors consistently).
        let m_in = match ctx.in_try_pop(0) {
            Ok(m) => m,
            Err(QueueError::Empty) => return Ok(StepResult::NoInput),
            Err(QueueError::Backpressured) => return Ok(StepResult::Backpressured),
            Err(e) => return Err(map_queue_err(e)),
        };

        // Create an OutStepContext (borrows outputs/telemetry) while message is owned.
        let mut out = ctx.to_out_step_context();

        // Delegate to per-message hook. We pass a reference to the owned message.
        let res = self.process_message(&m_in, &mut out)?;
        Ok(res)
    }

    fn step_batch<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Decide effective batch size from node policy, backend caps, and MAX_BATCH.
        let want = self.node_policy.batching().fixed_n().unwrap_or(1);
        let backend_cap = self.backend_caps.max_batch().unwrap_or(usize::MAX);
        let nmax = core::cmp::min(core::cmp::min(want, backend_cap), MAX_BATCH);

        if nmax <= 1 {
            // No batching requested — fall back to single-step.
            return self.step(ctx);
        }

        // Use the existing batched helpers which consume via `ctx.in_try_pop` and call infer_batch.
        #[cfg(not(feature = "alloc"))]
        {
            self.step_batched_stack::<InQ, OutQ, C, T>(ctx, nmax)
        }
        #[cfg(feature = "alloc")]
        {
            self.step_batched_alloc::<InQ, OutQ, C, T>(ctx, nmax)
        }
    }

    fn on_watchdog_timeout<C, T>(
        &mut self,
        clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        T: Telemetry,
    {
        // Use configured budget backoff if present; otherwise yield once at "now".
        if let Some(backoff) = self.node_policy.budget().watchdog_ticks() {
            let until = clock.now_ticks().saturating_add(*backoff);
            Ok(StepResult::YieldUntil(until))
        } else {
            Ok(StepResult::YieldUntil(clock.now_ticks()))
        }
    }

    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.model.drain().map_err(map_inference_err)?;
        self.model.reset().map_err(map_inference_err)
    }
}

#[allow(dead_code)]
impl<B, InP, OutP, const MAX_BATCH: usize> InferenceModel<B, InP, OutP, MAX_BATCH>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload,
    OutP: Payload + Default,
{
    fn step_batched_stack<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        cx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
        nmax: usize,
    ) -> Result<StepResult, NodeError>
    where
        // NOTE: for stack arrays without `alloc`, we require `Default` on InP
        // so we can `mem::take()` payloads out of borrowed Messages.
        InP: Payload,
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Use the StepContext batch helper that enforces sliding/disjoint semantics.
        match cx.pop_input_messages_as_batch_with_out(0, nmax, &self.node_policy) {
            Ok((batch_view, mut out)) => {
                let batch_len = batch_view.len();
                if batch_len == 0 {
                    return Ok(StepResult::NoInput);
                }

                // Stack scratch buffers (MAX_BATCH).
                let mut headers: [MessageHeader; MAX_BATCH] =
                    core::array::from_fn(|_| MessageHeader::empty());
                let mut out_buf: [OutP; MAX_BATCH] = core::array::from_fn(|_| OutP::default());

                // Convert the BatchView into the public Batch<'_, InP> (borrowed view).
                // The Batch references the messages inside `batch_view`.
                let batch = batch_view.as_batch();
                let msgs = batch.messages();
                let n = msgs.len();

                if n == 0 {
                    return Ok(StepResult::NoInput);
                }

                // Copy headers (MessageHeader is Copy). The batch helper has already
                // set FIRST/LAST flags for us.
                for i in 0..n {
                    headers[i] = *msgs[i].header();
                }

                // Call the backend with a borrowed Batch<'_, InP> so backends receive
                // `&InP` references directly. No allocation, no moves/clones.
                self.model
                    .infer_batch(batch, &mut out_buf[..n])
                    .map_err(map_inference_err)?;

                // Emit outputs using the provided OutStepContext.
                for i in 0..n {
                    let out_msg = Message::new(headers[i], core::mem::take(&mut out_buf[i]));
                    match out.out_try_push(0, out_msg) {
                        EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => {}
                        EnqueueResult::Rejected => return Ok(StepResult::Backpressured),
                    }
                }

                Ok(StepResult::MadeProgress)
            }
            Err(QueueError::Empty) => Ok(StepResult::NoInput),
            Err(QueueError::Backpressured) => Err(NodeError::backpressured()),
            Err(e) => Err(map_queue_err(e)),
        }
    }

    #[cfg(feature = "alloc")]
    fn step_batched_alloc<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        cx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
        nmax: usize,
    ) -> Result<StepResult, NodeError>
    where
        InP: Payload,
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        match cx.pop_input_messages_as_batch_with_out(0, nmax, &self.node_policy) {
            Ok((batch_view, mut out)) => {
                let batch_len = batch_view.len();
                if batch_len == 0 {
                    return Ok(StepResult::NoInput);
                }

                // Convert to borrowed Batch and acquire message slice.
                let batch = batch_view.as_batch();
                let msgs = batch.messages();
                let n = msgs.len();

                let mut headers: Vec<MessageHeader> = Vec::with_capacity(n);

                // Copy headers from the borrowed messages.
                for msg in msgs.iter() {
                    headers.push(*msg.header());
                }

                // Prepare output buffer and run inference using the borrowed Batch.
                let mut out_buf: Vec<OutP> = Vec::with_capacity(n);
                out_buf.resize_with(n, || OutP::default());
                self.model
                    .infer_batch(batch, &mut out_buf)
                    .map_err(map_inference_err)?;

                for i in 0..n {
                    let out_msg = Message::new(headers[i], core::mem::take(&mut out_buf[i]));
                    match out.out_try_push(0, out_msg) {
                        EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => {}
                        EnqueueResult::Rejected => return Ok(StepResult::Backpressured),
                    }
                }

                Ok(StepResult::MadeProgress)
            }
            Err(QueueError::Empty) => Ok(StepResult::NoInput),
            Err(QueueError::Backpressured) => Err(NodeError::backpressured()),
            Err(e) => Err(map_queue_err(e)),
        }
    }
}
