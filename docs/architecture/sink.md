# Sink Node Model

A sink node has **>= 1 inputs and 0 outputs**. Sinks represent actuators,
serial output, file writers, network publishers, or any terminal consumer.
They implement the thin `Sink` trait and are wrapped by `SinkNode` to satisfy
the full `Node` contract.

---

## Sink Trait

```rust
pub trait Sink<InP: Payload, const IN: usize> {
    type Error;

    fn open(&mut self) -> Result<(), Self::Error>;
    fn consume(&mut self, msg: &Message<InP>) -> Result<(), Self::Error>;
    fn input_acceptance(&self) -> [PlacementAcceptance; IN];
    fn capabilities(&self) -> NodeCapabilities;
    fn policy(&self) -> NodePolicy;
    fn select_input(&mut self, occ: &[EdgeOccupancy; IN]) -> Option<usize>;
}
```

| Method | Purpose |
|---|---|
| `open()` | One-time initialisation (open device, connect) |
| `consume(msg)` | Side-effect entry point (write, print, publish) |
| `select_input(occ)` | Choose which input port to drain next. Default: first non-empty. |

`select_input` takes `&mut self` to allow stateful port selection strategies
(e.g., round-robin, priority-based).

---

## SinkNode Adapter

`SinkNode<S, InP, IN>` wraps a `Sink` and implements `Node<IN, 0, InP, ()>`:

- `step()` snapshots occupancies, calls `sink.select_input()`, delegates to
  `ctx.pop_and_process()` with `sink.consume()`.
- `step_batch()` delegates to `ctx.pop_batch_and_process()`.
- `From<S>` is implemented — callers never need to name the adapter type.

---

## Related

- [Node Model](node.md) — the full `Node` trait
- [Source Nodes](source.md) — the input-side counterpart
- [Edge Model](edge.md) — how upstream outputs connect to sink inputs
