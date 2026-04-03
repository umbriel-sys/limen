//! Source node traits and adapters.
//!
//! This module defines a minimal `Source` trait and a `SourceNode` adapter that
//! plugs a source into the existing `Node` and `Edge` contracts without changing
//! any runtime or graph APIs. It also includes:
//! //! - `SourceIngressEdge`: a borrowing, no-alloc adapter that exposes **ingress
//!   pressure** (items/bytes before the source) as an `Edge` so that the graph
//!   and runtimes can uniformly sample it with their existing occupancy code.
//! - `IngressProbe` / `NoProbe` / `IngressProbeImpl`: platform-agnostic ingress
//!   pressure observer — zero-cost `NoProbe` by default; replace for real occupancy
//!
//! ### Design notes
//! * A `Source` has **no input ports** and one or more **output ports**. It can
//!   produce at most one message per `step()` via `try_produce()`.
//! * Ingress pressure is surfaced via `ingress_occupancy()`, which the graph
//!   exposes as a synthetic "monitor edge" using `SourceIngressEdge` (no_std)
//!   or `probe::SourceIngressProbeEdge` (std). Runtimes keep using
//!   `GraphApi::(edge_)occupancy` without any special-case code.

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::NodeError;
use crate::errors::QueueError;
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::node::{Node, NodeCapabilities, NodeKind, ProcessResult, StepContext, StepResult};
use crate::policy::{BatchingPolicy, EdgePolicy, NodePolicy, WatermarkState};
use crate::prelude::{
    BatchView, EdgeDescriptor, HeaderStore, MemoryManager, PlatformClock, Telemetry,
};
use crate::types::{EdgeIndex, MessageToken, NodeIndex, PortId};

use core::marker::PhantomData;

/// Reserved node index used for virtual input nodes.
pub const EXTERNAL_INGRESS_NODE: NodeIndex = NodeIndex::new(usize::MAX);

/// Uniform contract for source implementations (0 inputs / ≥1 outputs).
///
/// `Source` types produce messages for downstream nodes and report **ingress
/// pressure** (items/bytes *before* the source, e.g., device FIFO depth),
/// allowing schedulers to decide when to poll the source.
///
/// # Type Parameters
/// * `OutP` — Payload type for produced messages.
/// * `OUT`  — Number of output ports on the source node.
pub trait Source<OutP, const OUT: usize>
where
    OutP: Payload,
{
    /// Source-specific error type for `open()`.
    type Error;

    /// Prepare the source for production (e.g., open device, init driver).
    ///
    /// Called from `Node::initialize`. Must be idempotent or fail safely if
    /// called multiple times by a higher layer.
    fn open(&mut self) -> Result<(), Self::Error>;

    /// Attempt to produce **exactly one** `(port, message)` pair.
    ///
    /// Return `None` if there is nothing to produce *right now*.
    ///
    /// # Contract
    /// * Must be **non-blocking**.
    /// * The returned `port` must be `< OUT`.
    fn try_produce(&mut self) -> Option<(usize, Message<OutP>)>;

    /// Report **ingress pressure** (items/bytes before the source).
    ///
    /// Implementations should be **non-blocking** and may read hardware
    /// counters, driver FIFOs, ring buffer lengths, or cached snapshots.
    ///
    /// `policy` is provided so implementations can compute a consistent
    /// `EdgeOccupancy.watermark` using the same thresholds as real edges.
    fn ingress_occupancy(&self) -> EdgeOccupancy;

    /// Return the creation tick of the `index`'th ingress item (0-based) without
    /// dequeuing it. Implementations must be non-blocking and non-destructive.
    /// Return `None` if metadata is unavailable or `index` is out-of-range.
    fn peek_ingress_creation_tick(&self, item_index: usize) -> Option<u64>;

    /// Return output placement acceptances for zero-copy compatibility.
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT];

    /// Describe source capabilities (device streams, degrade tiers, etc.).
    fn capabilities(&self) -> NodeCapabilities;

    /// Convenience: wrap this source in a `SourceNode` with the provided policy.
    ///
    /// This is a zero-overhead helper so all `Source` implementations can be
    /// lifted into a node uniformly without each impl writing a custom helper.
    #[inline]
    fn into_sourcenode(self, policy: NodePolicy) -> SourceNode<Self, OutP, OUT>
    where
        Self: Sized,
    {
        SourceNode::new(self, policy)
    }

    /// Provide the node policy bundle (batching/budget/deadlines).
    fn policy(&self) -> NodePolicy;

    /// Provude the ingress edge policy for this source node.
    fn ingress_policy(&self) -> EdgePolicy;
}

/// A thin adapter that exposes a `Source` as a `Node<0, OUT, (), OutP>`.
///
/// This allows sources to participate in graphs and be scheduled by runtimes
/// without any special-case code. The node owns the source and forwards the
/// node lifecycle calls as needed.
pub struct SourceNode<S, OutP, const OUT: usize>
where
    S: Source<OutP, OUT>,
    OutP: Payload,
{
    /// The concrete source implementation.
    src: S,
    /// Static node policy (batching/budgets/deadlines).
    policy: NodePolicy,
    /// Phantom to bind the `OutP` generic.
    _pd: PhantomData<OutP>,
}

/// Allow graphs to accept any `Source` and convert implicitly.
impl<S, OutP, const OUT: usize> From<S> for SourceNode<S, OutP, OUT>
where
    S: Source<OutP, OUT>,
    OutP: Payload,
{
    #[inline]
    fn from(src: S) -> Self {
        let policy = src.policy();
        SourceNode::new(src, policy)
    }
}

impl<S, OutP, const OUT: usize> SourceNode<S, OutP, OUT>
where
    S: Source<OutP, OUT>,
    OutP: Payload,
{
    /// Construct a `SourceNode` from a source and a static policy bundle.
    #[inline]
    pub const fn new(src: S, policy: NodePolicy) -> Self {
        Self {
            src,
            policy,
            _pd: PhantomData,
        }
    }

    /// Borrow the underlying source immutably.
    #[inline]
    pub fn source_ref(&self) -> &S {
        &self.src
    }

    /// Borrow the underlying source mutably.
    #[inline]
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.src
    }

    /// Return `true` if the ingress (external) edge for this source can
    /// produce a batch *now* under the given batching `policy`.
    ///
    /// This mirrors `StepContext::input_edge_has_batch` semantics and the
    /// `step_batch` checks: it is *observational* (no side effects) and
    /// conservative on header-peek failures.
    #[inline]
    pub fn ingress_edge_has_batch(&self) -> bool {
        // occupancy short-circuit
        let ingress_occ = self.source_ref().ingress_occupancy();
        if *ingress_occ.items() == 0 {
            return false;
        }

        let policy = self.policy.batching();

        let fixed_opt = *policy.fixed_n();
        let delta_opt = *policy.max_delta_t();

        match (fixed_opt, delta_opt) {
            (Some(fixed_n), None) => *ingress_occ.items() >= fixed_n,
            (None, Some(_max_delta_t)) => {
                // Delta-only: any non-empty ingress can form a size-1 batch.
                true
            }
            (Some(fixed_n), Some(max_delta_t)) => {
                // Must be able to form a full fixed_n batch first.
                if *ingress_occ.items() < fixed_n {
                    return false;
                }

                // Non-destructive peeks at creation ticks for the first and
                // the fixed_n-th ingress items.
                let first_tick_opt = self.src.peek_ingress_creation_tick(0);
                let last_tick_opt = self
                    .src
                    .peek_ingress_creation_tick(fixed_n.saturating_sub(1));

                match (first_tick_opt, last_tick_opt) {
                    (Some(first_ticks), Some(last_ticks)) => {
                        let span = last_ticks.saturating_sub(first_ticks);
                        span <= *max_delta_t.as_u64()
                    }
                    _ => false,
                }
            }
            (None, None) => {
                // No batching configured: treat as single-message readiness.
                true
            }
        }
    }
}

impl<S, OutP, const OUT: usize> Node<0, OUT, (), OutP> for SourceNode<S, OutP, OUT>
where
    S: Source<OutP, OUT>,
    OutP: Payload + Copy,
{
    #[inline]
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.src.capabilities()
    }

    #[inline]
    fn input_acceptance(&self) -> [PlacementAcceptance; 0] {
        []
    }

    #[inline]
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT] {
        self.src.output_acceptance()
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
        NodeKind::Source
    }

    #[inline]
    fn initialize<C, T>(&mut self, _c: &C, _t: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry,
    {
        self.src
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
    fn process_message<C>(
        &mut self,
        _msg: &Message<()>,
        _sys_clock: &C,
    ) -> Result<ProcessResult<OutP>, NodeError>
    where
        C: PlatformClock + Sized,
    {
        if let Some((_port, msg)) = self.src.try_produce() {
            Ok(ProcessResult::Output(msg))
        } else {
            Err(NodeError::no_input())
        }
    }

    #[inline]
    fn step<'g, 't, 'ck, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<'g, 't, 'ck, 0, OUT, (), OutP, InQ, OutQ, InM, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<()>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        if let Some((port, msg)) = self.src.try_produce() {
            ctx.push_output(port, msg)
        } else {
            Ok(StepResult::NoInput)
        }
    }

    fn step_batch<'graph, 'telemetry, 'clock, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<
            'graph,
            'telemetry,
            'clock,
            0,
            OUT,
            (),
            OutP,
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
        InM: MemoryManager<()>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        let ingress_occ = self.source_ref().ingress_occupancy();
        if *ingress_occ.items() == 0 {
            return Ok(StepResult::NoInput);
        }

        let policy = self.policy();

        let fixed_opt = *policy.batching().fixed_n();
        let delta_opt = *policy.batching().max_delta_t();

        let has_batch = match (fixed_opt, delta_opt) {
            (Some(fixed_n), None) => *ingress_occ.items() >= fixed_n,
            (None, Some(_max_delta_t)) => {
                // Span constraint only: a non-empty queue can always produce a
                // span-valid batch of size 1.
                true
            }
            (Some(fixed_n), Some(max_delta_t)) => {
                // Must be able to form a full fixed_n batch first.
                if *ingress_occ.items() < fixed_n {
                    false
                } else {
                    let first_tick_opt = self.src.peek_ingress_creation_tick(0);
                    let last_tick_opt = self
                        .src
                        .peek_ingress_creation_tick(fixed_n.saturating_sub(1));

                    match (first_tick_opt, last_tick_opt) {
                        (Some(first_ticks), Some(last_ticks)) => {
                            let span = last_ticks.saturating_sub(first_ticks);
                            span <= *max_delta_t.as_u64()
                        }
                        _ => false,
                    }
                }
            }
            (None, None) => {
                // No batching configured: treat as single-message readiness.
                true
            }
        };

        if !has_batch {
            return Ok(StepResult::NoInput);
        }

        let batch_n: usize = fixed_opt.unwrap_or(1);

        let mut made_progress = false;

        for _ in 0..batch_n {
            match self.src.try_produce() {
                Some((port, msg)) => match ctx.push_output(port, msg) {
                    Ok(StepResult::MadeProgress) => {
                        made_progress = true;
                    }
                    Ok(StepResult::Backpressured) | Err(_) => {
                        return Ok(StepResult::Backpressured);
                    }
                    Ok(_) => {}
                },
                None => {
                    break;
                }
            }
        }

        if made_progress {
            Ok(StepResult::MadeProgress)
        } else {
            Ok(StepResult::NoInput)
        }
    }

    #[inline]
    fn on_watchdog_timeout<C, Tel>(&mut self, _c: &C, _t: &mut Tel) -> Result<StepResult, NodeError>
    where
        Tel: Telemetry,
    {
        Ok(StepResult::WaitingOnExternal)
    }

    #[inline]
    fn stop<C, Tel>(&mut self, _c: &C, _t: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry,
    {
        Ok(())
    }
}

/// Borrowing adapter that exposes a source’s **ingress pressure** as an `Edge`.
///
/// This is used by the graph/builder to wire a synthetic "monitor edge" whose
/// occupancy is returned by `Source::ingress_occupancy()`. It rejects all push
/// and pop operations (no buffering); only `occupancy()` is meaningful.
///
/// This form is zero-allocation and suitable for `no_std`/single-threaded runs.
pub struct SourceIngressEdge<'src, OutP, S, const OUT: usize>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    /// Borrow to the underlying source.
    src: &'src S,
    /// Phantom to bind the `OutP` generic.
    _pd: PhantomData<OutP>,
}

impl<'src, OutP, S, const OUT: usize> SourceIngressEdge<'src, OutP, S, OUT>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    /// Create a borrowing ingress-edge view over a source.
    #[inline]
    pub const fn new(src: &'src S) -> Self {
        Self {
            src,
            _pd: PhantomData,
        }
    }
}

impl<'src, OutP, S, const OUT: usize> Edge for SourceIngressEdge<'src, OutP, S, OUT>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    #[inline]
    fn try_push<H: HeaderStore>(
        &mut self,
        _token: MessageToken,
        _policy: &EdgePolicy,
        _headers: &H,
    ) -> EnqueueResult {
        EnqueueResult::Rejected
    }

    #[inline]
    fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    #[inline]
    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    #[inline]
    fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    #[inline]
    fn occupancy(&self, _policy: &EdgePolicy) -> EdgeOccupancy {
        self.src.ingress_occupancy()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        *self.src.ingress_occupancy().items() == 0
    }

    #[inline]
    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        _policy: &BatchingPolicy,
        _headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        Err(QueueError::Empty)
    }
}

/// A tiny link wrapper for the synthetic ingress edge.
///
/// Wraps a borrowing `SourceIngressEdge<'s, OutP, S, OUT>` so the graph
/// can expose ingress pressure straight from the concrete source implementation.
pub struct IngressEdgeLink<'src, OutP, S, const OUT: usize>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    edge: SourceIngressEdge<'src, OutP, S, OUT>,
    id: EdgeIndex,
    upstream: PortId,
    downstream: PortId,
    policy: EdgePolicy,
    name: Option<&'static str>,
}

impl<'src, OutP, S, const OUT: usize> IngressEdgeLink<'src, OutP, S, OUT>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    /// Construct from a borrowed source reference.
    #[inline]
    pub const fn from_source(
        src: &'src S,
        id: EdgeIndex,
        upstream: PortId,
        downstream: PortId,
        policy: EdgePolicy,
        name: Option<&'static str>,
    ) -> Self {
        Self {
            edge: SourceIngressEdge::new(src),
            id,
            upstream,
            downstream,
            policy,
            name,
        }
    }

    /// Edge descriptor.
    #[inline]
    pub fn descriptor(&self) -> EdgeDescriptor {
        EdgeDescriptor::new(self.id, self.upstream, self.downstream, self.name)
    }

    /// Policy accessor.
    #[inline]
    pub fn policy(&self) -> EdgePolicy {
        self.policy
    }

    /// Borrow the inner borrowing edge.
    #[inline]
    pub fn inner(&self) -> &SourceIngressEdge<'src, OutP, S, OUT> {
        &self.edge
    }
}

impl<'s, OutP, S, const OUT: usize> Edge for IngressEdgeLink<'s, OutP, S, OUT>
where
    OutP: Payload,
    S: Source<OutP, OUT> + ?Sized,
{
    #[inline]
    fn try_push<H: HeaderStore>(
        &mut self,
        _token: MessageToken,
        _policy: &EdgePolicy,
        _headers: &H,
    ) -> EnqueueResult {
        EnqueueResult::Rejected
    }
    #[inline]
    fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }
    #[inline]
    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }
    #[inline]
    fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }
    #[inline]
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        // Delegate to the borrowing edge (reads Source::ingress_occupancy).
        self.edge.occupancy(policy)
    }
    #[inline]
    fn is_empty(&self) -> bool {
        self.edge.is_empty()
    }
    #[inline]
    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        _policy: &BatchingPolicy,
        _headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        Err(QueueError::Empty)
    }
}

/// Platform-agnostic ingress pressure observer.
///
/// Implemented by `NoProbe` (zero-cost no_std stub) and by
/// `probe::SourceIngressProbe` (live atomic counters, std-only).
/// Source node logic uses `IngressProbeImpl` uniformly — no `#[cfg]` in node bodies.
pub trait IngressProbe: Send {
    /// Return the current ingress occupancy snapshot using `policy` to compute
    /// the watermark consistently with real edges.
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy;
}

/// Zero-cost stub used on no_std targets where no real probe is wired up.
pub struct NoProbe;

impl IngressProbe for NoProbe {
    #[inline]
    fn occupancy(&self, _policy: &EdgePolicy) -> EdgeOccupancy {
        EdgeOccupancy::new(0, 0, WatermarkState::BelowSoft)
    }
}

/// Concrete ingress probe type used on `std` targets.
///
/// This resolves to [`probe::SourceIngressProbe`], which tracks ingress
/// pressure using shared atomic counters.
#[cfg(feature = "std")]
pub type IngressProbeImpl = probe::SourceIngressProbe;

/// Concrete ingress probe type used on `no_std` targets.
///
/// This resolves to [`NoProbe`], a zero-cost stub that reports no ingress
/// pressure when no live probe implementation is available.
#[cfg(not(feature = "std"))]
pub type IngressProbeImpl = NoProbe;

/// Std-only, lock-free ingress pressure probe for cross-thread sources.
#[cfg(feature = "std")]
pub mod probe {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Shared atomic counters for ingress pressure (items/bytes).
    #[derive(Clone, Debug)]
    pub struct SourceIngressProbe {
        items: Arc<AtomicUsize>,
        bytes: Arc<AtomicUsize>,
    }

    impl SourceIngressProbe {
        /// Create a new ingress probe with zeroed item and byte counters.
        #[inline]
        pub fn new() -> Self {
            Self {
                items: Arc::new(AtomicUsize::new(0)),
                bytes: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// Set the current ingress item count snapshot.
        #[inline]
        pub fn set_items(&self, n: usize) {
            self.items.store(n, Ordering::Relaxed);
        }

        /// Set the current ingress byte count snapshot.
        #[inline]
        pub fn set_bytes(&self, b: usize) {
            self.bytes.store(b, Ordering::Relaxed);
        }

        /// Build an occupancy snapshot from the current probe counters using
        /// `policy` to compute the watermark.
        #[inline]
        pub fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
            let items = self.items.load(Ordering::Relaxed);
            let bytes = self.bytes.load(Ordering::Relaxed);
            EdgeOccupancy::new(items, bytes, policy.watermark(items, bytes))
        }
    }

    impl Default for SourceIngressProbe {
        fn default() -> Self {
            Self {
                items: Arc::new(AtomicUsize::new(0)),
                bytes: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl super::IngressProbe for SourceIngressProbe {
        #[inline]
        fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
            SourceIngressProbe::occupancy(self, policy)
        }
    }

    /// Payload-typed wrapper that exposes a probe as an `Edge`.
    #[derive(Debug, Clone)]
    pub struct SourceIngressProbeEdge<P: Payload> {
        probe: SourceIngressProbe,
        _pd: PhantomData<P>,
    }

    impl<P: Payload> SourceIngressProbeEdge<P> {
        /// Wrap a probe as a payload-typed ingress monitor edge.
        #[inline]
        pub fn new(probe: SourceIngressProbe) -> Self {
            Self {
                probe,
                _pd: PhantomData,
            }
        }

        /// Borrow the underlying ingress probe.
        #[inline]
        pub fn inner(&self) -> &SourceIngressProbe {
            &self.probe
        }
    }

    impl<P: Payload> Edge for SourceIngressProbeEdge<P> {
        #[inline]
        fn try_push<H: HeaderStore>(
            &mut self,
            _token: MessageToken,
            _policy: &EdgePolicy,
            _headers: &H,
        ) -> EnqueueResult {
            EnqueueResult::Rejected
        }

        #[inline]
        fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }

        #[inline]
        fn try_peek(&self) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }

        #[inline]
        fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }

        #[inline]
        fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
            self.probe.occupancy(policy)
        }

        #[inline]
        fn is_empty(&self) -> bool {
            self.probe.items.load(core::sync::atomic::Ordering::Relaxed) == 0
        }

        #[inline]
        fn try_pop_batch<H: HeaderStore>(
            &mut self,
            _policy: &BatchingPolicy,
            _headers: &H,
        ) -> Result<BatchView<'_, MessageToken>, QueueError> {
            Err(QueueError::Empty)
        }
    }

    /// Cross-thread updater for a `SourceIngressProbe`.
    #[derive(Clone)]
    pub struct SourceIngressUpdater {
        probe: SourceIngressProbe,
    }

    impl SourceIngressUpdater {
        /// Create an updater for the given ingress probe.
        #[inline]
        pub fn new(probe: SourceIngressProbe) -> Self {
            Self { probe }
        }

        /// Update both ingress counters atomically from the caller's perspective.
        #[inline]
        pub fn update(&self, items: usize, bytes: usize) {
            self.probe.set_items(items);
            self.probe.set_bytes(bytes);
        }
    }

    /// Convenience: create a typed probe edge and its paired updater.
    #[inline]
    pub fn new_probe_edge_pair<P: Payload>() -> (SourceIngressProbeEdge<P>, SourceIngressUpdater) {
        let probe = SourceIngressProbe::new();
        let edge = SourceIngressProbeEdge::<P>::new(probe.clone());
        let updater = SourceIngressUpdater::new(probe);
        (edge, updater)
    }

    /// Convenience: create an untyped probe and updater.
    #[inline]
    pub fn new_probe_pair() -> (SourceIngressProbe, SourceIngressUpdater) {
        let p = SourceIngressProbe::new();
        (p.clone(), SourceIngressUpdater::new(p))
    }

    /// Link wrapper for a concurrent ingress monitor edge (std-only).
    #[derive(Debug)]
    pub struct ConcurrentIngressEdgeLink<OutP: Payload> {
        edge: SourceIngressProbeEdge<OutP>,
        id: EdgeIndex,
        upstream: PortId,
        downstream: PortId,
        policy: EdgePolicy,
        name: Option<&'static str>,
    }

    impl<OutP: Payload> ConcurrentIngressEdgeLink<OutP> {
        /// Construct a concurrent ingress edge link from a probe-backed edge and its
        /// descriptor metadata.
        #[inline]
        pub fn from_probe(
            probe_edge: SourceIngressProbeEdge<OutP>,
            id: EdgeIndex,
            upstream: PortId,
            downstream: PortId,
            policy: EdgePolicy,
            name: Option<&'static str>,
        ) -> Self {
            Self {
                edge: probe_edge,
                id,
                upstream,
                downstream,
                policy,
                name,
            }
        }

        /// Return the descriptor for this synthetic ingress edge.
        #[inline]
        pub fn descriptor(&self) -> EdgeDescriptor {
            EdgeDescriptor::new(self.id, self.upstream, self.downstream, self.name)
        }

        /// Return the policy associated with this synthetic ingress edge.
        #[inline]
        pub fn policy(&self) -> EdgePolicy {
            self.policy
        }

        /// Borrow the inner probe-backed edge immutably.
        #[inline]
        pub fn inner(&self) -> &SourceIngressProbeEdge<OutP> {
            &self.edge
        }

        /// Borrow the inner probe-backed edge mutably.
        #[inline]
        pub fn inner_mut(&mut self) -> &mut SourceIngressProbeEdge<OutP> {
            &mut self.edge
        }
    }

    impl<OutP: Payload> Edge for ConcurrentIngressEdgeLink<OutP> {
        #[inline]
        fn try_push<H: HeaderStore>(
            &mut self,
            _token: MessageToken,
            _policy: &EdgePolicy,
            _headers: &H,
        ) -> EnqueueResult {
            EnqueueResult::Rejected
        }
        #[inline]
        fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }
        #[inline]
        fn try_peek(&self) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }
        #[inline]
        fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
            Err(QueueError::Empty)
        }
        #[inline]
        fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
            self.edge.occupancy(policy)
        }
        #[inline]
        fn is_empty(&self) -> bool {
            self.edge.is_empty()
        }
        #[inline]
        fn try_pop_batch<H: HeaderStore>(
            &mut self,
            _policy: &BatchingPolicy,
            _headers: &H,
        ) -> Result<BatchView<'_, MessageToken>, QueueError> {
            Err(QueueError::Empty)
        }
    }
}
