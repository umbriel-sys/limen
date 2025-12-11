//! Telemetry primitives for Limen runtimes.
//!
//! This module provides no_std friendly metrics, structured telemetry events,
//! and timing spans that can be used by runtimes without imposing any logging,
//! allocation, or input output policy.

pub mod event_message;
pub mod graph_telemetry;
pub mod sink;

#[cfg(feature = "std")]
pub mod concurrent;

#[cfg(feature = "alloc")]
extern crate alloc;

use core::fmt;

use crate::policy::WatermarkState;
use crate::types::{EdgeIndex, NodeIndex};
use event_message::EventMessage;
use sink::write_u64;

// ====================== Core telemetry trait and keys ===================

/// Core interface for collecting runtime metrics and structured telemetry events.
///
/// This trait is intentionally minimal and input output agnostic so that it can be
/// implemented in both no_std and std environments. Implementations are free to
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
    pub ns: TelemetryNs,
    /// Integer identifier within the namespace (for example node index).
    pub id: u32,
    /// Logical kind of metric represented by this key.
    pub kind: TelemetryKind,
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
}

/// Logical namespace for telemetry keys.
///
/// Separates metrics that describe nodes, edges, and the runtime itself.
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
#[derive(Copy, Clone, Debug)]
pub struct NodeStepTelemetry {
    /// Identifier of the graph instance this node belongs to.
    pub graph_id: GraphInstanceId,
    /// Index of the node within the graph.
    pub node_index: NodeIndex,
    /// Optional static node name for debugging and correlation.
    pub node_name: Option<&'static str>,

    /// Start timestamp of the step in nanoseconds since an arbitrary epoch.
    pub timestamp_start_ns: u64,
    /// End timestamp of the step in nanoseconds since an arbitrary epoch.
    pub timestamp_end_ns: u64,
    /// Duration of the step in nanoseconds.
    pub duration_ns: u64,

    /// Optional absolute deadline in nanoseconds for this step.
    pub deadline_ns: Option<u64>,
    /// Whether the deadline was missed during this step.
    pub deadline_missed: bool,

    /// Optional high level error classification for this step.
    pub error_kind: Option<NodeStepError>,
}

/// Structured snapshot describing the state of a single edge.
///
/// These snapshots are typically taken by runtimes when they want to record
/// backpressure or queue depth for a link between nodes.
#[derive(Copy, Clone, Debug)]
pub struct EdgeSnapshotTelemetry {
    /// Identifier of the graph instance this edge belongs to.
    pub graph_id: GraphInstanceId,
    /// Index of the edge within the graph.
    pub edge_index: EdgeIndex,
    /// Index of the source node for this edge.
    pub source_node_index: NodeIndex,
    /// Index of the target node for this edge.
    pub target_node_index: NodeIndex,

    /// Timestamp of the snapshot in nanoseconds since an arbitrary epoch.
    pub timestamp_ns: u64,
    /// Current occupancy of the edge buffer.
    pub current_occupancy: u32,
    /// Configured soft watermark for this edge.
    pub soft_watermark: u32,
    /// Configured hard watermark for this edge.
    pub hard_watermark: u32,
    /// Current watermark state relative to the configured thresholds.
    pub watermark_state: WatermarkState,
}

/// Classification of runtime level events that are not tied to a single node.
///
/// These events are useful for monitoring graph lifecycle, connectivity, and
/// data quality issues.
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
#[derive(Copy, Clone, Debug)]
pub struct RuntimeTelemetryEvent {
    /// Identifier of the graph instance this event refers to.
    pub graph_id: GraphInstanceId,
    /// Timestamp of the event in nanoseconds since an arbitrary epoch.
    pub timestamp_ns: u64,
    /// Kind of runtime event that occurred.
    pub event_kind: RuntimeTelemetryEventKind,
    /// Optional static message with additional context.
    ///
    /// NOTE: `message` is rendered as the final `msg=` field in `fmt_event`.
    /// It must not contain newlines; spaces are allowed and are treated
    /// as part of the message up to end-of-line.
    pub message: Option<EventMessage>,
}

/// Discriminated union of all structured telemetry events.
///
/// This is the type carried by event writers and is the payload for all
/// structured telemetry emission.
#[derive(Copy, Clone, Debug)]
pub enum TelemetryEvent {
    /// Node level timing and throughput information for a single step.
    NodeStep(NodeStepTelemetry),
    /// Edge level snapshot representing queue state and watermarks.
    EdgeSnapshot(EdgeSnapshotTelemetry),
    /// Runtime level lifecycle and connectivity event.
    Runtime(RuntimeTelemetryEvent),
}

/// Per node metrics aggregated by fixed and dynamic collectors.
///
/// These metrics are updated via the `Telemetry` trait and represent simple
/// counters and latency aggregates.
#[derive(Debug, Clone, Copy)]
pub struct NodeMetrics {
    /// Number of items successfully processed by the node.
    pub processed: u64,
    /// Number of items dropped by the node (including deadline misses).
    pub dropped: u64,
    /// Number of ingress messages observed by the node.
    pub ingress: u64,
    /// Number of egress messages emitted by the node.
    pub egress: u64,
    /// Sum of all recorded latencies in nanoseconds.
    pub lat_sum: u64,
    /// Number of latency samples recorded.
    pub lat_cnt: u64,
    /// Maximum latency observed in nanoseconds.
    pub lat_max: u64,
    /// Number of deadline misses observed for this node.
    pub deadline_miss_count: u64,
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
}

/// Per edge metrics aggregated by fixed and dynamic collectors.
#[derive(Debug, Clone, Copy)]
pub struct EdgeMetrics {
    /// Current queue depth for the edge.
    pub queue_depth: u32,
}

impl EdgeMetrics {
    /// Create a new metrics record with a queue depth of zero.
    pub const fn new() -> Self {
        Self { queue_depth: 0 }
    }
}

/// Per graph telemetry metrics.
#[derive(Debug, Clone, Copy)]
pub struct GraphMetrics<const MAX_NODES: usize, const MAX_EDGES: usize> {
    /// Graph id.
    pub id: u32,
    /// graoh nodes.
    pub nodes: [NodeMetrics; MAX_NODES],
    /// graoh edges.
    pub edges: [EdgeMetrics; MAX_EDGES],
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
