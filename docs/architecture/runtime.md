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

| Tier | Runtime | Scheduler | Allocation | Status |
|---|---|---|---|---|
| P0 | `TestNoStdRuntime` | Round-robin | None | Stable (integration tested) |
| P1 | `NoAllocRuntime` | Policy-enforcing | None | In progress |
| P2 single | `P2Runtime` | EDF / Throughput | `std` | Stubs present |
| P2 concurrent | `ScopedGraphApi::run_scoped` | Per-node workers | `std` | Stable (integration tested) |

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
