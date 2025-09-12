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
