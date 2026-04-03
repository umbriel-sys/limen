# Message and Payload Model

Every value flowing through a Limen graph is wrapped in `Message<P>`. The
message header carries metadata used by the edge admission system, scheduler,
and telemetry layer — node implementations work with the payload while the
framework handles the rest.

---

## MessageHeader

Fixed header present on all messages:

```rust
pub struct MessageHeader {
    trace_id:           TraceId,           // Correlation identifier across nodes
    sequence:           SequenceNumber,    // Producer-assigned monotonic ordering
    creation_tick:      Ticks,             // Monotonic creation timestamp
    deadline_ns:        Option<DeadlineNs>,// Optional absolute deadline (nanoseconds)
    qos:                QoSClass,          // BestEffort / Critical / Safety
    payload_size_bytes: usize,             // For byte-cap admission
    flags:              MessageFlags,      // Batch boundaries, degrade hints
    memory_class:       MemoryClass,       // Host / Device placement
}
```

All fields have getters and setters. `MessageHeader::empty()` produces a
zero-initialised header suitable for test or placeholder use.

---

## MessageFlags

Compact `u32` bitfield carried in the header:

| Flag | Bit | Meaning |
|---|---|---|
| `FIRST_IN_BATCH` | 0 | First element of a batch |
| `LAST_IN_BATCH` | 1 | Last element of a batch |
| `DEGRADE_ALLOWED` | 2 | Downstream may use a degraded (fast/low-precision) path |

Builder API: `.with(bit)`, `.without(bit)`, `.contains(bit)`. Typed helpers:
`.with_first_in_batch()`, `.with_last_in_batch()`, `.with_degrade_allowed()`.

---

## Message\<P\>

```rust
pub struct Message<P: Payload> {
    header: MessageHeader,
    payload: P,
}
```

| Method | Description |
|---|---|
| `Message::new(header, payload)` | Constructor |
| `.header()` / `.header_mut()` | Header access |
| `.payload()` / `.payload_mut()` | Payload access |
| `.with_payload(p)` | Replace payload, returns `Message<Q>` |
| `.map_payload(f)` | Transform payload via closure |
| `.into_parts()` | Destructure into `(MessageHeader, P)` |

`Default` is implemented when `P: Default`. `Message<P>` itself implements
`Payload` (size = payload bytes + header size).

---

## Payload Trait

```rust
pub trait Payload {
    fn buffer_descriptor(&self) -> BufferDescriptor;
}
```

Returns the byte size of the payload for admission accounting and telemetry.
Memory class is **not** part of `BufferDescriptor` — it is owned by the
`MemoryManager` (see [Memory Model](memory_manager.md)).

---

## Tensor Types

Limen's primary payload currency for ML workloads:

- **`TensorRef<'a, T, N, R>`** — immutable typed tensor view. `T` is the
  element type, `N` is the element count, `R` is the rank. Backed by a
  fixed-size `[T; N]` buffer with a shape array `[usize; R]`.
- **`MutTensorRef<'a, T, N, R>`** — mutable tensor view for in-place writes.

Both implement `Payload` and carry shape metadata for inference backends.

---

## Batch Containers

- **`Batch<'a, P>`** — read-only slice of payloads for batch inference.
- **`BatchView<'a, T>`** — iterator-like view over a contiguous batch of
  `MessageToken` handles, used by edges for batch pop operations.

Batch boundaries are signalled via `MessageFlags::FIRST_IN_BATCH` and
`LAST_IN_BATCH` in the message header. The `BatchingPolicy` in `NodePolicy`
controls how batches are assembled (see [Policy Guide](policy.md)).

---

## Related

- [Memory Model](memory_manager.md) — how messages are stored and accessed
  via tokens
- [Edge Model](edge.md) — how message tokens flow between nodes
- [Policy Guide](policy.md) — batching semantics and admission rules
