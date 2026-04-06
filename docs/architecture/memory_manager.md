# Memory Model

Limen uses a **token-based, zero-copy** message storage system. Edges do not
store full messages — they store lightweight `MessageToken` handles. The actual
message data resides in a `MemoryManager`, which issues tokens on store and
returns guards on read. This design enables zero-copy routing, heterogeneous
memory class support, and heap-free operation in `no_std` builds.

---

## MemoryManager\<P\> Trait

```rust
pub trait MemoryManager<P: Payload>: HeaderStore {
    type ReadGuard<'a>: Deref<Target = Message<P>> where Self: 'a;
    type WriteGuard<'a>: DerefMut<Target = Message<P>> where Self: 'a;

    fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError>;
    fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError>;
    fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError>;
    fn free(&mut self, token: MessageToken) -> Result<(), MemoryError>;
    fn available(&self) -> usize;
    fn capacity(&self) -> usize;
    fn memory_class(&self) -> MemoryClass;
}
```

### Guard-Based API

The `ReadGuard` and `WriteGuard` associated types are the key abstraction.
In single-threaded builds they resolve to direct references with zero overhead.
In concurrent builds they resolve to per-slot `RwLock` guards. Application code
is identical in both cases — the guard type is an implementation detail.

### HeaderStore Supertrait

`MemoryManager` requires `HeaderStore`, which provides type-erased header
access. This allows edges to inspect message headers (for admission and byte
accounting) without knowing the payload type:

```rust
pub trait HeaderStore {
    type HeaderGuard<'a>: Deref<Target = MessageHeader> where Self: 'a;
    fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError>;
}
```

---

## Implementations

| Implementation | Feature | Allocation | Concurrency |
|---|---|---|---|
| `StaticMemoryManager<P, DEPTH>` | *(default)* | Fixed-capacity, stack-allocated | Single-threaded |
| `HeapMemoryManager<P>` | `alloc` | Heap-allocated slots, stack freelist | Single-threaded |
| `ConcurrentMemoryManager<P>` | `std` | Heap-allocated, per-slot `RwLock` | Thread-safe (lock-free freelist) |

All three implement the same `MemoryManager<P>` trait. Switching between them
requires only a type change in the graph definition — no application code
changes.

### StaticMemoryManager

The `no_std` default. Uses a fixed-size array of `Option<Message<P>>` slots
and a simple freelist. The `checked-memory-manager-refs` feature enables
per-slot borrow tracking for debugging (single-threaded safety checks).

### ConcurrentMemoryManager

Used by `ScopedGraphApi` for multi-threaded execution. Each slot is
independently locked via `RwLock<Message<P>>`, so reads on different tokens
do not contend. The freelist is lock-free (atomic CAS).

### Planned: Zero-Lock, Zero-Copy Concurrent Manager

A future `no_alloc`, lock-free concurrent memory manager is planned. It will
use raw pointers internally (safe external API, `unsafe` confined to the
implementation) to provide true zero-lock concurrent access without `Arc` or
`Mutex`. This will unify the single-threaded and multi-threaded code paths —
a graph using this manager will run on both a bare-metal `no_std` runtime and
a multi-threaded runtime without changing types.

The unsafe memory manager will also enable true zero-copy processing — nodes
can mutate payload data in place via `UnsafeCell<MaybeUninit<T>>` slot
storage, eliminating store/free overhead for in-place transforms.

See [ADR-013](../ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md) for the
full design rationale.

---

## MemoryClass

```rust
pub enum MemoryClass {
    Host,           // Regular host memory (default)
    PinnedHost,     // Page-locked, DMA-capable host memory
    Device(u8),     // Device-specific region (GPU/NPU ordinal 0..=15)
    Shared,         // Shared region accessible by multiple devices
}
```

Memory class is owned by the `MemoryManager`, not by the payload. This allows
the same payload type to reside in different memory regions depending on the
target platform.

---

## Placement Decisions

### PlacementAcceptance

A compact `u32` bitfield describing which `MemoryClass` values a port can
accept without requiring a copy:

```rust
pub struct PlacementAcceptance { bits: u32 }
```

Builder: `.with_host()`, `.with_pinned_host()`, `.with_device(ordinal)`,
`.with_shared()`. Query: `.accepts(MemoryClass)`. Supports union, intersection,
and superset operations.

### PlacementDecision

```rust
pub enum PlacementDecision {
    ZeroCopy,       // Producer and consumer agree on memory class
    AdaptRequired,  // A copy or transfer is needed
}
```

`decide_placement(acceptance, current)` is a `const fn` that returns `ZeroCopy`
if the acceptance bitfield includes the current memory class, `AdaptRequired`
otherwise.

---

## Why Token-Based?

1. **Zero-copy routing.** Edges carry lightweight tokens; the same message
   data can be referenced by multiple edges without duplication.
2. **Header access without payload type.** The `HeaderStore` supertrait
   lets edges inspect headers for admission decisions without knowing `P`.
3. **Heap-free in `no_std`.** `StaticMemoryManager` requires no allocator.
4. **Device-transparent.** Memory class metadata lives in the manager, so
   the same graph can target Host, Device, or Shared memory transparently.

---

## Related

- [Message and Payload](message_payload.md) — what gets stored
- [Edge Model](edge.md) — how tokens flow between nodes
- [Graph Flow (no_std)](graph_flow_no_std.md) — memory manager in the
  single-threaded execution path
- [Graph Flow (concurrent)](graph_flow_concurrent.md) — concurrent memory
  manager with per-slot locking
