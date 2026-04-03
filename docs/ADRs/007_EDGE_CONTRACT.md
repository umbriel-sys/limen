# ADR-007: Edge Contract

**Status:** Accepted
**Date:** 2025-11-01

## Context

Edges connect node output ports to node input ports. They must support:
- Policy-enforced admission (drop, evict, reject, block).
- Byte-aware capacity management.
- Batch operations.
- Both single-threaded and concurrent execution.
- No heap allocation in the default build.

## Decision

Edges implement a SPSC (single-producer, single-consumer) queue trait. They
store `MessageToken` handles, not full messages. Admission decisions use
`HeaderStore` for byte accounting — statically dispatched, never dynamic:

```rust
pub trait Edge {
    fn try_push<H: HeaderStore>(
        &mut self, token: MessageToken, policy: &EdgePolicy, headers: &H,
    ) -> EnqueueResult;

    fn try_pop<H: HeaderStore>(
        &mut self, headers: &H,
    ) -> Result<MessageToken, QueueError>;

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy;
    fn is_empty(&self) -> bool;
    fn try_peek(&self) -> Result<MessageToken, QueueError>;
}
```

### HeaderStore Parameter

The `H: HeaderStore` parameter is the key design decision. Edges receive
header access as a generic parameter, not via `dyn HeaderStore`. This means:
- Zero vtable overhead.
- Edges can inspect message headers (byte size, QoS, deadline) for admission.
- Edges do not know the payload type `P`.

### ScopedEdge Extension

For concurrent execution, `ScopedEdge` provides handle-based access:

```rust
pub trait ScopedEdge: Edge {
    type Handle<'a>: Edge + Send + 'a;
    fn scoped_handle<'a>(&'a self, kind: EdgeHandleKind) -> Self::Handle<'a>;
}
```

### Conformance Testing

A `run_edge_contract_tests!` macro validates any custom edge implementation
against the full contract (FIFO ordering, capacity, token coherence, occupancy
accuracy).

## Rationale

- **Token storage.** Edges carry tokens, not payloads. This enables zero-copy
  routing and keeps edge memory footprint small (one `MessageToken` per slot).
- **Static HeaderStore dispatch.** Admission decisions happen in the hot path.
  Using a generic parameter avoids vtable lookup.
- **Policy as parameter.** `EdgePolicy` is passed to `try_push` and
  `occupancy`, not stored in the edge. This allows the same edge type to be
  used with different policies (configured at the graph level).

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Edges store `Message<P>` directly | No indirection | Payload type in edge generic params, no zero-copy, larger memory footprint | Against memory model |
| `dyn HeaderStore` parameter | Simpler trait signature | Vtable dispatch in hot path | Violates ADR-002 |
| Edge owns its policy | Simpler `try_push` signature | Can't reuse same edge type with different policies | Inflexible |

## Consequences

### Positive
- Five edge implementations from a single trait, all interchangeable.
- External implementors validate via conformance test suite.
- Concurrent execution adds `ScopedEdge` without changing the base `Edge` API.

### Trade-offs
- Edges require a `HeaderStore` reference on every push/pop — this is always
  the memory manager, passed via `StepContext`.
- `NoQueue` placeholder needed for unconnected ports.

## Related ADRs

- [ADR-006](006_MEMORY_MODEL_MANAGER_CONTRACT.md) — the `HeaderStore` supertrait
- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — edge responsibility boundary
