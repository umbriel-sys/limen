# Telemetry Model

Telemetry in Limen is a core contract, not an optional add-on. The `Telemetry`
trait is designed for compile-time opt-out: when `METRICS_ENABLED` or
`EVENTS_STATICALLY_ENABLED` are `false`, all telemetry calls compile away
completely — zero overhead on constrained targets.

---

## Telemetry Trait

```rust
pub trait Telemetry {
    const METRICS_ENABLED: bool = true;
    const EVENTS_STATICALLY_ENABLED: bool = true;

    fn incr_counter(&mut self, key: TelemetryKey, delta: u64);
    fn set_gauge(&mut self, key: TelemetryKey, value: u64);
    fn record_latency_ns(&mut self, key: TelemetryKey, value_ns: u64);

    // Default no-ops:
    fn push_metrics(&mut self) {}
    fn events_enabled(&self) -> bool { false }
    fn push_event(&mut self, event: TelemetryEvent) {}
    fn flush(&mut self) {}
}
```

Setting `METRICS_ENABLED = false` causes runtimes to compile out all metric
collection. Setting `EVENTS_STATICALLY_ENABLED = false` skips
`TelemetryEvent` construction entirely.

---

## TelemetryKey

```rust
pub struct TelemetryKey {
    ns: TelemetryNs,       // Node | Edge | Runtime
    id: u32,               // Node/edge index
    kind: TelemetryKind,
}
```

### TelemetryKind

```rust
pub enum TelemetryKind {
    Processed, Dropped, DeadlineMiss,
    QueueDepth, Latency,
    IngressMsgs, EgressMsgs,
}
```

---

## Automatic Telemetry

When enabled, the framework automatically emits:

- Per-node latency histograms and deadline miss counters
- Queue depth gauges and watermark transitions
- Ingress/egress message counters
- Structured `NodeStep` events with timing, deadline, and error information
- Runtime lifecycle events (start, stop, reset)

On MCU targets, telemetry drains via UART or a fixed ring buffer. On
Linux/desktop, it drains via stdout or a file writer. Under `std`, a planned
TCP/network telemetry adapter will allow draining telemetry events to a remote
collector over the network.

---

## Structured Events

### NodeStepTelemetry

Per-step timing and outcome:

| Field | Type | Description |
|---|---|---|
| `graph_id` | `u32` | Graph identifier |
| `node_id` | `u32` | Node index |
| `node_name` | `&str` | Human-readable name |
| `timestamp_start_ns` | `u64` | Step start (nanoseconds) |
| `duration_ns` | `u64` | Step duration |
| `messages_processed` | `usize` | Batch count |
| `deadline_ns` | `Option<u64>` | Applicable deadline |
| `deadline_missed` | `bool` | Whether deadline was exceeded |
| `error_kind` | `Option<NodeStepError>` | Error category if step failed |

### TelemetryEvent

```rust
pub enum TelemetryEvent {
    NodeStep(NodeStepTelemetry),
    EdgeSnapshot(EdgeSnapshotTelemetry),
    Runtime(RuntimeTelemetryEvent),
}
```

---

## Aggregated Metrics

### GraphMetrics\<MAX_NODES, MAX_EDGES\>

Fixed-buffer aggregated metrics for the entire graph. Tracks per-node
`NodeMetrics` (processed, dropped, latency, deadline misses) and per-edge
`EdgeMetrics` (queue depth). Implements `Display` for human-readable output.

---

## GraphTelemetry

Full telemetry implementation backed by `GraphMetrics`:

- Fixed-buffer metric aggregation (no heap in `no_std`)
- I/O writer sink (`std`) for stdout or file output
- `push_metrics()` flushes aggregated state to the configured sink

---

## NoopTelemetry

```rust
pub struct NoopTelemetry;
```

`METRICS_ENABLED = false`, `EVENTS_STATICALLY_ENABLED = false`. All methods
are no-ops. `()` also implements `Telemetry` as a noop.

---

## Related

- [Runtime Model](runtime.md) — the runtime that drives telemetry emission
- [Node Model](node.md) — StepContext telemetry integration
- [Policy Guide](policy.md) — deadlines and budgets that trigger telemetry
