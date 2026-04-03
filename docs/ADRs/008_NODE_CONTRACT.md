# ADR-008: Node Contract

**Status:** Accepted
**Date:** 2025-11-01

## Context

Nodes are the units of computation. They must:
- Work with any payload type, any edge implementation, any memory manager.
- Not embed scheduling, admission, or telemetry logic.
- Support three roles: source (0 inputs), sink (0 outputs), and general
  processing (N inputs, M outputs).
- Be testable in isolation.

## Decision

A single uniform trait covers all node roles:

```rust
pub trait Node<const IN: usize, const OUT: usize, InP: Payload, OutP: Payload> {
    fn process_message<C: PlatformClock>(
        &mut self, msg: &Message<InP>, clock: &C,
    ) -> Result<ProcessResult<OutP>, NodeError>;

    fn step(&mut self, ctx: &mut StepContext<...>) -> Result<StepResult, NodeError>;
    fn step_batch(&mut self, ctx: &mut StepContext<...>) -> Result<StepResult, NodeError>;
    // ... lifecycle, capabilities, policy methods
}
```

### Policy-Agnostic Nodes

Nodes **do not** implement admission, batching, or telemetry. These are
handled by the framework:
- `StepContext` manages edge interactions (pop, push, occupancy).
- `NodePolicy` configures batching and budget from outside the node.
- Telemetry emission happens in the runtime, not in node code.

### Role Adapters

Thin role-specific traits (`Source`, `Sink`) wrap into the full `Node` trait
via adapters (`SourceNode`, `SinkNode`). This lets sensor and output
implementations focus on their specific concerns while conforming to the
uniform contract.

### StepContext

The node's window into the graph. Generic over all types — zero dynamic
dispatch. Provides curated access to input/output edges, memory managers,
clock, and telemetry.

## Rationale

- **Single trait.** Runtimes and schedulers need one interface, not three.
  `NodeKind` distinguishes roles for heuristic purposes.
- **`process_message` is the primary extension point.** Most nodes only
  implement this one method. Default `step` and `step_batch` handle queue
  interaction automatically.
- **Policy-agnostic.** Separating policy from node logic means the same node
  implementation works with different batching, budget, and deadline
  configurations.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Separate traits per role | Cleaner role separation | Runtime needs match/dispatch; can't step a generic "node" | Adds complexity, breaks uniform stepping |
| Node owns its edges | Natural actor model | Prevents external scheduling, complicates graph wiring | Scheduler cannot optimise |
| Async trait (`async fn step`) | Natural for I/O-bound nodes | Requires async runtime, complex on `no_std`, adds overhead | Sync-first core; async adapters above |

## Consequences

### Positive
- One `step_node_by_index(i)` method steps any node — no pattern matching.
- Role adapters (`SourceNode`, `SinkNode`) provide ergonomic constructors
  via `From<S>`.
- Nodes are trivially testable: construct a `StepContext` with mock edges
  and managers.

### Trade-offs
- `StepContext` has many type parameters (12+). This is intentional — codegen
  generates them.
- Source nodes need a synthetic "ingress edge" to report pre-source
  occupancy, which is slightly unintuitive.

## Related ADRs

- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — node responsibility boundary
- [ADR-009](009_GRAPH_CONTRACT_CODEGEN.md) — how nodes are wired into graphs
- [ADR-011](011_BATCH_SEMANTICS.md) — how batching policy interacts with nodes
