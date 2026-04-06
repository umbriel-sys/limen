# Node Model

Nodes are the units of computation in a Limen graph. Every node implements a
uniform trait — `Node<IN, OUT, InP, OutP>` — and receives a fully wired
`StepContext` at each step. Nodes are policy-agnostic: the framework handles
batching, admission, deadline enforcement, and telemetry. Node implementations
focus solely on payload transformation.

---

## NodeKind

Categorical label used by runtimes and schedulers for heuristic decisions:

```rust
pub enum NodeKind {
    Source,    // 0 inputs, >=1 outputs
    Process,   // >=1 inputs, >=1 outputs (general transform)
    Model,     // >=1 inputs, >=1 outputs (bound to ComputeBackend)
    Split,     // >=1 inputs, >=2 outputs (fan-out)
    Join,      // >=2 inputs, >=1 outputs (fan-in)
    Sink,      // >=1 inputs, 0 outputs
    External,  // Request/response via transport
}
```

> **Note:** `NodeKind` and `NodeCapabilities` are not yet finalised. Some
> variants (e.g., `Split`, `Join`, `External`) are legacy placeholders and
> currently unused. These will be finalised in the run-up to v0.1.0 as part
> of C2 (N-to-M arity) and C3 (trait stabilisation).

---

## Node Trait

```rust
pub trait Node<const IN: usize, const OUT: usize, InP: Payload, OutP: Payload> {
    // Capabilities and policies
    fn describe_capabilities(&self) -> NodeCapabilities;
    fn input_acceptance(&self) -> [PlacementAcceptance; IN];
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT];
    fn policy(&self) -> NodePolicy;
    fn node_kind(&self) -> NodeKind;

    // Lifecycle
    fn initialize<C, Tel>(&mut self, clock: &C, telemetry: &mut Tel) -> Result<(), NodeError>;
    fn start<C, Tel>(&mut self, clock: &C, telemetry: &mut Tel) -> Result<(), NodeError>;

    // Per-message processing (required)
    fn process_message<C: PlatformClock>(
        &mut self, msg: &Message<InP>, clock: &C,
    ) -> Result<ProcessResult<OutP>, NodeError>;

    // Per-step execution (default: pops one message, calls process_message)
    fn step(&mut self, ctx: &mut StepContext<...>) -> Result<StepResult, NodeError>;

    // Batch step (default: pops a batch, calls process_message per item)
    fn step_batch(&mut self, ctx: &mut StepContext<...>) -> Result<StepResult, NodeError>;
}
```

Most node implementations only need to implement `process_message`. The default
`step` and `step_batch` implementations handle popping from input edges,
pushing to output edges, and managing memory tokens automatically.

---

## StepResult

Returned by `step()` to inform the scheduler:

| Variant | Meaning |
|---|---|
| `MadeProgress` | Consumed and/or produced messages |
| `NoInput` | All input edges are empty |
| `Backpressured` | Output edges are full |
| `WaitingOnExternal` | Device or transport operation pending |
| `YieldUntil(Ticks)` | Cooperative scheduling hint — don't step again until this tick |
| `Terminal` | Node will produce no further outputs |

---

## ProcessResult\<P\>

Returned by `process_message` to indicate the per-message outcome:

```rust
pub enum ProcessResult<P: Payload> {
    Output(Message<P>),  // Produced output for port 0
    Consumed,            // Consumed input, no output this time
    Skip,                // Nothing to process
}
```

---

## StepContext

The execution environment passed to `Node::step`. Generic over all queue,
manager, clock, and telemetry types — zero dynamic dispatch:

```rust
pub struct StepContext<
    'graph, 'telemetry, 'clock,
    const IN: usize, const OUT: usize,
    InP, OutP, InQ, OutQ, InM, OutM, C, T,
> { ... }
```

### Key Methods

| Method | Description |
|---|---|
| `pop_and_process(port, f)` | Pop one message, call closure, push output, free token |
| `pop_batch_and_process(port, n, policy, f)` | Pop a batch, process per item, free stride tokens |
| `push_output(port, msg)` | Store message, push token to output edge |
| `in_occupancy(i)` | Snapshot input port occupancy (updates telemetry gauge) |
| `out_occupancy(o)` | Snapshot output port occupancy (updates telemetry gauge) |
| `in_peek_header(i)` | Peek front message header without consuming |
| `in_policy(i)` / `out_policy(o)` | Edge policy for port |
| `clock()` | Borrow the platform clock |
| `now_ticks()` / `now_nanos()` | Current time |
| `ticks_to_nanos(t)` / `nanos_to_ticks(ns)` | Time conversion |
| `telemetry_mut()` | Mutable access to telemetry sink |
| `input_edge_has_batch(port, policy)` | Readiness predicate for batch-mode nodes |

`StepContext` also provides `OutStepContext` — a subset exposing only output
ports, managers, clock, and telemetry — used internally by batch processing
helpers.

---

## NodeCapabilities

```rust
pub struct NodeCapabilities {
    device_streams: bool,   // Can execute on device streams (P2)
    degrade_tiers: bool,    // Supports mixed-precision degradation
}
```

---

## Related

- [Source Nodes](source.md) — 0-input adapter
- [Sink Nodes](sink.md) — 0-output adapter
- [Inference Nodes](model.md) — ComputeBackend-backed nodes
- [Policy Guide](policy.md) — NodePolicy and batching
- [Graph Model](graph.md) — how StepContext is constructed
