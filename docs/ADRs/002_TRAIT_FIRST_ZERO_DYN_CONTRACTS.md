# ADR-002: Trait-First, Zero Dynamic Dispatch Contracts

**Status:** Accepted
**Date:** 2025-10-01

## Context

Limen targets bare-metal microcontrollers where vtable dispatch and heap
allocation are unacceptable overheads. At the same time, the framework must
be pluggable — users must be able to swap queue implementations, inference
backends, and clock sources without modifying core code.

## Decision

All hot-path abstractions are defined as generic traits parameterised by
concrete types and const generics. No `Box<dyn Trait>`, `dyn Fn`, or
vtable-based patterns are used in any code that runs per-step.

Key traits:
- `Node<const IN, const OUT, InP, OutP>` — node contract
- `Edge` — SPSC queue contract
- `MemoryManager<P>` — storage contract
- `Telemetry` — metrics and events contract
- `PlatformClock` — time contract

All are monomorphized at compile time. The codegen layer generates the concrete
type parameters so application developers don't write them by hand.

## Rationale

- **Zero overhead.** Monomorphization produces the same machine code as
  hand-written, type-specific implementations. No vtable lookup, no indirect
  call, no heap allocation.
- **LTO-friendly.** Concrete types enable aggressive link-time optimisation
  across crate boundaries.
- **`no_std` compatible.** No heap is required for dispatch. `StaticRing<N>`
  and `StaticMemoryManager<P, DEPTH>` are stack-allocated.
- **Codegen absorbs the ergonomic cost.** The boilerplate of fully-qualified
  generic types is generated once at build time.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| `dyn Trait` everywhere | Simple, familiar | Vtable overhead, requires heap for `Box<dyn>`, incompatible with `no_std`/no_alloc | Fundamental constraint violation |
| `dyn Trait` behind feature flag | Ergonomic on `std` targets | Two code paths, testing burden doubles, `no_std` path still needs generics | Complexity for no benefit |
| Enum dispatch | No heap, no vtable | Closed set of implementations, combinatorial explosion | Doesn't scale |

## Consequences

### Positive
- Predictable, deterministic performance on MCU targets.
- Single code path for all targets — no `std`-only convenience wrappers.
- Generated code is fully type-checked at compile time.

### Trade-offs
- Verbose type signatures in generated code (intentional — codegen absorbs
  this cost).
- Longer compile times due to monomorphization (mitigated by incremental
  compilation).
- Cannot dynamically load or swap graph components at runtime.

## Related ADRs

- [ADR-003](003_NO_STD_ALLOC_STD_UNSAFE_DIVISION.md) — no-heap default
- [ADR-009](009_GRAPH_CONTRACT_CODEGEN.md) — how codegen removes the boilerplate cost
