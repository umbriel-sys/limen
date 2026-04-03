# Source Node Model

A source node has **0 inputs and >= 1 outputs**. Sources represent sensors,
file readers, protocol ingress, or any external data producer. They implement
the thin `Source` trait and are wrapped by `SourceNode` to satisfy the full
`Node` contract.

---

## Source Trait

```rust
pub trait Source<OutP: Payload, const OUT: usize> {
    type Error;

    fn open(&mut self) -> Result<(), Self::Error>;
    fn try_produce(&mut self) -> Option<(usize, Message<OutP>)>;
    fn ingress_occupancy(&self) -> EdgeOccupancy;
    fn peek_ingress_creation_tick(&self, item_index: usize) -> Option<u64>;
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT];
    fn capabilities(&self) -> NodeCapabilities;
    fn policy(&self) -> NodePolicy;
    fn ingress_policy(&self) -> EdgePolicy;
}
```

| Method | Purpose |
|---|---|
| `open()` | One-time initialisation (open device, connect) |
| `try_produce()` | Non-blocking: returns `Some((port, msg))` or `None` |
| `ingress_occupancy()` | Reports items/bytes *before* the source (e.g., device FIFO depth) |
| `peek_ingress_creation_tick(i)` | Non-destructive timestamp peek for batch span validation |

---

## SourceNode Adapter

`SourceNode<S, OutP, OUT>` wraps a `Source` and implements
`Node<0, OUT, (), OutP>`:

- `step()` calls `try_produce()` then `ctx.push_output()`.
- `step_batch()` respects `NodePolicy::batching` for fixed-N and delta-t
  windowing.
- `From<S>` is implemented — callers never need to name the adapter type.

---

## Ingress Edge

`SourceIngressEdge` is a no-alloc borrowing adapter that exposes
`Source::ingress_occupancy()` as an `Edge`. Only `occupancy()` and `is_empty()`
are meaningful; push and pop operations return `Rejected` / `Empty`.

This allows the scheduler to monitor source backpressure using the same
`EdgeOccupancy` interface as regular edges.

---

## Ingress Probes (std)

For concurrent graphs, a lock-free atomic-backed probe provides cross-thread
source pressure monitoring:

- `SourceIngressProbe` — shared `Arc<AtomicUsize>` counters
- `SourceIngressUpdater` — cloneable writer handle
- `SourceIngressProbeEdge<P>` — wraps probe as a payload-typed `Edge`

---

## Related

- [Node Model](node.md) — the full `Node` trait
- [Edge Model](edge.md) — how source outputs connect to downstream
- [Sink Nodes](sink.md) — the output-side counterpart
