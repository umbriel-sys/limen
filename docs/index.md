# Limen Documentation

Limen is a Rust framework for portable, contract-enforcing computation graphs
targeting AI-enabled embedded systems. Node and policy code is identical from
bare-metal `no_std` microcontrollers to multi-threaded Linux servers. Edge and
memory manager types must currently be switched between targets — a planned
zero-lock implementation ([ADR-013](ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md))
will close this gap.

---

## Architecture

The [Architecture Guide](architecture/index.md) covers the full system design.
Individual subsystem documents are listed below.

### Core Abstractions

| Document | Topic |
|---|---|
| [Message and Payload](architecture/message_payload.md) | Message envelope, headers, payload trait, tensors, batches |
| [Memory Model](architecture/memory_manager.md) | Token-based zero-copy storage, guard API, memory classes |
| [Edge Model](architecture/edge.md) | SPSC queue contract, implementations, admission flow |
| [Node Model](architecture/node.md) | Uniform processing contract, StepContext, step lifecycle |
| [Source Nodes](architecture/source.md) | Sensor/ingress adapter, ingress edge, probes |
| [Sink Nodes](architecture/sink.md) | Output adapter, input selection |
| [Inference Nodes](architecture/model.md) | ComputeBackend, ComputeModel, InferenceModel adapter |
| [Graph Model](architecture/graph.md) | Compile-time typed topology, GraphApi, ScopedGraphApi |
| [Runtime Model](architecture/runtime.md) | Executor contract, runtime tiers, scheduling |
| [Telemetry](architecture/telemetry.md) | Zero-cost metrics, structured events, aggregation |
| [Platform](architecture/platform.md) | Clock abstraction, platform boundary |
| [Policy Guide](architecture/policy.md) | Admission, batching, deadline, and budget policies |
| [Graph Codegen](architecture/codegen.md) | Three graph definition approaches, DSL, validation |

### Execution Flow

| Document | Topic |
|---|---|
| [no_std Graph Flow](architecture/graph_flow_no_std.md) | Single-threaded execution on bare-metal targets |
| [Concurrent Graph Flow](architecture/graph_flow_concurrent.md) | Multi-threaded execution with scoped worker threads |

---

## Decision Records

Architecture Decision Records (ADRs) document the rationale behind key design
choices. Each record explains the context, the decision, alternatives
considered, and consequences.

| ADR | Decision |
|---|---|
| [001](ADRs/001_WORKSPACE_STRUCTURE_CRATE_RESPONSIBILITIES.md) | Workspace structure and crate responsibilities |
| [002](ADRs/002_TRAIT_FIRST_ZERO_DYN_CONTRACTS.md) | Trait-first, zero dynamic dispatch contracts |
| [003](ADRs/003_NO_STD_ALLOC_STD_UNSAFE_DIVISION.md) | `no_std` / `alloc` / `std` / `unsafe` division |
| [004](ADRs/004_PAYLOAD_FIRST_CONTRACTS.md) | Payload-first contracts |
| [005](ADRs/005_MEMORY_MANAGER_EDGE_NODE_GRAPH_RUNTIME_RESPONSIBILITIES.md) | Responsibility division across subsystems |
| [006](ADRs/006_MEMORY_MODEL_MANAGER_CONTRACT.md) | Memory model and manager contract |
| [007](ADRs/007_EDGE_CONTRACT.md) | Edge contract |
| [008](ADRs/008_NODE_CONTRACT.md) | Node contract |
| [009](ADRs/009_GRAPH_CONTRACT_CODEGEN.md) | Graph contract and code generation |
| [010](ADRs/010_RUNTIME_CONTRACT.md) | Runtime contract |
| [011](ADRs/011_BATCH_SEMANTICS.md) | Batch semantics |
| [012](ADRs/012_ROBOTICS_READINESS.md) | Robotics readiness primitives |
| [013](ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md) | Zero-lock, zero-copy concurrent graphs |

---

## Guides

| Document | Description |
|---|---|
| [Roadmap](roadmap.md) | Phased plan to v0.1.0, v1, and stretch goals |
| [Development Guide](dev_guide.md) | Building, testing, feature flags, code style, and contribution workflow |
