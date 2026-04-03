# Graph Model

The graph is the compile-time typed topology that wires nodes and edges
together. Generated graph structs implement `GraphApi` — the unified surface
that all runtimes program against. Compile-time access traits provide
type-safe, zero-overhead access to individual nodes and edges by const-generic
index.

---

## Compile-Time Access Traits

### GraphNodeAccess\<I\>

```rust
pub trait GraphNodeAccess<const I: usize> {
    type Node;
    fn node_ref(&self) -> &Self::Node;
    fn node_mut(&mut self) -> &mut Self::Node;
}
```

### GraphEdgeAccess\<E\>

```rust
pub trait GraphEdgeAccess<const E: usize> {
    type Edge;
    fn edge_ref(&self) -> &Self::Edge;
    fn edge_mut(&mut self) -> &mut Self::Edge;
}
```

### GraphNodeTypes\<I, IN, OUT\>

Associates payload, queue, and memory manager types with node `I` at compile
time:

```rust
pub trait GraphNodeTypes<const I: usize, const IN: usize, const OUT: usize> {
    type InP: Payload;
    type OutP: Payload;
    type InQ: Edge;
    type OutQ: Edge;
    type InM: MemoryManager<Self::InP>;
    type OutM: MemoryManager<Self::OutP>;
}
```

---

## Context Factory

### GraphNodeContextBuilder\<I, IN, OUT\>

Builds the `StepContext` for a specific node, handling borrow safety:

```rust
pub trait GraphNodeContextBuilder<const I: usize, const IN: usize, const OUT: usize>:
    GraphNodeTypes<I, IN, OUT>
{
    fn make_step_context<C, T>(
        &mut self, clock: &C, telemetry: &mut T,
    ) -> StepContext<...>;

    fn with_node_and_step_context<C, T, R, E>(
        &mut self, clock: &C, telemetry: &mut T, f: F,
    ) -> Result<R, E>;
}
```

`with_node_and_step_context` lends both `&mut node(I)` and a fully wired
`StepContext` to a closure in a single `&mut self` borrow — avoiding the
overlapping mutable reference problem that would arise from separate access.

---

## GraphApi

The unified runtime-facing interface:

```rust
pub trait GraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    // Descriptors
    fn get_node_descriptors(&self) -> [NodeDescriptor; NODE_COUNT];
    fn get_edge_descriptors(&self) -> [EdgeDescriptor; EDGE_COUNT];
    fn get_node_policies(&self) -> [NodePolicy; NODE_COUNT];
    fn get_edge_policies(&self) -> [EdgePolicy; EDGE_COUNT];
    fn validate_graph(&self) -> Result<(), GraphError>;

    // Occupancy
    fn edge_occupancy_for<const E: usize>(
        &self,
    ) -> Result<EdgeOccupancy, GraphError>;

    fn write_all_edge_occupancies(
        &self, out: &mut [EdgeOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
        &self, out: &mut [EdgeOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    // Stepping
    fn step_node_by_index<C, T>(
        &mut self, index: usize, clock: &C, telemetry: &mut T,
    ) -> Result<StepResult, NodeError>;

    // Policy query
    fn node_policy_for<const I: usize, const IN: usize, const OUT: usize>(
        &self,
    ) -> NodePolicy;
}
```

### Occupancy Semantics

- `write_all_edge_occupancies` — atomic snapshot of **all** edge occupancies.
  Overwrites the entire output buffer.
- `refresh_occupancies_for_node<I>` — **partial** in-place update. Only
  edges incident to node `I` are refreshed; other slots are untouched.

---

## ScopedGraphApi (std)

Extension trait for concurrent execution via scoped threads:

```rust
#[cfg(feature = "std")]
pub trait ScopedGraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize>:
    GraphApi<NODE_COUNT, EDGE_COUNT>
{
    fn run_scoped<C, T, S>(
        &mut self, clock: C, telemetry: T, scheduler: S,
    ) where
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
        S: WorkerScheduler + 'static;
}
```

Spawns one scoped thread per node. Each worker calls `scheduler.decide()`
before every step. Requires all edges to implement `ScopedEdge` and all
managers to support concurrent access.

---

## Graph Validation

The codegen validates graphs at build time. The following invariants are
enforced:

1. Node indices must be contiguous `0..N`; edge indices contiguous `0..M`.
2. Edge endpoints must reference valid node port indices.
3. Edge payload type must match node input/output payload.
4. All inbound edges to a node share the same queue type; all outbound edges
   share the same queue type.
5. Ingress edges (synthetic, source nodes only) must have the lowest global
   edge indices.

`validate_graph()` also performs runtime validation when called explicitly.

---

## Related

- [Node Model](node.md) — what the graph steps
- [Edge Model](edge.md) — what the graph wires
- [Runtime Model](runtime.md) — what programs against `GraphApi`
- [Graph Codegen](codegen.md) — how graphs are generated
