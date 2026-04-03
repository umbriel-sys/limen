# ADR-003: no_std / alloc / std / unsafe Division

**Status:** Accepted
**Date:** 2025-10-01

## Context

Limen must run on three classes of target: bare-metal MCU (no heap, no OS),
embedded Linux (heap available, POSIX), and desktop/server (full `std`). The
codebase must compile correctly on all three without conditional compilation
in application code.

## Decision

Feature flags are **additive** and form a strict hierarchy:

```
(default)  →  no_std, no heap, fixed-size structures
  alloc    →  adds heap-backed queues, Vec-based batches
    std    →  implies alloc, adds concurrent queues, threaded runtimes, I/O
```

`std` implies `alloc`. `alloc` implies nothing about `std`. Feature flags must
never be exclusive or conflicting.

`unsafe` code is confined to a single module (`spsc_raw`) gated behind its
own feature flag. All other hot-path code is safe Rust.

### Rules

1. All code in `limen-core`, `limen-node`, `limen-runtime`, and
   `limen-platform` must compile without `std`.
2. Heap use must be gated behind `#[cfg(feature = "alloc")]`.
3. `std`-only types (`Arc`, `Mutex`, `RwLock`, `Vec` in `std` context) are
   gated behind `#[cfg(feature = "std")]`.
4. No `println!` or `eprintln!` — use the `Telemetry` trait.
5. No `unsafe` outside `spsc_raw`. The `spsc_raw` feature requires `std`.
6. `derive(Debug)` only where verified to compile under `no_std`.

## Rationale

- **Bare-metal is the hardest target.** If it compiles and runs there, it
  works everywhere. Building from `no_std` up is strictly easier than
  stripping `std` down.
- **Additive flags prevent conflicts.** There is never a situation where
  enabling two features breaks the build.
- **`unsafe` confinement.** The lock-free ring buffer (`SpscAtomicRing`) is
  inherently unsafe. Confining it to one opt-in module means the default build
  is 100% safe Rust.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| `std` default, `no_std` feature | Easier for desktop users | Embedded users must opt in; easy to accidentally depend on `std` | Wrong default for target audience |
| Separate crates per feature level | Clean separation | Trait duplication, triple the maintenance | Complexity without benefit |
| Allow `unsafe` anywhere with `unsafe` blocks | More flexibility | Harder to audit, safety claims weaken | Safety is a selling point |

## Consequences

### Positive
- Same graph definition compiles under all three configurations.
- CI matrix enforces correctness across the feature matrix.
- Default build is safe, heap-free, and MCU-ready.

### Trade-offs
- Every new type or function must consider feature gating up front.
- Some ergonomic patterns (e.g., `format!`, `to_string()`) are unavailable
  in the default build.

## Related ADRs

- [ADR-001](001_WORKSPACE_STRUCTURE_CRATE_RESPONSIBILITIES.md) — crate isolation
- [ADR-002](002_TRAIT_FIRST_ZERO_DYN_CONTRACTS.md) — zero dynamic dispatch
