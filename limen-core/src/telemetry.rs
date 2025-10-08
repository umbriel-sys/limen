//! Telemetry interfaces for counters, histograms, and spans.
//!
//! The goal is to keep the cost minimal by default (P0/P1) and enable richer
//! metrics/tracing in P2. Implementations are provided by runtimes.

/// Minimal counters expected across profiles.
pub trait Telemetry {
    /// Increment a counter identified by an implementation-defined key.
    fn incr_counter(&mut self, key: TelemetryKey, delta: u64);

    /// Record a gauge value.
    fn set_gauge(&mut self, key: TelemetryKey, value: u64);

    /// Record a latency value in nanoseconds to a histogram (if available).
    fn record_latency_ns(&mut self, key: TelemetryKey, value_ns: u64);
}

/// A compact key for counters, gauges, and histograms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TelemetryKey {
    /// Namespace (e.g., "node", "edge").
    pub ns: TelemetryNs,
    /// Identifier within the namespace (node index, edge id, etc.).
    pub id: u32,
    /// Metric kind (e.g., "processed", "dropped", "latency").
    pub kind: TelemetryKind,
}

impl TelemetryKey {
    /// Convenience: key for a node-scoped metric.
    #[inline]
    pub const fn node(node_id: u32, kind: TelemetryKind) -> Self {
        Self {
            ns: TelemetryNs::Node,
            id: node_id,
            kind,
        }
    }

    /// Convenience: key for an edge-scoped metric.
    #[inline]
    pub const fn edge(edge_id: u32, kind: TelemetryKind) -> Self {
        Self {
            ns: TelemetryNs::Edge,
            id: edge_id,
            kind,
        }
    }

    /// Optional: node-port encoding (kept in Node ns).
    /// Layout: [ node_id:20 | is_output:1 | port_index:11 ]
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

/// Telemetry namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelemetryNs {
    /// Metrics for a node instance.
    Node,
    /// Metrics for an edge/queue.
    Edge,
    /// Runtime-level metrics.
    Runtime,
}

/// Metric kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelemetryKind {
    /// Count of processed items.
    Processed,
    /// Count of dropped items.
    Dropped,
    /// Count of deadline misses.
    DeadlineMiss,
    /// Queue depth or bytes gauge.
    QueueDepth,
    /// Latency histogram bucket (nanoseconds).
    Latency,
    /// Count of items pulled from inputs (node/edge-local).
    IngressMsgs,
    /// Count of items pushed to outputs (node/edge-local).
    EgressMsgs,
}

/// A no-op telemetry implementation, useful for P0.
#[derive(Debug, Default)]
pub struct NoopTelemetry;

impl Telemetry for NoopTelemetry {
    #[inline]
    fn incr_counter(&mut self, _key: TelemetryKey, _delta: u64) {}

    #[inline]
    fn set_gauge(&mut self, _key: TelemetryKey, _value: u64) {}

    #[inline]
    fn record_latency_ns(&mut self, _key: TelemetryKey, _value_ns: u64) {}
}

impl Telemetry for () {
    #[inline]
    fn incr_counter(&mut self, _key: TelemetryKey, _delta: u64) {}

    #[inline]
    fn set_gauge(&mut self, _key: TelemetryKey, _value: u64) {}

    #[inline]
    fn record_latency_ns(&mut self, _key: TelemetryKey, _value_ns: u64) {}
}
