# ADR-001: Workspace Structure and Crate Responsibilities

**Status:** Accepted
**Date:** 2025-10-01

## Context

Limen targets a wide range of hardware — from bare-metal Cortex-M4
microcontrollers to multi-threaded Linux servers. The codebase must cleanly
separate contracts from implementations, allow independent evolution of
runtimes and platforms, and keep the core contract crate free of optional
dependencies.

## Decision

Organise the project as a Cargo workspace with seven crates in a strict
dependency hierarchy:

```
limen-core          ← base contracts, no_std-first
  ├── limen-node    ← concrete node implementations
  ├── limen-runtime ← graph executors and schedulers
  ├── limen-platform← platform adapters (clock, affinity)
  └── limen-codegen ← DSL → Rust code generator
        └── limen-build ← proc-macro wrapper for codegen
limen-examples      ← integration tests (depends on all)
```

`limen-core` owns all traits, types, and contracts. It has no dependency on
any implementation crate. Implementation crates depend on `limen-core` but
never on each other (except `limen-build` depending on `limen-codegen`).

## Rationale

- **Isolation of contracts.** Changes to a runtime or platform adapter cannot
  break the core trait surface. Downstream crates can depend on `limen-core`
  alone.
- **Independent versioning.** Runtime and platform crates can release at
  different cadences than core.
- **Feature flag hygiene.** Only `limen-core` defines the `alloc`/`std`/
  `spsc_raw` feature hierarchy. Downstream crates re-export or gate on the
  same flags without introducing new ones.
- **Build isolation.** `limen-codegen` depends on `syn`/`quote`/`proc-macro2`
  — heavy build dependencies that must not leak into embedded builds of
  `limen-core`.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Monorepo single crate | Simpler build, single version | Feature explosion, cannot depend on contracts alone, proc-macro deps leak everywhere | Breaks embedded builds |
| Traits crate + impl crate (2-crate) | Cleaner than monolith | Doesn't isolate codegen deps; runtime and platform tightly coupled | Insufficient separation |
| Workspace with finer splits (10+ crates) | Maximum isolation | Excessive dependency management, slower CI | Over-engineering at this scale |

## Consequences

### Positive
- Clear ownership: each crate has a single responsibility.
- Embedded users depend only on `limen-core` — no `syn`/`quote` in their build.
- Integration tests in `limen-examples` exercise the full stack without
  circular dependencies.

### Trade-offs
- Seven crates means seven `Cargo.toml` files and version strings to maintain.
- Feature flags must be kept in sync across crates manually (mitigated by CI).

## Related ADRs

- [ADR-003](003_NO_STD_ALLOC_STD_UNSAFE_DIVISION.md) — feature flag hierarchy
- [ADR-005](005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) — crate-level responsibility boundaries
