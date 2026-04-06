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

## Tensor Type

Limen's primary payload currency for ML workloads:

- **`Tensor<T, N, R>`** — owned, inline, fixed-capacity tensor. `T` is the
  element scalar type (`Copy + Default + DType`), `N` is the max element
  capacity (compile-time const), `R` is the rank. Stored inline as
  `[T; N]` with a shape array `[usize; R]`.

`Tensor` is `Copy` and requires no heap, enabling zero-allocation message
pipelines. It implements `Payload` with byte size computed from the live
element count (`len * size_of::<T>()`).

Rank-specific constructors are provided: `nhwc(n, h, w, c, data)`,
`nc(n, c, data)`, `from_slice(data)`, `from_shape(shape, data)`.

---

## Batch Containers

- **`Batch<'a, P>`** — read-only slice of payloads for batch inference.
- **`BatchView<'a, T>`** — iterator-like view over a contiguous batch of
  `MessageToken` handles, used by edges for batch pop operations.
- **`BatchMessageIter<'edge, 'mgr, P, M>`** — lazy token-resolving iterator
  that reads messages from a `MemoryManager` on demand. Used internally by
  `step_batch` to iterate over batched tokens without copying all messages
  up front.

Batch boundaries are signalled via `MessageFlags::FIRST_IN_BATCH` and
`LAST_IN_BATCH` in the message header. The `BatchingPolicy` in `NodePolicy`
controls how batches are assembled (see [Policy Guide](policy.md)).

---

## Related

- [Memory Model](memory_manager.md) — how messages are stored and accessed
  via tokens
- [Edge Model](edge.md) — how message tokens flow between nodes
- [Policy Guide](policy.md) — batching semantics and admission rules
