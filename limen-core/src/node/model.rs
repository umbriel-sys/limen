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
use crate::node::{Node, NodeCapabilities, NodeKind, StepContext, StepResult};
use crate::policy::NodePolicy;
use crate::prelude::{PlatformClock, Telemetry};

use heapless::Vec as HeaplessVec;

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

    fn step<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        cx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Decide effective batch size.
        let want = self.node_policy.batching().fixed_n().unwrap_or(1);
        // FIX / TODO: should this unwrap to usize::max?
        let cap = self.backend_caps.max_batch().unwrap_or(usize::MAX);
        let nmax = core::cmp::min(core::cmp::min(want, cap), MAX_BATCH);

        // Single-item fast path (no alloc, no arrays).
        if nmax == 1 {
            let m_in = match cx.in_try_pop(0) {
                Ok(m) => m,
                Err(QueueError::Empty) => return Ok(StepResult::NoInput),
                Err(QueueError::Backpressured) => return Ok(StepResult::Backpressured),
                Err(e) => return Err(map_queue_err(e)),
            };

            let inp: &InP = m_in.payload();
            self.model
                .infer_one(inp, &mut self.scratch_out)
                .map_err(map_inference_err)?;

            let out_msg = m_in.with_payload(core::mem::take(&mut self.scratch_out));
            return match cx.out_try_push(0, out_msg) {
                EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => {
                    Ok(StepResult::MadeProgress)
                }
                EnqueueResult::Rejected => Ok(StepResult::Backpressured),
            };
        }

        // Batched path:
        #[cfg(not(feature = "alloc"))]
        {
            self.step_batched_stack::<InQ, OutQ, C, T>(cx, nmax)
        }
        #[cfg(feature = "alloc")]
        {
            self.step_batched_alloc::<InQ, OutQ, C, T>(cx, nmax)
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
        // NOTE: for stack arrays without `alloc`, we require `Copy + Default` on InP.
        InP: Payload,
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // Fixed-capacity, stack-allocated scratch (no alloc).
        let mut headers: [MessageHeader; MAX_BATCH] =
            core::array::from_fn(|_| MessageHeader::empty());
        let mut in_buf: HeaplessVec<InP, { MAX_BATCH }> = HeaplessVec::new();
        let mut out_buf: [OutP; MAX_BATCH] = core::array::from_fn(|_| OutP::default());

        while in_buf.len() < nmax {
            match cx.in_try_pop(0) {
                Ok(m) => {
                    let (h, p) = m.into_parts();
                    let idx = in_buf.len();
                    headers[idx] = h;
                    // `nmax <= MAX_BATCH` should guarantee capacity; if this ever
                    // fails, it indicates a logic error. Avoid imposing `Debug`
                    // on `InP` by not using `.expect(..)`.
                    if let Err(_overflowed) = in_buf.push(p) {
                        debug_assert!(
                            false,
                            "heapless capacity exceeded (nmax <= MAX_BATCH invariant broken)"
                        );
                        return Err(NodeError::execution_failed().with_code(1));
                    }
                }
                Err(QueueError::Empty) | Err(QueueError::Backpressured) => break,
                Err(e) => return Err(map_queue_err(e)),
            }
        }

        let n = in_buf.len();

        if n == 0 {
            return Ok(StepResult::NoInput);
        }

        // Mark batch boundary flags.
        headers[0].set_first_in_batch();
        headers[n - 1].set_last_in_batch();

        self.model
            .infer_batch(in_buf.as_slice(), &mut out_buf[..n])
            .map_err(map_inference_err)?;

        for i in 0..n {
            let out_msg = Message::new(headers[i], core::mem::take(&mut out_buf[i]));
            match cx.out_try_push(0, out_msg) {
                EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => { /* progress */ }
                EnqueueResult::Rejected => return Ok(StepResult::Backpressured),
            };
        }
        Ok(StepResult::MadeProgress)
    }

    #[cfg(feature = "alloc")]
    fn step_batched_alloc<'g, 't, 'c, InQ, OutQ, C, T>(
        &mut self,
        cx: &mut StepContext<'g, 't, 'c, 1, 1, InP, OutP, InQ, OutQ, C, T>,
        nmax: usize,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let mut headers: Vec<MessageHeader> = Vec::with_capacity(nmax);
        let mut in_buf: Vec<InP> = Vec::with_capacity(nmax);

        while headers.len() < nmax {
            match cx.in_try_pop(0) {
                Ok(m) => {
                    let (h, p) = m.into_parts();
                    headers.push(h);
                    in_buf.push(p);
                }
                Err(QueueError::Empty) | Err(QueueError::Backpressured) => break,
                Err(e) => return Err(map_queue_err(e)),
            }
        }

        if headers.is_empty() {
            return Ok(StepResult::NoInput);
        }

        // Mark batch boundary flags.
        headers[0].set_first_in_batch();
        let last = headers.len() - 1;
        headers[last].set_last_in_batch();

        // Prepare outputs and run batched inference.
        let mut out_buf: Vec<OutP> = Vec::with_capacity(headers.len());
        out_buf.resize_with(headers.len(), || OutP::default());
        self.model
            .infer_batch(&in_buf, &mut out_buf)
            .map_err(map_inference_err)?;

        for i in 0..headers.len() {
            let out_msg = Message::new(headers[i], core::mem::take(&mut out_buf[i]));
            match cx.out_try_push(0, out_msg) {
                EnqueueResult::Enqueued | EnqueueResult::DroppedNewest => { /* progress */ }
                EnqueueResult::Rejected => return Ok(StepResult::Backpressured),
            };
        }
        Ok(StepResult::MadeProgress)
    }
}
