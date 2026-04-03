# ADR-011: Batch Semantics

**Status:** Accepted
**Date:** 2025-12-01

## Context

ML inference is most efficient when operating on batches of inputs. However,
batching in a streaming graph is non-trivial: batch size, time constraints,
window overlap, and memory management must all be coordinated. This
coordination must not be embedded in node logic — nodes should focus on
payload transformation.

## Decision

Batching is a declarative policy defined at graph construction time and
enforced by the framework:

```rust
pub struct BatchingPolicy {
    fixed_n: Option<usize>,         // Fixed items per batch
    max_delta_t: Option<Ticks>,     // Max time span (first to last item)
    window_kind: WindowKind,        // Disjoint or Sliding
}
```

### Assembly

Batches are assembled by `StepContext::pop_batch_and_process()`:
1. Pop up to `fixed_n` items from the input edge.
2. If `max_delta_t` is set, stop if the time span exceeds the limit.
3. Window kind controls reuse: `Disjoint` consumes items; `Sliding` advances
   by stride.

### Signalling

Batch boundaries are signalled in `MessageFlags`:
- `FIRST_IN_BATCH` on the first item.
- `LAST_IN_BATCH` on the last item.

### Integration with Inference

`InferenceModel` uses `step_batch` which:
1. Assembles a batch per `NodePolicy::batching`.
2. Caps at `min(policy.fixed_n, backend.max_batch, MAX_BATCH)`.
3. Calls `ComputeModel::infer_batch()`.
4. Frees stride tokens after processing.

In `no_std` builds, batch scratch buffers are stack-allocated (bounded by
`MAX_BATCH`). In `alloc` builds, `Vec`-backed batch paths are available.

## Rationale

- **Declarative, not imperative.** Nodes don't implement batching logic.
  Changing batch size is a policy change, not a code change.
- **Time-bounded.** `max_delta_t` prevents unbounded latency from waiting
  for a full batch that may never arrive.
- **Sliding windows.** Enables overlapping signal processing (e.g., audio
  frames with 50% overlap) without custom node logic.
- **Stack-allocated by default.** `MAX_BATCH` const generic bounds the
  scratch buffer, keeping `no_std` builds heap-free.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Nodes implement their own batching | Maximum flexibility | Duplicated logic, inconsistent behaviour across nodes, no policy control | Against policy-agnostic nodes |
| Fixed batch size only | Simple | Cannot handle variable-rate inputs, no time bounds | Insufficient for real-time |
| Edge-level batching | Natural grouping point | Edge doesn't know node policy; complicates admission | Wrong layer |

## Consequences

### Positive
- Batch size is tunable per-node without code changes.
- Time bounds prevent latency spikes from incomplete batches.
- Inference nodes get optimised batch paths automatically.

### Trade-offs
- `MAX_BATCH` must be chosen at compile time for `no_std` builds.
- Sliding windows with small strides increase message reprocessing.

## Related ADRs

- [ADR-008](008_NODE_CONTRACT.md) — how nodes interact with batching
- [ADR-004](004_PAYLOAD_FIRST_CONTRACTS.md) — payload types in batches
