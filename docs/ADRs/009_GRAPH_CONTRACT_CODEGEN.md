# ADR-009: Graph Contract and Code Generation

**Status:** Accepted
**Date:** 2025-11-01

## Context

A zero-dynamic-dispatch graph system requires fully-typed wiring between nodes,
edges, and memory managers. Writing this by hand for even a simple three-node
graph produces hundreds of lines of trait implementations. This boilerplate is
a barrier to adoption.

## Decision

### Compile-Time Typed Graph

The graph is a concrete struct with typed node and edge tuples. The `GraphApi`
trait provides the unified runtime-facing interface:

```rust
pub trait GraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    fn step_node_by_index<C, T>(
        &mut self, index: usize, clock: &C, telemetry: &mut T,
    ) -> Result<StepResult, NodeError>;
    // ... descriptors, occupancy, validation
}
```

Compile-time access traits (`GraphNodeAccess<I>`, `GraphEdgeAccess<E>`,
`GraphNodeTypes<I, IN, OUT>`) provide zero-overhead typed access to individual
nodes and edges by const-generic index.

### Three Code Generation Approaches

No forced tooling — users choose the approach that fits their workflow:

1. **Proc-macro** (`define_graph!`) — inline in source. Fast iteration.
2. **Build-script DSL** (`expand_str_to_file`) — explicit, no incremental
   build overhead.
3. **Typed builder API** (`GraphBuilder`) — language-server-friendly,
   strongly typed, no DSL parsing.

All three produce identical generated code.

### Validation at Build Time

The codegen validates:
1. Contiguous node/edge indices.
2. Port bounds.
3. Payload type compatibility.
4. Queue type homogeneity per node.

Invalid graphs fail the build with descriptive error messages.

## Rationale

- **Codegen removes the abstraction tax.** The ergonomic cost of
  monomorphization is paid once, at build time, not by application developers.
- **No forced tooling.** Proc-macro users get inline convenience.
  Build-script users get explicit control. Builder API users get
  language-server support. All three are first-class.
- **Identical output.** There is one code generator (`limen-codegen`).
  The proc-macro (`limen-build`) and DSL parser are thin wrappers.
- **Build-time validation.** Structural errors are caught at compile time,
  not at runtime.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| Hand-written graphs only | No build dependency | Hundreds of lines of boilerplate per graph | Adoption barrier |
| Proc-macro only | Simple entry point | No build-script control, longer incremental builds | Limits user choice |
| Runtime graph construction | Dynamic topology | Requires heap, cannot validate at compile time, vtable dispatch | Against core design |

## Consequences

### Positive
- Three-node graph defined in ~20 lines; codegen emits ~300 lines of
  fully-typed implementations.
- Same graph definition produces both `no_std` and `std` (concurrent)
  variants.
- Build-time validation catches wiring errors before any code runs.

### Trade-offs
- Generated code is verbose (intentionally — monomorphization is the goal).
- Users must not hand-edit generated files in `OUT_DIR`.
- `limen-codegen` depends on `syn`/`quote` — isolated in its own crate to
  avoid polluting embedded builds.

## Related ADRs

- [ADR-002](002_TRAIT_FIRST_ZERO_DYN_CONTRACTS.md) — why monomorphization requires codegen
- [ADR-001](001_WORKSPACE_STRUCTURE_CRATE_RESPONSIBILITIES.md) — codegen crate isolation
- [ADR-008](008_NODE_CONTRACT.md) — what gets wired
