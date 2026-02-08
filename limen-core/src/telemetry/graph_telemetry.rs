//! Basic graph level telemetry implementation,

use crate::prelude::sink::TelemetrySink;

use super::*;

/// Fixed size telemetry collector backed by arrays.
///
/// This collector stores node and edge metrics in statically sized arrays and
/// forwards structured events to a `TelemetrySink`. It is suitable for no_std
/// environments where heap allocation is not available.
pub struct GraphTelemetry<const MAX_NODES: usize, const MAX_EDGES: usize, Sink: TelemetrySink> {
    /// Per-graph metrics (nodes and edges) indexed by identifier.
    metrics: GraphMetrics<MAX_NODES, MAX_EDGES>,
    /// Event writer used for structured telemetry.
    writer: Sink,
    /// Flag that controls whether structured events are forwarded.
    events: bool,
}

impl<const MAX_NODES: usize, const MAX_EDGES: usize, Writer: TelemetrySink>
    GraphTelemetry<MAX_NODES, MAX_EDGES, Writer>
{
    /// Create a new fixed size collector with the given event writer.
    ///
    /// All node and edge metrics are initialized to zero.
    pub const fn new(id: u32, events: bool, writer: Writer) -> Self {
        Self {
            metrics: GraphMetrics::new(id),
            writer,
            events,
        }
        // const NM: NodeMetrics = NodeMetrics::new();
        // const EM: EdgeMetrics = EdgeMetrics::new();
    }

    /// Enable emission of structured telemetry events.
    #[inline]
    pub fn enable_events(&mut self) {
        self.events = true;
    }

    /// Disable emission of structured telemetry events.
    #[inline]
    pub fn disable_events(&mut self) {
        self.events = false;
    }

    /// Return true if the given node identifier is within bounds.
    #[inline]
    fn node_ok(id: u32) -> bool {
        (id as usize) < MAX_NODES
    }

    /// Return true if the given edge identifier is within bounds.
    #[inline]
    fn edge_ok(id: u32) -> bool {
        (id as usize) < MAX_EDGES
    }

    /// Access the underlying metrics record.
    #[inline]
    pub fn metrics(&self) -> &GraphMetrics<MAX_NODES, MAX_EDGES> {
        &self.metrics
    }

    /// Access the full array of node metrics.
    #[inline]
    pub fn nodes(&self) -> &[NodeMetrics; MAX_NODES] {
        &self.metrics.nodes
    }

    /// Access the full array of edge metrics.
    #[inline]
    pub fn edges(&self) -> &[EdgeMetrics; MAX_EDGES] {
        &self.metrics.edges
    }

    /// Access the underlying event writer.
    #[inline]
    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// Merge metrics from another fixed size collector into this one.
    ///
    /// Node metrics are combined by saturating addition and maximum for latency
    /// maxima, while edge queue depth uses a last writer wins strategy.
    pub fn merge_from<const N2: usize, const E2: usize, W2: TelemetrySink>(
        &mut self,
        other: &GraphTelemetry<N2, E2, W2>,
    ) {
        let n = core::cmp::min(MAX_NODES, N2);
        let e = core::cmp::min(MAX_EDGES, E2);

        for i in 0..n {
            let dst = &mut self.metrics.nodes[i];
            let src = &other.metrics.nodes[i];
            dst.processed = dst.processed.saturating_add(src.processed);
            dst.dropped = dst.dropped.saturating_add(src.dropped);
            dst.ingress = dst.ingress.saturating_add(src.ingress);
            dst.egress = dst.egress.saturating_add(src.egress);
            dst.lat_sum = dst.lat_sum.saturating_add(src.lat_sum);
            dst.lat_cnt = dst.lat_cnt.saturating_add(src.lat_cnt);
            if src.lat_max > dst.lat_max {
                dst.lat_max = src.lat_max;
            }
        }

        for i in 0..e {
            // last-wins
            self.metrics.edges[i].queue_depth = other.metrics.edges[i].queue_depth;
        }
    }
}

impl<const N: usize, const E: usize, W: TelemetrySink> Telemetry for GraphTelemetry<N, E, W> {
    const METRICS_ENABLED: bool = true;
    const EVENTS_STATICALLY_ENABLED: bool = true;

    #[inline]
    fn incr_counter(&mut self, key: TelemetryKey, delta: u64) {
        match (key.ns(), key.kind()) {
            (TelemetryNs::Node, TelemetryKind::Processed) if Self::node_ok(key.id()) => {
                self.metrics.nodes[key.id() as usize].processed = self.metrics.nodes
                    [key.id() as usize]
                    .processed
                    .saturating_add(delta);
            }
            (TelemetryNs::Node, TelemetryKind::Dropped) if Self::node_ok(key.id()) => {
                self.metrics.nodes[key.id() as usize].dropped = self.metrics.nodes
                    [key.id() as usize]
                    .dropped
                    .saturating_add(delta);
            }
            (TelemetryNs::Node, TelemetryKind::IngressMsgs) if Self::node_ok(key.id()) => {
                self.metrics.nodes[key.id() as usize].ingress = self.metrics.nodes
                    [key.id() as usize]
                    .ingress
                    .saturating_add(delta);
            }
            (TelemetryNs::Node, TelemetryKind::EgressMsgs) if Self::node_ok(key.id()) => {
                self.metrics.nodes[key.id() as usize].egress = self.metrics.nodes
                    [key.id() as usize]
                    .egress
                    .saturating_add(delta);
            }
            (TelemetryNs::Node, TelemetryKind::DeadlineMiss) if Self::node_ok(key.id()) => {
                self.metrics.nodes[key.id() as usize].deadline_miss_count = self.metrics.nodes
                    [key.id() as usize]
                    .deadline_miss_count
                    .saturating_add(delta);
            }
            _ => {}
        }
    }

    #[inline]
    fn set_gauge(&mut self, key: TelemetryKey, value: u64) {
        if matches!(key.ns(), TelemetryNs::Edge)
            && matches!(key.kind(), TelemetryKind::QueueDepth)
            && Self::edge_ok(key.id())
        {
            self.metrics.edges[key.id() as usize].queue_depth = value as u32;
        }
    }

    #[inline]
    fn record_latency_ns(&mut self, key: TelemetryKey, value_ns: u64) {
        if matches!(key.ns(), TelemetryNs::Node)
            && matches!(key.kind(), TelemetryKind::Latency)
            && Self::node_ok(key.id())
        {
            let m = &mut self.metrics.nodes[key.id() as usize];
            m.lat_sum = m.lat_sum.saturating_add(value_ns);
            m.lat_cnt = m.lat_cnt.saturating_add(1);
            if value_ns > m.lat_max {
                m.lat_max = value_ns;
            }
        }
    }

    #[inline]
    fn push_metrics(&mut self) {
        let _ = self.writer.push_metrics(&self.metrics);
    }

    /// Return true if this telemetry collector wants structured events.
    ///
    /// Runtimes and nodes can use this to avoid constructing `TelemetryEvent`
    /// values when events are disabled, keeping the hot path as cheap as possible.
    #[inline]
    fn events_enabled(&self) -> bool {
        self.events
    }

    #[inline]
    fn push_event(&mut self, event: TelemetryEvent) {
        if self.events {
            let _ = self.writer.push_event(&event);
        }
    }

    #[inline]
    fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}

impl<const N: usize, const E: usize, W: TelemetrySink + Clone> Clone for GraphTelemetry<N, E, W> {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics,
            writer: self.writer.clone(),
            events: self.events,
        }
    }
}

/// Convenience helper to merge one fixed collector into another.
///
/// This simply forwards to `FixedTelemetry::merge_from` and is useful when
/// working with references to the collectors.
pub fn merge_fixed_telemetry<
    const N1: usize,
    const E1: usize,
    W1: TelemetrySink,
    const N2: usize,
    const E2: usize,
    W2: TelemetrySink,
>(
    dst: &mut GraphTelemetry<N1, E1, W1>,
    src: &GraphTelemetry<N2, E2, W2>,
) {
    dst.merge_from(src);
}
