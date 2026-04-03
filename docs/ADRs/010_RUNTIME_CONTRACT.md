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

### Tiered Implementations

| Tier | Runtime | Scheduling | Allocation |
|---|---|---|---|
| P0 | `TestNoStdRuntime` | Round-robin | None |
| P1 | `NoAllocRuntime` | Policy-enforcing (EDF, Throughput) | None |
| P2 | `ScopedGraphApi::run_scoped` | Per-node `WorkerScheduler` | `std` |

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
- P1/P2 runtimes are still in development — only P0 and the scoped-thread
  test runtime are fully stable.

## Related ADRs

- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — runtime responsibility boundary
- [ADR-009](009_GRAPH_CONTRACT_CODEGEN.md) — the `GraphApi` that runtimes drive
