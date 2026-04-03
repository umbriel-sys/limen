# ADR-005: Responsibility Division Across Subsystems

**Status:** Accepted
**Date:** 2025-10-01

## Context

A graph runtime has many concerns: data storage, message routing, payload
transformation, topology management, scheduling, and telemetry. Without clear
ownership boundaries, these concerns become entangled — making it impossible
to swap a single layer independently.

## Decision

Each subsystem has a strictly defined responsibility. No subsystem reaches
into another's domain:

| Subsystem | Responsibility | Does NOT |
|---|---|---|
| **MemoryManager** | Stores messages, issues tokens, provides header access | Route messages, enforce policies, know about nodes |
| **Edge** | Routes tokens between ports, enforces admission via `EdgePolicy` | Store payloads, know about node logic, schedule |
| **Node** | Transforms payloads, reports step result | Own queues, manage memory, choose scheduling order |
| **Graph** | Wires nodes and edges, builds `StepContext`, provides typed access | Execute steps, own runtime state, make scheduling decisions |
| **Runtime** | Drives the step loop, owns clock and telemetry, manages lifecycle | Know node internals, bypass graph API, own edges |
| **Scheduler** | Selects next node from summaries | Access edges directly, modify node state, own graph |

### Key Boundaries

- **Edges receive `&impl HeaderStore`, not `&impl MemoryManager<P>`.** This
  means edges can inspect message headers without knowing the payload type.
- **Nodes receive `StepContext`, not raw edge/manager references.** The
  context provides a curated, borrow-safe window into the graph.
- **Runtimes use `GraphApi`, not direct field access.** All graph interaction
  goes through the trait surface.
- **Schedulers receive `NodeSummary` snapshots, not live graph state.** This
  prevents scheduling decisions from mutating graph state.

## Rationale

- **Independent evolution.** Changing how memory is stored (e.g., adding a
  device memory manager) requires no changes to nodes, edges, or runtimes.
- **Testability.** Each subsystem can be tested in isolation with mock
  collaborators.
- **Swappability.** Users can provide custom edge implementations, custom
  runtimes, or custom memory managers — each independently.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Monolithic graph object | Simple API surface | Cannot swap layers independently, testing requires full graph | Coupling defeats portability |
| Node owns its edges | Natural "actor" model | Prevents external scheduling, complicates multi-rate graphs | Runtime cannot optimise |
| Edge owns message storage | Simpler edge API | Duplicates storage per edge, prevents zero-copy routing | Memory waste on MCU |

## Consequences

### Positive
- Any layer can be replaced without touching others.
- The codegen only needs to wire references — it doesn't embed runtime logic.
- Clear audit trail: each subsystem's invariants are documented in its trait.

### Trade-offs
- More indirection in the API (token → manager → message rather than
  direct access).
- `StepContext` has many type parameters (absorbed by codegen).

## Related ADRs

- [ADR-006](006_MEMORY_MODEL_MANAGER_CONTRACT.md) — memory manager specifics
- [ADR-007](007_EDGE_CONTRACT.md) — edge specifics
- [ADR-008](008_NODE_CONTRACT.md) — node specifics
- [ADR-009](009_GRAPH_CONTRACT_CODEGEN.md) — graph specifics
- [ADR-010](010_RUNTIME_CONTRACT.md) — runtime specifics
