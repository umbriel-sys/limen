# Runtime Model

The runtime is the top-level executor that drives the graph step loop.
`LimenRuntime` is allocation- and threading-agnostic — the same graph runs
under a bare-metal round-robin runtime or a multi-threaded scheduler without
changes to application code.

---

## LimenRuntime Trait

```rust
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
{
    type Clock: PlatformClock + Sized;
    type Telemetry: Telemetry + Sized;
    type Error;

    fn init(
        &mut self, graph: &mut Graph,
        clock: Self::Clock, telemetry: Self::Telemetry,
    ) -> Result<(), Self::Error>;

    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error>;
    fn request_stop(&mut self);
    fn is_stopping(&self) -> bool;
    fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT];
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error>;
    fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error>;
}
```

- The runtime **owns** `Clock` and `Telemetry` after `init()`.
- `step()` returns `Ok(true)` if more work remains, `Ok(false)` to stop.
- `run()` default implementation: loops `step()` until `is_stopping()` or
  `step()` returns `false`.

---

## Execution Loop

A single iteration of the runtime step loop:

1. **Snapshot occupancies** — `write_all_edge_occupancies()` populates the
   occupancy buffer.
2. **Compute node summaries** — for each node, derive a `NodeSummary` from
   its incident edge occupancies and policy.
3. **Select next node** — the `DequeuePolicy` (scheduler) picks which node
   to step from the summary array.
4. **Step the node** — `step_node_by_index(i)` builds a `StepContext` and
   calls `node.step(ctx)`.
5. **Partial refresh** — `refresh_occupancies_for_node<I>()` updates only
   edges incident to the stepped node.
6. **Emit telemetry** — record step result, latency, and occupancy.
7. **Loop** — repeat from step 3 (or step 1 if a full refresh is due).

---

## Runtime Tiers

### Planned Production Runtimes

| Tier | Runtime | Features | Scheduling | Status |
|---|---|---|---|---|
| P0 | `NoStdRuntime` | `no_std`, no alloc, single-threaded | Round-robin / policy-enforcing | Planned |
| P1 | `AllocRuntime` | `no_std`, alloc enabled, single-threaded | Policy-enforcing (EDF, Throughput) | Planned |
| P2 | `ThreadedRuntime` | `std`, alloc enabled, multi-threaded | Per-node workers via `WorkerScheduler` | Planned (requires [ADR-013](../ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md)) |

### Existing Test/Bench Runtimes

| Runtime | Features | Purpose | Status |
|---|---|---|---|
| `TestNoStdRuntime` | `no_std`, no alloc | Bench-only integration testing (single-threaded, round-robin) | Stable |
| `TestScopedRuntime` | `std` | Bench-only integration testing (concurrent, `ScopedGraphApi::run_scoped`) | Stable |

P0, P1, and P2 are the planned production runtimes — all are currently
unimplemented. `TestNoStdRuntime` and `TestScopedRuntime` are bench-only
implementations (gated behind the `bench` feature) used by integration tests.
They validate the `LimenRuntime` contract but are not intended for production
use.

A single P2 multi-threaded runtime is planned rather than separate
single-threaded and multi-threaded `std` runtimes. If `std` is available but
only one thread is needed, the P1 runtime with heap-backed types (`HeapRing`,
`HeapMemoryManager`) covers that scenario without thread-safe overhead.

---

## Scheduling

### Readiness

`NodeReadiness` captures whether a node can make progress:

```rust
pub enum NodeReadiness {
    Ready,                // Has input and output capacity
    ReadyUnderPressure,   // Has input but output above soft watermark
    Blocked,              // No input or output at hard cap
    Terminal,             // Previously returned StepResult::Terminal
}
```

### DequeuePolicy (P0/P1)

Selects the next node from `NodeSummary` candidates:

```rust
pub trait DequeuePolicy<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    fn select_next(
        &mut self,
        summaries: &[NodeSummary; NODE_COUNT],
        occupancies: &[EdgeOccupancy; EDGE_COUNT],
    ) -> Option<usize>;
}
```

Built-in policies:
- **`EdfPolicy`** — earliest deadline first
- **`ThroughputPolicy`** — prefer `Ready` over `ReadyUnderPressure`

### WorkerScheduler (P2 concurrent)

Per-worker decision trait for `ScopedGraphApi::run_scoped`:

```rust
pub trait WorkerScheduler {
    fn decide(&mut self, state: &WorkerState) -> WorkerDecision;
}

pub enum WorkerDecision {
    Step,                  // Execute one step
    WaitMicros(u64),       // Sleep then re-evaluate
    Stop,                  // Shut down this worker
}
```

---

## RuntimeStopHandle (std)

Cross-thread cooperative stop mechanism:

```rust
pub struct RuntimeStopHandle {
    flag: Arc<AtomicBool>,
}
```

`Clone`able. Call `request_stop()` from any thread; workers observe via
`is_stopping()`.

---

## Related

- [Graph Model](graph.md) — the `GraphApi` that runtimes drive
- [Policy Guide](policy.md) — policies that govern scheduling decisions
- [Platform](platform.md) — the clock consumed by the runtime
- [Graph Flow (no_std)](graph_flow_no_std.md) — P0/P1 execution path
- [Graph Flow (concurrent)](graph_flow_concurrent.md) — P2 execution path
