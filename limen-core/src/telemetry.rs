//! Telemetry primitives for Limen runtimes.
//!
//! This module provides `no_std`-friendly metrics, structured telemetry events,
//! and timing spans that can be used by runtimes without imposing any logging,
//! allocation, or I/O policy.

pub mod event_message;
pub mod graph_telemetry;
pub mod sink;

#[cfg(feature = "std")]
pub mod concurrent;

use core::fmt;

use crate::policy::WatermarkState;
use crate::types::{EdgeIndex, NodeIndex};
use event_message::EventMessage;
use sink::write_u64;

// ====================== Core telemetry trait and keys ===================

/// Core interface for collecting runtime metrics and structured telemetry events.
///
/// This trait is intentionally minimal and I/O-agnostic so that it can be
/// implemented in both `no_std` and `std` environments. Implementations are free to
/// ignore any subset of calls.
pub trait Telemetry {
    /// Compile-time flag indicating whether this telemetry implementation
    /// wants metrics (counters, gauges, latencies) at all.
    ///
    /// Runtimes can use this to completely compile out metric collection
    /// when `METRICS_ENABLED` is `false` for a given `Telemetry` type.
    const METRICS_ENABLED: bool = true;

    /// Compile-time flag indicating whether this telemetry implementation
    /// ever produces structured events.
    ///
    /// When this is `false`, runtimes can skip both the construction of
    /// `TelemetryEvent` values and any calls to `events_enabled()`,
    /// allowing event handling code to compile out entirely.
    const EVENTS_STATICALLY_ENABLED: bool = true;

    /// Increment a counter metric identified by the given key.
    ///
    /// Counters are monotonically increasing and are typically used for counts such
    /// as processed messages, dropped messages, or deadline misses.
    fn incr_counter(&mut self, key: TelemetryKey, delta: u64);

    /// Set a gauge metric identified by the given key.
    ///
    /// Gauges represent the latest value of a quantity such as queue depth or
    /// current occupancy.
    fn set_gauge(&mut self, key: TelemetryKey, value: u64);

    /// Record a latency sample in nanoseconds for the given key.
    ///
    /// Implementations are free to aggregate these values as histograms, rolling
    /// averages, or to ignore them.
    fn record_latency_ns(&mut self, key: TelemetryKey, value_ns: u64);

    /// Optional: push a snapshot of aggregated metrics to the sink.
    ///
    /// Runtimes can call this periodically without knowing how metrics are
    /// stored. Implementations that have no aggregated metrics can keep the
    /// default no-op.
    #[inline]
    fn push_metrics(&mut self) {}

    /// Return true if this telemetry collector wants structured events.
    ///
    /// Runtimes and nodes can use this to avoid constructing `TelemetryEvent`
    /// values when events are disabled, keeping the hot path as cheap as possible.
    #[inline]
    fn events_enabled(&self) -> bool {
        false
    }

    /// Emit a structured telemetry event.
    ///
    /// The default implementation is a no operation so that simple collectors can
    /// ignore structured events entirely.
    #[inline]
    fn push_event(&mut self, _event: TelemetryEvent) {}

    /// Flush any buffered telemetry data to the underlying sink.
    ///
    /// The default implementation is a no operation. Implementations that buffer
    /// data or write to external sinks can use this to force a drain.
    #[inline]
    fn flush(&mut self) {}
}

/// Compact identifier for a metric or latency sample.
///
/// Keys combine a namespace, an integer identifier, and a logical kind so that
/// backends can map them to their own internal representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TelemetryKey {
    /// Namespace that this key belongs to (node, edge, or runtime).
    ns: TelemetryNs,
    /// Integer identifier within the namespace (for example node index).
    id: u32,
    /// Logical kind of metric represented by this key.
    kind: TelemetryKind,
}

impl TelemetryKey {
    /// Construct a key for a node level metric.
    ///
    /// The `node_id` is the zero based index of the node in the graph.
    #[inline]
    pub const fn node(node_id: u32, kind: TelemetryKind) -> Self {
        Self {
            ns: TelemetryNs::Node,
            id: node_id,
            kind,
        }
    }

    /// Construct a key for an edge level metric.
    ///
    /// The `edge_id` is the zero based index of the edge in the graph.
    #[inline]
    pub const fn edge(edge_id: u32, kind: TelemetryKind) -> Self {
        Self {
            ns: TelemetryNs::Edge,
            id: edge_id,
            kind,
        }
    }

    /// Construct a key for a runtime level metric.
    ///
    /// The identifier is currently always zero and reserved for future use.
    #[inline]
    pub const fn runtime(kind: TelemetryKind) -> Self {
        Self {
            ns: TelemetryNs::Runtime,
            id: 0,
            kind,
        }
    }

    /// Construct a compact key for a node port metric.
    ///
    /// The identifier encodes the node identifier, the port index, and whether
    /// this is an input or output port into a single integer.
    #[inline]
    pub const fn node_port(
        node_id: u32,
        port_index: u16,
        is_output: bool,
        kind: TelemetryKind,
    ) -> Self {
        let enc = ((node_id & 0x000F_FFFF) << 12)
            | (((is_output as u32) & 0x1) << 11)
            | (port_index as u32 & 0x7FF);
        Self {
            ns: TelemetryNs::Node,
            id: enc,
            kind,
        }
    }

    /// Return the namespace.
    #[inline]
    pub const fn ns(&self) -> &TelemetryNs {
        &self.ns
    }

    /// Return the identifier.
    #[inline]
    pub const fn id(&self) -> &u32 {
        &self.id
    }

    /// Return the logical kind.
    #[inline]
    pub const fn kind(&self) -> &TelemetryKind {
        &self.kind
    }
}

/// Logical namespace for telemetry keys.
///
/// Separates metrics that describe nodes, edges, and the runtime itself.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelemetryNs {
    /// Node level metrics such as processed counts and latencies.
    Node,
    /// Edge level metrics such as queue depth.
    Edge,
    /// Runtime level metrics that are not tied to a particular node or edge.
    Runtime,
}

/// Logical kind of metric represented by a telemetry key.
///
/// These kinds are interpreted by collectors such as fixed and dynamic telemetry
/// stores and are intentionally small and generic.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelemetryKind {
    /// Number of items successfully processed.
    Processed,
    /// Number of items dropped or discarded.
    Dropped,
    /// Number of deadline misses observed.
    DeadlineMiss,
    /// Current depth of a queue or buffer.
    QueueDepth,
    /// Latency measurement in nanoseconds.
    Latency,
    /// Number of ingress messages received.
    IngressMsgs,
    /// Number of egress messages emitted.
    EgressMsgs,
}

/// Unique identifier for a running graph instance.
///
/// This allows telemetry to distinguish between multiple graphs managed by the
/// same runtime.
pub type GraphInstanceId = u32;

/// High level classification of a node level error used in telemetry.
///
/// This mirrors `crate::errors::NodeErrorKind` so that telemetry can
/// faithfully report the scheduler-visible error semantics without
/// inventing additional information that is not available at this layer.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum NodeStepError {
    /// Inputs were not available to progress this node.
    NoInput,
    /// Outputs could not be enqueued due to backpressure.
    Backpressured,
    /// An execution budget or deadline was exceeded.
    OverBudget,
    /// External dependency (device, transport) was unavailable or timed out.
    ExternalUnavailable,
    /// A generic failure in node logic.
    ExecutionFailed,
}

/// Structured telemetry produced for each node step.
///
/// A node step represents a single scheduling decision in which a node consumes
/// zero or more input messages and produces zero or more output messages.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub struct NodeStepTelemetry {
    /// Identifier of the graph instance this node belongs to.
    graph_id: GraphInstanceId,
    /// Index of the node within the graph.
    node_index: NodeIndex,
    /// Optional static node name for debugging and correlation.
    node_name: Option<&'static str>,

    /// Start timestamp of the step in nanoseconds since an arbitrary epoch.
    timestamp_start_ns: u64,
    /// End timestamp of the step in nanoseconds since an arbitrary epoch.
    timestamp_end_ns: u64,
    /// Duration of the step in nanoseconds.
    duration_ns: u64,

    /// Number of messages processed by this step (batch size or 1 for single-message).
    processed_count: u64,

    /// Optional absolute deadline in nanoseconds for this step.
    deadline_ns: Option<u64>,
    /// Whether the deadline was missed during this step.
    deadline_missed: bool,

    /// Optional high level error classification for this step.
    error_kind: Option<NodeStepError>,
}

impl NodeStepTelemetry {
    /// Construct a new `NodeStepTelemetry` record.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        graph_id: GraphInstanceId,
        node_index: NodeIndex,
        node_name: Option<&'static str>,
        timestamp_start_ns: u64,
        timestamp_end_ns: u64,
        duration_ns: u64,
        processed_count: u64,
        deadline_ns: Option<u64>,
        deadline_missed: bool,
        error_kind: Option<NodeStepError>,
    ) -> Self {
        Self {
            graph_id,
            node_index,
            node_name,
            timestamp_start_ns,
            timestamp_end_ns,
            duration_ns,
            processed_count,
            deadline_ns,
            deadline_missed,
            error_kind,
        }
    }

    /// Returns the identifier of the graph instance this step belongs to.
    #[inline]
    pub const fn graph_id(&self) -> &GraphInstanceId {
        &self.graph_id
    }

    /// Returns the index of the node within the graph.
    #[inline]
    pub const fn node_index(&self) -> &NodeIndex {
        &self.node_index
    }

    /// Returns the optional static node name associated with this step.
    #[inline]
    pub const fn node_name(&self) -> &Option<&'static str> {
        &self.node_name
    }

    /// Returns the step start timestamp in nanoseconds since an arbitrary epoch.
    #[inline]
    pub const fn timestamp_start_ns(&self) -> &u64 {
        &self.timestamp_start_ns
    }

    /// Returns the step end timestamp in nanoseconds since an arbitrary epoch.
    #[inline]
    pub const fn timestamp_end_ns(&self) -> &u64 {
        &self.timestamp_end_ns
    }

    /// Returns the step duration in nanoseconds.
    #[inline]
    pub const fn duration_ns(&self) -> &u64 {
        &self.duration_ns
    }

    /// Returns the number of messages processed by this step.
    #[inline]
    pub const fn processed_count(&self) -> &u64 {
        &self.processed_count
    }

    /// Returns the optional absolute deadline for this step in nanoseconds.
    #[inline]
    pub const fn deadline_ns(&self) -> &Option<u64> {
        &self.deadline_ns
    }

    /// Returns whether the step exceeded its deadline.
    #[inline]
    pub const fn deadline_missed(&self) -> &bool {
        &self.deadline_missed
    }

    /// Returns the optional high-level error classification for this step.
    #[inline]
    pub const fn error_kind(&self) -> &Option<NodeStepError> {
        &self.error_kind
    }
}

/// Structured snapshot describing the state of a single edge.
///
/// These snapshots are typically taken by runtimes when they want to record
/// backpressure or queue depth for a link between nodes.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub struct EdgeSnapshotTelemetry {
    /// Identifier of the graph instance this edge belongs to.
    graph_id: GraphInstanceId,
    /// Index of the edge within the graph.
    edge_index: EdgeIndex,
    /// Index of the source node for this edge.
    source_node_index: NodeIndex,
    /// Index of the target node for this edge.
    target_node_index: NodeIndex,

    /// Timestamp of the snapshot in nanoseconds since an arbitrary epoch.
    timestamp_ns: u64,
    /// Current occupancy of the edge buffer.
    current_occupancy: u32,
    /// Configured soft watermark for this edge.
    soft_watermark: u32,
    /// Configured hard watermark for this edge.
    hard_watermark: u32,
    /// Current watermark state relative to the configured thresholds.
    watermark_state: WatermarkState,
}

impl EdgeSnapshotTelemetry {
    /// Creates a new snapshot record for a single edge.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        graph_id: GraphInstanceId,
        edge_index: EdgeIndex,
        source_node_index: NodeIndex,
        target_node_index: NodeIndex,
        timestamp_ns: u64,
        current_occupancy: u32,
        soft_watermark: u32,
        hard_watermark: u32,
        watermark_state: WatermarkState,
    ) -> Self {
        Self {
            graph_id,
            edge_index,
            source_node_index,
            target_node_index,
            timestamp_ns,
            current_occupancy,
            soft_watermark,
            hard_watermark,
            watermark_state,
        }
    }

    /// Returns the identifier of the graph instance this edge belongs to.
    #[inline]
    pub const fn graph_id(&self) -> &GraphInstanceId {
        &self.graph_id
    }

    /// Returns the index of the edge within the graph.
    #[inline]
    pub const fn edge_index(&self) -> &EdgeIndex {
        &self.edge_index
    }

    /// Returns the index of the source node for this edge.
    #[inline]
    pub const fn source_node_index(&self) -> &NodeIndex {
        &self.source_node_index
    }

    /// Returns the index of the target node for this edge.
    #[inline]
    pub const fn target_node_index(&self) -> &NodeIndex {
        &self.target_node_index
    }

    /// Returns the snapshot timestamp in nanoseconds since an arbitrary epoch.
    #[inline]
    pub const fn timestamp_ns(&self) -> &u64 {
        &self.timestamp_ns
    }

    /// Returns the current occupancy of the edge buffer.
    #[inline]
    pub const fn current_occupancy(&self) -> &u32 {
        &self.current_occupancy
    }

    /// Returns the configured soft watermark for this edge.
    #[inline]
    pub const fn soft_watermark(&self) -> &u32 {
        &self.soft_watermark
    }

    /// Returns the configured hard watermark for this edge.
    #[inline]
    pub const fn hard_watermark(&self) -> &u32 {
        &self.hard_watermark
    }

    /// Returns the current watermark state relative to the configured thresholds.
    #[inline]
    pub const fn watermark_state(&self) -> &WatermarkState {
        &self.watermark_state
    }
}

/// Classification of runtime level events that are not tied to a single node.
///
/// These events are useful for monitoring graph lifecycle, connectivity, and
/// data quality issues.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum RuntimeTelemetryEventKind {
    /// A graph instance has started running.
    GraphStarted,
    /// A graph instance has stopped cleanly.
    GraphStopped,
    /// A graph instance has panicked or aborted unexpectedly.
    GraphPanicked,
    /// A sensor connection has been lost.
    SensorDisconnected,
    /// A sensor connection has been reestablished.
    SensorRecovered,
    /// A model failed to load or initialize.
    ModelLoadFailed,
    /// A model has recovered after a previous failure.
    ModelRecovered,
    /// The message broker connection has been lost.
    MqttDisconnected,
    /// The message broker connection has been reestablished.
    MqttRecovered,
    /// A gap in the input data stream has been detected.
    DataGapDetected,
    /// Invalid or malformed data has been observed.
    InvalidDataSeen,
}

/// Structured runtime level telemetry event.
///
/// These events describe lifecycle transitions, connectivity changes, and
/// coarse grained data quality issues at the graph level.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub struct RuntimeTelemetryEvent {
    /// Identifier of the graph instance this event refers to.
    graph_id: GraphInstanceId,
    /// Timestamp of the event in nanoseconds since an arbitrary epoch.
    timestamp_ns: u64,
    /// Kind of runtime event that occurred.
    event_kind: RuntimeTelemetryEventKind,
    /// Optional static message with additional context.
    ///
    /// NOTE: `message` is rendered as the final `msg=` field in `fmt_event`.
    /// It must not contain newlines; spaces are allowed and are treated
    /// as part of the message up to end-of-line.
    message: Option<EventMessage>,
}

impl RuntimeTelemetryEvent {
    /// Creates a new runtime telemetry event record.
    #[inline]
    pub const fn new(
        graph_id: GraphInstanceId,
        timestamp_ns: u64,
        event_kind: RuntimeTelemetryEventKind,
        message: Option<EventMessage>,
    ) -> Self {
        Self {
            graph_id,
            timestamp_ns,
            event_kind,
            message,
        }
    }

    /// Returns the identifier of the graph instance this event refers to.
    #[inline]
    pub const fn graph_id(&self) -> &GraphInstanceId {
        &self.graph_id
    }

    /// Returns the event timestamp in nanoseconds since an arbitrary epoch.
    #[inline]
    pub const fn timestamp_ns(&self) -> &u64 {
        &self.timestamp_ns
    }

    /// Returns the kind of runtime event that occurred.
    #[inline]
    pub const fn event_kind(&self) -> &RuntimeTelemetryEventKind {
        &self.event_kind
    }

    /// Returns the optional static message associated with this event.
    #[inline]
    pub const fn message(&self) -> &Option<EventMessage> {
        &self.message
    }
}

/// Discriminated union of all structured telemetry events.
///
/// This is the type carried by event writers and is the payload for all
/// structured telemetry emission.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum TelemetryEvent {
    /// Node level timing and throughput information for a single step.
    NodeStep(NodeStepTelemetry),
    /// Edge level snapshot representing queue state and watermarks.
    EdgeSnapshot(EdgeSnapshotTelemetry),
    /// Runtime level lifecycle and connectivity event.
    Runtime(RuntimeTelemetryEvent),
}

impl TelemetryEvent {
    /// Creates a telemetry event from a node step telemetry record.
    #[inline]
    pub const fn node_step(ev: NodeStepTelemetry) -> Self {
        TelemetryEvent::NodeStep(ev)
    }

    /// Creates a telemetry event from an edge snapshot telemetry record.
    #[inline]
    pub const fn edge_snapshot(ev: EdgeSnapshotTelemetry) -> Self {
        TelemetryEvent::EdgeSnapshot(ev)
    }

    /// Creates a telemetry event from a runtime telemetry event record.
    #[inline]
    pub const fn runtime(ev: RuntimeTelemetryEvent) -> Self {
        TelemetryEvent::Runtime(ev)
    }
}

/// Per node metrics aggregated by fixed and dynamic collectors.
///
/// These metrics are updated via the `Telemetry` trait and represent simple
/// counters and latency aggregates.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct NodeMetrics {
    /// Number of items successfully processed by the node.
    processed: u64,
    /// Number of items dropped by the node (including deadline misses).
    dropped: u64,
    /// Number of ingress messages observed by the node.
    ingress: u64,
    /// Number of egress messages emitted by the node.
    egress: u64,
    /// Sum of all recorded latencies in nanoseconds.
    lat_sum: u64,
    /// Number of latency samples recorded.
    lat_cnt: u64,
    /// Maximum latency observed in nanoseconds.
    lat_max: u64,
    /// Number of deadline misses observed for this node.
    deadline_miss_count: u64,
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeMetrics {
    /// Create a new zero initialized metrics record.
    pub const fn new() -> Self {
        Self {
            processed: 0,
            dropped: 0,
            ingress: 0,
            egress: 0,
            lat_sum: 0,
            lat_cnt: 0,
            lat_max: 0,
            deadline_miss_count: 0,
        }
    }

    /// Returns the number of items successfully processed by the node.
    #[inline]
    pub const fn processed(&self) -> &u64 {
        &self.processed
    }

    /// Returns the number of items dropped by the node.
    #[inline]
    pub const fn dropped(&self) -> &u64 {
        &self.dropped
    }

    /// Returns the number of ingress messages observed by the node.
    #[inline]
    pub const fn ingress(&self) -> &u64 {
        &self.ingress
    }

    /// Returns the number of egress messages emitted by the node.
    #[inline]
    pub const fn egress(&self) -> &u64 {
        &self.egress
    }

    /// Returns the sum of all recorded latencies in nanoseconds.
    #[inline]
    pub const fn lat_sum(&self) -> &u64 {
        &self.lat_sum
    }

    /// Returns the number of latency samples recorded.
    #[inline]
    pub const fn lat_cnt(&self) -> &u64 {
        &self.lat_cnt
    }

    /// Returns the maximum latency observed in nanoseconds.
    #[inline]
    pub const fn lat_max(&self) -> &u64 {
        &self.lat_max
    }

    /// Returns the number of deadline misses observed for this node.
    #[inline]
    pub const fn deadline_miss_count(&self) -> &u64 {
        &self.deadline_miss_count
    }

    /// Increment `processed` by `delta` (saturating).
    #[inline]
    pub fn inc_processed(&mut self, delta: u64) {
        self.processed = self.processed.saturating_add(delta);
    }

    /// Subtract from `processed` (saturating at zero).
    #[inline]
    pub fn dec_processed(&mut self, delta: u64) {
        self.processed = self.processed.saturating_sub(delta);
    }

    /// Set `processed` to `v`.
    #[inline]
    pub fn set_processed(&mut self, v: u64) {
        self.processed = v;
    }

    /// Increment `dropped` by `delta` (saturating).
    #[inline]
    pub fn inc_dropped(&mut self, delta: u64) {
        self.dropped = self.dropped.saturating_add(delta);
    }

    /// Subtract from `dropped` (saturating at zero).
    #[inline]
    pub fn dec_dropped(&mut self, delta: u64) {
        self.dropped = self.dropped.saturating_sub(delta);
    }

    /// Set `dropped` to `v`.
    #[inline]
    pub fn set_dropped(&mut self, v: u64) {
        self.dropped = v;
    }

    /// Increment `ingress` by `delta` (saturating).
    #[inline]
    pub fn inc_ingress(&mut self, delta: u64) {
        self.ingress = self.ingress.saturating_add(delta);
    }

    /// Subtract from `ingress` (saturating at zero).
    #[inline]
    pub fn dec_ingress(&mut self, delta: u64) {
        self.ingress = self.ingress.saturating_sub(delta);
    }

    /// Set `ingress` to `v`.
    #[inline]
    pub fn set_ingress(&mut self, v: u64) {
        self.ingress = v;
    }

    /// Increment `egress` by `delta` (saturating).
    #[inline]
    pub fn inc_egress(&mut self, delta: u64) {
        self.egress = self.egress.saturating_add(delta);
    }

    /// Subtract from `egress` (saturating at zero).
    #[inline]
    pub fn dec_egress(&mut self, delta: u64) {
        self.egress = self.egress.saturating_sub(delta);
    }

    /// Set `egress` to `v`.
    #[inline]
    pub fn set_egress(&mut self, v: u64) {
        self.egress = v;
    }

    /// Record a latency sample in nanoseconds.
    ///
    /// This updates `lat_sum`, `lat_cnt`, and `lat_max`. Uses saturating
    /// addition to avoid overflow on long running systems.
    #[inline]
    pub fn record_latency_ns(&mut self, value_ns: u64) {
        self.lat_sum = self.lat_sum.saturating_add(value_ns);
        self.lat_cnt = self.lat_cnt.saturating_add(1);
        if value_ns > self.lat_max {
            self.lat_max = value_ns;
        }
    }

    /// Merge another `NodeMetrics` into `self`. This uses saturating addition
    /// for counters and takes the maximum for latency max.
    #[inline]
    pub fn merge_from(&mut self, other: &Self) {
        self.processed = self.processed.saturating_add(other.processed);
        self.dropped = self.dropped.saturating_add(other.dropped);
        self.ingress = self.ingress.saturating_add(other.ingress);
        self.egress = self.egress.saturating_add(other.egress);
        self.lat_sum = self.lat_sum.saturating_add(other.lat_sum);
        self.lat_cnt = self.lat_cnt.saturating_add(other.lat_cnt);
        if other.lat_max > self.lat_max {
            self.lat_max = other.lat_max;
        }
        self.deadline_miss_count = self
            .deadline_miss_count
            .saturating_add(other.deadline_miss_count);
    }

    /// Increment the deadline miss counter by `delta` (saturating).
    #[inline]
    pub fn inc_deadline_miss_count(&mut self, delta: u64) {
        self.deadline_miss_count = self.deadline_miss_count.saturating_add(delta);
    }

    /// Reset all counters and aggregates to zero.
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Per edge metrics aggregated by fixed and dynamic collectors.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct EdgeMetrics {
    /// Current queue depth for the edge.
    queue_depth: u32,
}

impl Default for EdgeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeMetrics {
    /// Create a new metrics record with a queue depth of zero.
    pub const fn new() -> Self {
        Self { queue_depth: 0 }
    }

    /// Returns the current queue depth for the edge.
    #[inline]
    pub const fn queue_depth(&self) -> &u32 {
        &self.queue_depth
    }

    /// Sets the current queue depth for the edge.
    #[inline]
    pub fn set_queue_depth(&mut self, v: u32) {
        self.queue_depth = v;
    }

    /// Increment queue depth by `delta` (saturating).
    #[inline]
    pub fn inc_queue_depth(&mut self, delta: u32) {
        self.queue_depth = self.queue_depth.saturating_add(delta);
    }

    /// Decrement queue depth by `delta` (saturating at zero).
    #[inline]
    pub fn dec_queue_depth(&mut self, delta: u32) {
        self.queue_depth = self.queue_depth.saturating_sub(delta);
    }

    /// Merge another `EdgeMetrics` into `self`. For queue depth we follow the
    /// last-writer-wins semantics used by higher-level merge logic.
    #[inline]
    pub fn merge_from(&mut self, other: &Self) {
        self.queue_depth = other.queue_depth;
    }

    /// Reset all counters and aggregates to zero.
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Per graph telemetry metrics.
#[derive(Debug, Clone, Copy)]
pub struct GraphMetrics<const MAX_NODES: usize, const MAX_EDGES: usize> {
    /// Graph id.
    id: u32,
    /// Per-node metrics.
    nodes: [NodeMetrics; MAX_NODES],
    /// Per-edge metrics.
    edges: [EdgeMetrics; MAX_EDGES],
}

impl<const MAX_NODES: usize, const MAX_EDGES: usize> GraphMetrics<MAX_NODES, MAX_EDGES> {
    /// Create a new metrics record with a queue depth of zero.
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            nodes: [NodeMetrics::new(); MAX_NODES],
            edges: [EdgeMetrics::new(); MAX_EDGES],
        }
    }

    /// Return the identifier of this graph.
    pub fn id(&self) -> &u32 {
        &self.id
    }

    /// Return the metrics for all nodes.
    pub fn nodes(&self) -> &[NodeMetrics; MAX_NODES] {
        &self.nodes
    }

    /// Return the metrics for all edges.
    pub fn edges(&self) -> &[EdgeMetrics; MAX_EDGES] {
        &self.edges
    }
}

impl<const MAX_NODES: usize, const MAX_EDGES: usize> GraphMetrics<MAX_NODES, MAX_EDGES> {
    /// Format this graph metrics record as multi-line text.
    ///
    /// The first line contains the graph identifier. Each subsequent line
    /// contains metrics for a single node or edge, indented by two spaces and
    /// prefixed with `node id:` or `edge id:` respectively.
    pub fn fmt<W: fmt::Write>(&self, w: &mut W) -> fmt::Result {
        // First line: graph id
        w.write_str("graph id: ")?;
        write_u64(w, self.id as u64)?;
        w.write_str("\n")?;

        // Nodes
        for i in 0..MAX_NODES {
            let m = &self.nodes[i];
            w.write_str("  node id: ")?;
            write_u64(w, i as u64)?;
            w.write_str(" processed=")?;
            write_u64(w, m.processed)?;
            w.write_str(" dropped=")?;
            write_u64(w, m.dropped)?;
            w.write_str(" ingress=")?;
            write_u64(w, m.ingress)?;
            w.write_str(" egress=")?;
            write_u64(w, m.egress)?;
            w.write_str(" lat_sum=")?;
            write_u64(w, m.lat_sum)?;
            w.write_str(" lat_cnt=")?;
            write_u64(w, m.lat_cnt)?;
            w.write_str(" lat_max=")?;
            write_u64(w, m.lat_max)?;
            w.write_str(" deadline_miss_count=")?;
            write_u64(w, m.deadline_miss_count)?;
            w.write_str("\n")?;
        }

        // Edges
        for i in 0..MAX_EDGES {
            let m = &self.edges[i];
            w.write_str("  edge id: ")?;
            write_u64(w, i as u64)?;
            w.write_str(" queue_depth=")?;
            write_u64(w, m.queue_depth as u64)?;
            w.write_str("\n")?;
        }

        Ok(())
    }
}

/// Telemetry implementation that discards all metrics and events.
///
/// This is useful in tests and extremely constrained environments where
/// telemetry is not required.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopTelemetry;

impl Telemetry for NoopTelemetry {
    const METRICS_ENABLED: bool = false;
    const EVENTS_STATICALLY_ENABLED: bool = false;

    #[inline]
    fn incr_counter(&mut self, _key: TelemetryKey, _delta: u64) {}
    #[inline]
    fn set_gauge(&mut self, _key: TelemetryKey, _value: u64) {}
    #[inline]
    fn record_latency_ns(&mut self, _key: TelemetryKey, _value_ns: u64) {}
}

impl Telemetry for () {
    const METRICS_ENABLED: bool = false;
    const EVENTS_STATICALLY_ENABLED: bool = false;

    #[inline]
    fn incr_counter(&mut self, _key: TelemetryKey, _delta: u64) {}

    #[inline]
    fn set_gauge(&mut self, _key: TelemetryKey, _value: u64) {}

    #[inline]
    fn record_latency_ns(&mut self, _key: TelemetryKey, _value_ns: u64) {}
}
