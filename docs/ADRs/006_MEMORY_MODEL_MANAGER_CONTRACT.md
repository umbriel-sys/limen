# ADR-006: Memory Model and Manager Contract

**Status:** Accepted
**Date:** 2025-11-01

## Context

In a typical graph runtime, edges own message data directly. This works for
simple cases but fails when:
- The same message must be routed to multiple outputs (fan-out).
- Messages must reside in heterogeneous memory (Host, Device, Shared).
- The system must be heap-free.

## Decision

Introduce a `MemoryManager<P>` trait that owns all message storage. Edges
store `MessageToken` handles, not full messages. The manager issues tokens
on `store()` and returns guard-protected references on `read()`.

```rust
pub trait MemoryManager<P: Payload>: HeaderStore {
    type ReadGuard<'a>: Deref<Target = Message<P>>;
    type WriteGuard<'a>: DerefMut<Target = Message<P>>;

    fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError>;
    fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError>;
    fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError>;
    fn free(&mut self, token: MessageToken) -> Result<(), MemoryError>;
    fn available(&self) -> usize;
    fn capacity(&self) -> usize;
    fn memory_class(&self) -> MemoryClass;
}
```

### Guard-Based Access

`ReadGuard` and `WriteGuard` are associated types. In `StaticMemoryManager`
they resolve to direct `&Message<P>` references (zero cost). In
`ConcurrentMemoryManager` they resolve to `RwLockReadGuard` /
`RwLockWriteGuard` (per-slot locking, no global contention).

### HeaderStore Supertrait

```rust
pub trait HeaderStore {
    type HeaderGuard<'a>: Deref<Target = MessageHeader>;
    fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError>;
}
```

Edges use `HeaderStore` (not `MemoryManager<P>`) for admission and byte
accounting. This means edges operate without knowing the payload type —
a critical enabler for heterogeneous graphs.

### Memory Classes

`MemoryClass` (Host, PinnedHost, Device, Shared) is owned by the manager,
not the payload. `PlacementAcceptance` bitfields on node ports declare which
classes they accept zero-copy. `decide_placement()` returns `ZeroCopy` or
`AdaptRequired`.

## Rationale

- **Zero-copy routing.** Multiple edges can reference the same message via
  tokens without duplicating data.
- **Heap-free.** `StaticMemoryManager<P, DEPTH>` is stack-allocated with a
  fixed slot array.
- **Device-transparent.** The same graph definition works across Host and
  Device memory by changing only the manager implementation.
- **Header peek without payload.** Edges make admission decisions from
  `HeaderStore` without loading the full message.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Edges own messages directly | Simpler API | No zero-copy, no heterogeneous memory, heap required for concurrent | Fundamental limitation |
| Global arena allocator | Zero-copy | Global lock for concurrent access, hard to partition across memory classes | Contention and inflexibility |
| Reference-counted messages (`Arc<Message>`) | Zero-copy via sharing | Requires `alloc`, atomic overhead on MCU, no memory class support | Not `no_std` compatible |

## Consequences

### Positive
- Token-based routing enables zero-copy without heap in `no_std`.
- Per-slot locking in `ConcurrentMemoryManager` means reads on different
  tokens do not contend.
- Memory class negotiation is built into the trait surface.

### Trade-offs
- One extra level of indirection (token → read → message) compared to
  direct edge ownership.
- Token lifecycle management is the `StepContext`'s responsibility — tokens
  must be freed after processing to avoid leaks.

## Related ADRs

- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — who stores vs who routes
- [ADR-007](007_EDGE_CONTRACT.md) — how edges use tokens
- [ADR-004](004_PAYLOAD_FIRST_CONTRACTS.md) — what gets stored
