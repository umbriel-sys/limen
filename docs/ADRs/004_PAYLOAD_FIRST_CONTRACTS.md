# ADR-004: Payload-First Contracts

**Status:** Accepted
**Date:** 2025-10-01

## Context

A graph framework must not dictate what data flows through it. Users may need
to pass raw sensor readings, fixed-point values, tensor buffers, protobuf
structs, or opaque byte slices. The framework must support all of these while
still providing admission accounting and telemetry.

## Decision

The `Payload` trait is the single requirement for any type flowing through a
Limen graph:

```rust
pub trait Payload {
    fn buffer_descriptor(&self) -> BufferDescriptor;
}
```

`BufferDescriptor` reports the byte size of the payload. Memory class
information is owned by the `MemoryManager`, not the payload.

All core traits — `Node<IN, OUT, InP, OutP>`, `Edge`, `MemoryManager<P>` —
are generic over `InP: Payload` and `OutP: Payload`. Any type implementing
`Payload` can flow through the graph without modification to the framework.

### First-Class Tensor Support

While any `Payload` is valid, Limen provides optimised tensor types as the
primary currency for ML workloads:

- `TensorRef<'a, T, N, R>` — immutable typed tensor view
- `MutTensorRef<'a, T, N, R>` — mutable tensor view

These carry shape metadata and implement `Payload` with accurate byte
reporting.

## Rationale

- **No data format lock-in.** Users implement `Payload` for their types;
  the framework handles the rest.
- **Admission and telemetry.** `buffer_descriptor()` provides the byte size
  needed for byte-cap admission and telemetry accounting without requiring
  the framework to understand the payload structure.
- **Memory class separation.** By placing memory class in the manager (not
  the payload), the same payload type can reside in Host, Device, or Shared
  memory depending on the target platform.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Fixed payload types (e.g., `&[u8]`) | Simple, no trait needed | Forces serialisation, loses type safety | Defeats type-safe graph wiring |
| `serde`-based serialisation | Familiar ecosystem | Heap allocation, `no_std` limitations, runtime overhead | Unacceptable on MCU |
| Associated type on `Node` (not generic) | Simpler trait | Can't have different payload types on input vs output | Too restrictive |

## Consequences

### Positive
- Users bring their own types. No forced serialisation or allocation.
- Type safety is preserved end-to-end: the codegen validates that edge payload
  types match connected node ports.
- Tensor types provide a zero-copy, shape-aware path for ML workloads.

### Trade-offs
- Every custom payload type needs a `Payload` impl (usually one line).
- Byte size reporting is the user's responsibility — incorrect reporting
  affects admission accuracy but not safety.

## Related ADRs

- [ADR-006](006_MEMORY_MODEL_MANAGER_CONTRACT.md) — how payloads are stored
- [ADR-008](008_NODE_CONTRACT.md) — how nodes consume and produce payloads
- [ADR-011](011_BATCH_SEMANTICS.md) — batching of payload messages
