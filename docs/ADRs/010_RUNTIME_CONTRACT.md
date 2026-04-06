# ADR-010: Runtime Contract

**Status:** Accepted
**Date:** 2025-11-01

## Context

The same graph must execute on targets with vastly different capabilities:
bare-metal MCU (no heap, no threads), embedded Linux (heap, single-threaded),
and desktop/server (heap, multi-threaded). The execution layer must be
swappable without changing the graph or node implementations.

## Decision

A single `LimenRuntime` trait defines the executor contract:

```rust
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where Graph: GraphApi<NODE_COUNT, EDGE_COUNT>
{
    type Clock: PlatformClock;
    type Telemetry: Telemetry;
    type Error;

    fn init(&mut self, graph: &mut Graph, clock: Self::Clock, telemetry: Self::Telemetry) -> Result<(), Self::Error>;
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error>;
    fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error>;
    fn request_stop(&mut self);
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error>;
    fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT];
}
```

### Planned Production Runtimes

| Tier | Runtime | Scheduling | Features |
|---|---|---|---|
| P0 | `NoStdRuntime` | Round-robin / policy-enforcing | `no_std`, no alloc, single-threaded |
| P1 | `AllocRuntime` | Policy-enforcing (EDF, Throughput) | `no_std`, alloc enabled, single-threaded |
| P2 | `ThreadedRuntime` | Per-node `WorkerScheduler` | `std`, alloc enabled, multi-threaded |

All production runtimes are currently planned — none are implemented yet.

### Existing Test/Bench Runtimes

`TestNoStdRuntime` and `TestScopedRuntime` are bench-only implementations
(gated behind the `bench` feature) used by integration tests. They validate
the `LimenRuntime` contract but are not production runtimes.

### Scheduling Abstraction

- **P0/P1:** `DequeuePolicy` selects next node from `NodeSummary` snapshots.
- **P2:** `WorkerScheduler` makes per-worker decisions (`Step`, `WaitMicros`,
  `Stop`).

Schedulers receive snapshots, not live graph state. They cannot mutate the
graph directly.

### Runtime Owns Clock and Telemetry

After `init()`, the runtime takes ownership of the clock and telemetry
instances. This ensures consistent time and metric state across all steps.

## Rationale

- **Swappable execution.** The same `GraphApi` works with any runtime.
  Switching from MCU to server requires changing only the runtime type.
- **Allocation-agnostic.** `LimenRuntime` makes no assumption about heap
  availability. The P0 runtime proves heap-free execution is possible.
- **Scheduler isolation.** Schedulers cannot modify graph state; they only
  observe occupancy snapshots. This prevents scheduling bugs from corrupting
  graph invariants.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Graph drives its own execution | No separate runtime type | Graph becomes monolithic; can't swap scheduling strategy | Coupling |
| Async executor integration | Natural for I/O-bound work | Requires async runtime on MCU, non-trivial `no_std` async | Sync-first core |
| Fixed round-robin only | Simplest | Cannot implement deadline-aware scheduling | Insufficient for robotics |

## Consequences

### Positive
- Same integration tests run against both P0 (no_std) and P2 (concurrent)
  runtimes.
- New scheduling policies (EDF, throughput, custom) are pluggable without
  touching graph or node code.
- `RuntimeStopHandle` enables cooperative cross-thread shutdown.

### Trade-offs
- `step_node_by_index` uses runtime index dispatch (the one place where
  dispatch is not compile-time).
- All production runtimes (P0, P1, P2) are planned. Currently, only the
  bench-only test runtimes (`TestNoStdRuntime`, `TestScopedRuntime`) are
  implemented and integration-tested.

## Related ADRs

- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — runtime responsibility boundary
- [ADR-009](009_GRAPH_CONTRACT_CODEGEN.md) — the `GraphApi` that runtimes drive
- [ADR-013](013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md) — zero-lock, zero-copy concurrent graphs (required for full P2 design)
