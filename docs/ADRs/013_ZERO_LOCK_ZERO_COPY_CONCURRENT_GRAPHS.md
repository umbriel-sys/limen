# ADR-013: Zero-Lock, Zero-Copy Concurrent Graphs

**Status:** Proposed
**Date:** 2026-04-03

## Context

Limen currently requires different edge and memory manager types depending on
the execution model:

- **Single-threaded** (`no_std`): `StaticRing` + `StaticMemoryManager` — zero
  overhead, but cannot be shared across threads.
- **Multi-threaded** (`std`): `ConcurrentEdge` (Arc/Mutex wrapper) +
  `ConcurrentMemoryManager` (per-slot RwLock) — thread-safe, but requires
  heap allocation and introduces lock contention.

This means a graph must be defined with different types depending on whether it
will run single-threaded or multi-threaded. The vision of "same graph, any
target" is incomplete — it holds for node and policy code, but not for edge
and memory manager selection.

Additionally, the current `std`-only concurrency model (`ScopedGraphApi`)
requires `Arc`, `Mutex`, and `RwLock` — none of which are available in
`no_std`. There is no path to concurrent execution on bare-metal targets
(e.g., multi-core Cortex-M or asymmetric multiprocessing systems).

## Decision

### Part 1: Zero-Lock Concurrent Edge and Memory Manager

Introduce a new edge implementation and memory manager that are:

- **`no_alloc`** — fixed-size, stack-allocatable, no heap.
- **Lock-free** — use atomic operations and raw pointers for concurrent access.
  No `Arc`, no `Mutex`, no `RwLock`.
- **Zero-copy** — slot storage uses `UnsafeCell<MaybeUninit<T>>`, enabling
  nodes to mutate payload data in place without store/free overhead. True
  zero-copy processing for the entire graph.
- **Internally `unsafe`, externally safe** — the `unsafe` is confined to the
  implementation module (same principle as `spsc_raw`). The public API is safe.
- **Unified** — the same edge and manager types work in both single-threaded
  and multi-threaded execution. A graph using these types runs on a `no_std`
  single-threaded runtime *and* a multi-threaded runtime without changing types.

#### Tentative Design

**Lock-Free Edge (`AtomicRing<N>`):**
- Fixed-capacity ring buffer of `MessageToken` using atomic head/tail pointers.
- Single-producer, single-consumer (SPSC) — no need for MPMC complexity since
  each edge connects exactly one output port to one input port.
- Backing storage is `[UnsafeCell<MaybeUninit<MessageToken>>; N]` — `UnsafeCell`
  permits interior mutability without a lock, while `MaybeUninit` avoids
  requiring `Default` or initialising unused slots.
- Implements both `Edge` and `ScopedEdge` — the scoped handle is a pointer
  to the same ring, valid for the lifetime of the scoped thread.

**Lock-Free Memory Manager (`AtomicMemoryManager<P, DEPTH>`):**
- Fixed-capacity slot array with atomic per-slot state (Free/Occupied/Reading).
- Slot storage is `[UnsafeCell<MaybeUninit<Message<P>>>; DEPTH]`:
  - `UnsafeCell` enables zero-copy in-place mutation — nodes can transform
    payload data directly in the slot without a store/free round-trip.
  - `MaybeUninit` avoids requiring `Default` on `P` and eliminates
    initialisation cost for unused slots.
- Lock-free freelist using atomic CAS on a stack head pointer.
- Read access returns a guard that atomically transitions the slot from
  Occupied to Reading and back on drop — no `RwLock`.
- Write/mutate access returns a guard that provides `&mut Message<P>` via
  the `UnsafeCell`, enabling true zero-copy processing for in-place
  transforms.
- `HeaderStore` implementation uses the same atomic state machine for
  header-only access.

**Key invariant:** Both types are `Send + Sync` without requiring `alloc`.
They compile and work correctly under `no_std`.

### Part 2: Cooperative Concurrent Execution on `no_std` (Exploratory)

With lock-free edges and managers, a second opportunity opens: concurrent
execution on `no_std` targets without OS threads. Three approaches are
considered:

#### Option A: Cooperative Work-Stealing Runtime

A single-threaded runtime that simulates concurrency by interleaving **work
units** rather than whole node steps:

- Each node's `step()` is decomposed into a sequence of work units (pop,
  process, push) expressed as a state machine or coroutine.
- The runtime maintains a priority queue of pending work units across all
  nodes.
- On each tick, the runtime selects the highest-priority work unit (based on
  node policy, urgency, deadline) and executes it.
- This achieves fine-grained scheduling without threads — a high-priority
  node's push can preempt a low-priority node's process.

**Feasibility:** Moderate. Requires either:
- Nodes to express their work as an explicit state machine (breaking change to
  `Node` trait), or
- A `yield`-point mechanism where `StepContext` methods (pop, push) return
  suspension points that the runtime can interleave.

**Trade-off:** Significant complexity. The `Node` trait would need a new
execution model (state machine or generator-based). Not suitable for v0.1.0.

#### Option B: Async-Style Suspend/Resume via Generators

Use Rust's nightly generator/coroutine feature (or a polled future model) to
turn `step()` into a suspendable computation:

- `step()` returns a `Poll`-like result at yield points.
- The runtime resumes different nodes' generators in priority order.
- No OS threads, no allocator — generators are stack-allocated state machines.

**Feasibility:** Low-to-moderate. Depends on stabilisation of Rust generator
syntax. A polled future model is possible on stable Rust but requires manual
state machine implementation per node, which defeats the ergonomic goal.

**Trade-off:** Tied to nightly Rust or requires significant manual work on
stable. Not practical until generators stabilise.

#### Option C: Multi-Core `no_std` with Hardware Threads

On multi-core `no_std` targets (e.g., dual-core Cortex-M, RP2040), use
platform-specific thread spawning (not `std::thread`) with the lock-free
edges and managers:

- The runtime assigns nodes to cores at init time.
- Each core runs its own step loop using lock-free edges for cross-core
  communication.
- No OS, no allocator — just atomic operations on shared memory.

**Feasibility:** High for the specific case of multi-core `no_std` targets.
Requires a `no_std` thread/core abstraction in `limen-platform`, but no
changes to the `Node` trait or `Edge` API.

**Trade-off:** Platform-specific. Only applicable to multi-core targets.
Does not help single-core MCUs.

#### Recommendation

**Option C is the most practical near-term path.** Lock-free edges and
managers (Part 1) are the prerequisite. Once those exist, multi-core `no_std`
execution requires only a platform-specific thread spawning mechanism — the
graph, nodes, edges, and managers are already concurrent-safe.

**Option A is the most interesting long-term vision** for single-core targets.
It should be explored as a research direction after v0.1.0, likely requiring
a new `AsyncNode` trait or a generator-based extension to `StepContext`.

**Option B** should be revisited when Rust generators stabilise.

## Rationale

- **True "same graph, any target."** A graph using `AtomicRing` and
  `AtomicMemoryManager` compiles and runs correctly on a single-threaded
  `no_std` runtime, a multi-threaded `std` runtime, and a multi-core `no_std`
  runtime — with zero type changes.
- **True zero-copy.** `UnsafeCell<MaybeUninit<T>>` slot storage allows nodes
  to mutate data in place for the whole graph, eliminating store/free overhead.
- **No lock contention.** SPSC atomic ring buffers have no contention by
  design (one producer, one consumer). The memory manager uses per-slot atomic
  state instead of locks.
- **`unsafe` confinement.** Follows the same principle as `spsc_raw`:
  internally `unsafe`, externally safe API, gated behind a feature flag.
- **Enables future concurrency models.** Lock-free primitives are the
  prerequisite for all three `no_std` concurrency options above.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Keep separate types per execution model | Already works | Violates "same graph, any target" for edge/manager types | Incomplete vision |
| Use `crossbeam` channels | Battle-tested lock-free | Requires `std`, not `no_alloc` | Not `no_std` compatible |
| Global lock on entire graph | Simple concurrency | Serialises all access, defeats the purpose | No parallelism gain |

## Consequences

### Positive
- A single graph type works across all execution models.
- Lock-free primitives enable multi-core `no_std` execution.
- Opens the door to cooperative scheduling research on single-core targets.

### Trade-offs
- New `unsafe` code requires careful review and testing (mitigated by
  conformance test suite + Miri testing).
- Atomic operations have a small overhead on single-threaded targets where
  they are unnecessary (mitigated: compiler can elide uncontested atomics
  on single-core targets).
- Feature-gated to opt-in — users who don't need concurrency use
  `StaticRing` / `StaticMemoryManager` with zero overhead.

## Design Constraints

All current design decisions should **enable** this plan, even though
implementation is deferred. Specifically:

1. **Edge trait must not assume `&mut self` exclusivity.** The current `Edge`
   trait takes `&mut self`, which is correct for single-threaded use and
   compatible with lock-free implementations via `UnsafeCell`.
2. **`MemoryManager` guard types must remain associated types.** This allows
   lock-free implementations to provide atomic guards without changing the
   trait surface.
3. **`ScopedEdge` handles must not require `Arc`.** The trait specifies
   `Handle<'a>: Edge + Send + 'a` — this is compatible with raw-pointer-based
   handles.
4. **`HeaderStore` must remain a supertrait of `MemoryManager`.** Lock-free
   header peek is straightforward given per-slot atomic state.

## Dependencies

- Part 1 is independent of the v0.1.0 roadmap but should be informed by it.
- Part 2 (Options A/B) require post-v0.1.0 trait evolution.
- Part 2 (Option C) requires `limen-platform` multi-core abstractions.

## Related ADRs

- [ADR-002](002_TRAIT_FIRST_ZERO_DYN_CONTRACTS.md) — zero dynamic dispatch
- [ADR-003](003_NO_STD_ALLOC_STD_UNSAFE_DIVISION.md) — `unsafe` confinement
- [ADR-006](006_MEMORY_MODEL_MANAGER_CONTRACT.md) — memory manager contract
- [ADR-007](007_EDGE_CONTRACT.md) — edge contract
- [ADR-010](010_RUNTIME_CONTRACT.md) — runtime contract
