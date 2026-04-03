# Limen

> **Portable, contract-enforcing computation graphs for AI-enabled embedded systems — from bare-metal microcontrollers to multi-threaded servers, in safe Rust.**

---

## The Problem

Building AI-enabled robotics and edge computing systems today means rebuilding the same software pipeline for every hardware target. A perception → inference → actuation pipeline written for a Cortex-M4 cannot run on a Raspberry Pi or a server without substantial rework — despite implementing identical logic.

No existing framework bridges this gap cleanly:

| Framework | Portable Graphs | Inference Built-in | no_std | Contract Enforcement |
|---|---|---|---|---|
| ROS2 | ✓ | ✗ | ✗ | ✗ |
| Embassy | ✗ | ✗ | ✓ | ✗ |
| RTIC | ✗ | ✗ | ✓ | ✗ |
| Burn / Tract | ✗ | ✓ | Partial | ✗ |
| TFLite Micro | ✗ | ✓ | ✓ | ✗ |
| **Limen** | **✓** | **✓** | **✓** | **✓** |

Limen closes this gap.

---

## What Limen Is

Limen is a Rust framework for defining **portable, policy-enforcing computation graphs** that run identically from `no_std` Cortex-M4 microcontrollers to multi-threaded Linux servers.

You define your pipeline **once** — nodes, edges, policies, inference backends — using a declarative builder API. A code generation layer produces a fully type-safe, zero-dynamic-dispatch graph struct. A pluggable runtime then executes that same graph on any target. Swapping hardware requires changing only the platform and runtime implementation, not the graph or node logic.

### Core Properties

- **Single graph definition, any target.** The same graph runs on a bare-metal MCU or a server with no changes to application code.
- **Zero dynamic dispatch in the hot path.** Nodes and edges are monomorphized at compile time. No vtables, no heap allocation required.
- **Contract enforcement, not just observability.** Freshness, liveness, criticality, and backpressure are first-class runtime contracts, not logging conveniences.
- **Inference as a first-class citizen.** A `ComputeBackend` trait abstracts TFLite Micro on MCU and Tract/TFLite on desktop behind the same model node, within the same graph.
- **Safe Rust throughout.** No `unsafe` in the hot path. Memory safety without a runtime.
- **Codegen removes all boilerplate.** Developers describe their graph in `build.rs`; the framework generates the wiring.

---

## Workspace Structure

```
limen/
├── limen-core        # Core contracts: traits, types, edges, nodes, graph API, policy
├── limen-nodes       # Node implementations (CSV, I2C IMU, UART, model nodes)
├── limen-platform    # Platform backends (Raspberry Pi/Linux, Cortex-M4)
├── limen-runtime     # Runtime implementations (no_std/no_alloc, std/threaded)
├── limen-codegen     # Graph builder and code generator (build.rs entry point)
├── limen-build       # Proc-macro entry point for codegen
└── limen-examples    # Integration tests and cross-platform examples
```

### Feature Flags

| Flag | Enables |
|---|---|
| *(none)* | `no_std`, `no_alloc` — bare-metal MCU targets |
| `alloc` | Heap-backed edge implementations, `Vec`-based batch paths |
| `std` | Full standard library, concurrent queues, threaded runtimes, I/O sinks |

The same graph definition compiles under all three configurations.

---

## How It Works

### 1. Define your graph in `build.rs`

```rust
GraphBuilder::new("MyPipeline", GraphVisibility::Public)
    .node(
        Node::new(0)
            .ty::<ImuSourceNode<I2cBackend>>()
            .in_ports(0).out_ports(1)
            .in_payload::<()>().out_payload::<ImuFrame>()
            .name(Some("imu_source"))
            .ingress_policy(EdgePolicy::new(
                QueueCaps::new(8, 6, None, None),
                AdmissionPolicy::DropOldest,
                OverBudgetAction::Drop,
            )),
    )
    .node(
        Node::new(1)
            .ty::<InferenceModel<TflmBackend, ImuFrame, ActivityLabel, 32>>()
            .in_ports(1).out_ports(1)
            .in_payload::<ImuFrame>().out_payload::<ActivityLabel>()
            .name(Some("classifier")),
    )
    .node(
        Node::new(2)
            .ty::<UartSinkNode<UartBackend>>()
            .in_ports(1).out_ports(0)
            .in_payload::<ActivityLabel>().out_payload::<()>()
            .name(Some("uart_out")),
    )
    .edge(
        Edge::new(0)
            .ty::<SpscArrayQueue<Message<ImuFrame>, 8>>()
            .payload::<ImuFrame>()
            .from(0, 0).to(1, 0)
            .policy(EdgePolicy::new(
                QueueCaps::new(8, 6, None, None),
                AdmissionPolicy::DropOldest,
                OverBudgetAction::Drop,
            )),
    )
    .edge(
        Edge::new(1)
            .ty::<SpscArrayQueue<Message<ActivityLabel>, 4>>()
            .payload::<ActivityLabel>()
            .from(1, 0).to(2, 0)
            .policy(EdgePolicy::new(
                QueueCaps::new(4, 3, None, None),
                AdmissionPolicy::DropNewest,
                OverBudgetAction::Drop,
            )),
    )
    .finish()
    .write("my_pipeline")
    .unwrap();
```

### 2. Run it — on any target

```rust
// Include the generated graph struct
include!(concat!(env!("OUT_DIR"), "/generated/my_pipeline.rs"));

// Construct nodes, edges, and queues
let graph = MyPipeline::new(imu_source, classifier, uart_out, q0, q1);

// Pick a runtime for your target
let mut runtime = NoAllocRuntime::new();
runtime.init(&mut graph, clock, telemetry).unwrap();

// Run
runtime.run(&mut graph).unwrap();
```

The same graph definition — same nodes, same edges, same policies — runs under:
- `NoAllocRuntime` on a Cortex-M4 (no heap, no threads, deterministic scheduling)
- `ThreadedRuntime` on Linux (one worker thread per node, concurrent execution)

No application code changes. Only the runtime and platform implementations differ.

---

## Core Concepts

### Nodes

Nodes are the units of computation. Limen provides three roles:

- **Source** — 0 inputs, ≥1 outputs. Sensors, file readers, protocol ingress.
- **Process / Model** — ≥1 inputs, ≥1 outputs. Transforms, filters, inference.
- **Sink** — ≥1 inputs, 0 outputs. UART, CSV, GPIO, network output.

Implement the `Node<IN, OUT, InP, OutP>` trait (or the thinner `Source` / `Sink` traits with their adapter wrappers) and your node works in any graph, on any runtime.

### Edges

Edges are typed, policy-enforced queues connecting node ports. Multiple implementations are provided:

| Implementation | no_std | alloc | std | Notes |
|---|---|---|---|---|
| `SpscArrayQueue` | ✓ | ✓ | ✓ | Fixed-capacity, stack-allocated |
| `SpscVecDeque` | ✗ | ✓ | ✓ | Heap-backed, flexible capacity |
| `SpscRingBuf` | ✗ | ✗ | ✓ | `std`-backed ring buffer |
| `ConcurrentQueue` | ✗ | ✓ | ✓ | `Arc<Mutex<Q>>` wrapper for any queue |

Any type implementing the `Edge` trait can be used in a graph. External implementors can validate their implementations against the provided conformance test suite.

### Messages

Every value flowing through the graph is wrapped in a `Message<P>`:

```rust
pub struct MessageHeader {
    trace_id:           TraceId,        // Correlation across nodes
    sequence:           SequenceNumber, // Producer-assigned ordering
    creation_tick:      Ticks,          // Monotonic creation timestamp
    deadline_ns:        Option<DeadlineNs>, // Optional absolute deadline
    qos:                QoSClass,       // BestEffort / Critical / Safety
    payload_size_bytes: usize,          // For byte-cap admission
    flags:              MessageFlags,   // Batch boundaries, degrade hints
    memory_class:       MemoryClass,    // Host / Device placement
}
```

Headers are used by the edge admission system, scheduler, and telemetry layer automatically. Node implementations work with the payload; the framework handles the metadata.

### Policies

Policies are the contract mechanism. They are defined at graph construction time and enforced at runtime:

**EdgePolicy** controls admission to a queue:
- Capacity limits (item count and byte budget, soft and hard watermarks)
- Admission strategy: `DropNewest`, `DropOldest`, or `Reject`
- Over-budget behaviour

**NodePolicy** controls execution semantics:
- Batching: fixed-N, max time span (Δt), sliding or disjoint windows
- Budget: per-step tick budget, watchdog timeout
- Deadline: default deadline, slack tolerance

**Robotics primitives** (in progress, tracked in B0):
- Message freshness and expiry (R2)
- Input liveness — minimum message rate on input edges (R5)
- Node liveness — minimum output rate guarantee (R6)
- Criticality classes — safety-critical vs best-effort separation (R8)
- Urgency — priority ordering under contention (R1)

### Inference

Inference is built in through a backend abstraction:

```rust
pub trait ComputeBackend<InP: Payload, OutP: Payload> {
    type Model: ComputeModel<InP, OutP>;
    type ModelDescriptor<'a>;
    type Error;

    fn capabilities(&self) -> BackendCapabilities;
    fn load_model<'a>(&self, desc: Self::ModelDescriptor<'a>) -> Result<Self::Model, Self::Error>;
}

pub trait ComputeModel<InP: Payload, OutP: Payload> {
    type Error;
    fn infer_one(&mut self, input: &InP, output: &mut OutP) -> Result<(), Self::Error>;
    fn infer_batch<'b>(&mut self, batch: Batch<'b, InP>, outputs: &mut [OutP]) -> Result<(), Self::Error>;
}
```

Implement `ComputeBackend` for your inference engine. The `InferenceModel` node handles batching (stack-allocated in no_std, `Vec`-backed with alloc), telemetry, and lifecycle automatically.

Planned backends:
- **TFLite Micro** — no_std, static tensor arena, Cortex-M4 primary target
- **Tract** — pure Rust, std, desktop/server

### Telemetry

Telemetry is a core contract, not an optional add-on. The `Telemetry` trait is a compile-time feature: when `METRICS_ENABLED` or `EVENTS_STATICALLY_ENABLED` are false, all telemetry calls compile away completely.

When enabled, the framework automatically emits:
- Per-node latency histograms
- Deadline miss counters
- Queue depth gauges
- Ingress/egress message counters
- Structured `NodeStep` events with timing, deadline, and error information
- Runtime lifecycle events (start, stop, reset)

On MCU targets, telemetry drains via UART or a fixed ring buffer. On Linux/desktop, it drains via stdout or a file writer.

---

## The Codegen

Developers never manually construct a `StepContext` or wire queue references between nodes. The codegen handles all of this.

Given a graph definition, `limen-codegen` emits:

- A concrete `struct MyGraph` containing typed node and edge tuples
- A full `impl GraphApi<NODES, EDGES>` with descriptors, occupancy queries, and indexed node stepping
- `impl GraphNodeAccess<I>` and `impl GraphEdgeAccess<E>` for compile-time indexed access
- `impl GraphNodeContextBuilder<I, IN, OUT>` for borrow-safe `StepContext` construction
- *(std only)* `struct MyGraphStd` with `Arc<Mutex<_>>` edge endpoints for concurrent runtimes
- *(std only)* `enum MyGraphStdOwnedBundle` for safe node handoff to worker threads

The result is a fully type-safe graph with no runtime overhead from the wiring layer.

---

## Runtimes

The `LimenRuntime` trait is the scheduling interface:

```rust
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    type Clock;
    type Telemetry;
    type Error;

    fn init(&mut self, graph: &mut Graph, clock: Self::Clock, telemetry: Self::Telemetry) -> Result<(), Self::Error>;
    fn step(&mut self, graph: &mut Graph) -> Result<bool, Self::Error>;
    fn run(&mut self, graph: &mut Graph) -> Result<(), Self::Error> { ... } // default impl
    fn request_stop(&mut self);
    fn is_stopping(&self) -> bool;
    fn occupancies(&self) -> &[EdgeOccupancy; EDGE_COUNT];
    fn reset(&mut self, graph: &Graph) -> Result<(), Self::Error>;
}
```

Any scheduler that implements this trait can run any graph. Planned runtime implementations:

| Runtime | Target | Scheduling | Allocation |
|---|---|---|---|
| `NoAllocRuntime` | MCU / Linux | Round-robin, single-threaded | None |
| `ThreadedRuntime` | Linux / Server | Per-node worker threads | `std` |

---

## Platform Abstraction

Hardware specifics are hidden behind traits defined in `limen-core::platform` and implemented in `limen-platform`:

- `PlatformClock` — monotonic tick source, tick-to-nanosecond conversion
- I2C capability trait — for IMU and sensor ingress nodes
- UART capability trait — for telemetry drain and output sink nodes

Planned platform implementations:

| Platform | I2C | UART | Clock |
|---|---|---|---|
| Linux / Raspberry Pi | ✓ | ✓ | `CLOCK_MONOTONIC` |
| Cortex-M4 | ✓ | ✓ | SysTick / DWT |
| Cortex-M0 | Compile-only | Compile-only | — |

---

## Status

Limen has been in active development for six months. The following is complete:

- [x] `limen-core` contract layer: edges, nodes, messages, policies, graph API, telemetry traits
- [x] Multiple SPSC queue implementations with conformance test suite
- [x] Source, Sink, and InferenceModel node adapters
- [x] `limen-codegen` graph builder and code generator
- [x] No_std and std graph variants generated from the same definition
- [x] `TestNoStdRuntime` (round-robin, no heap) — integration tested
- [x] `TestStdRuntime` (per-node threads, owned bundle handoff) — integration tested
- [x] `GraphTelemetry` with fixed-buffer and I/O writer sinks
- [x] Linux monotonic clock implementation
- [x] End-to-end integration tests: source → model → sink on both runtimes

In progress (tracked issues):

- [ ] B0: Robotics primitives — freshness, liveness, criticality, urgency, mailbox semantics
- [ ] C2: N→M node arity and optional input ports
- [ ] P1: PlatformBackend finalisation
- [ ] Production `NoAllocRuntime` with full policy enforcement
- [ ] TFLite Micro backend (Cortex-M4)
- [ ] Tract backend (desktop/server)
- [ ] Raspberry Pi and Cortex-M4 platform implementations
- [ ] Cross-platform IMU activity recognition example
- [ ] External conformance test suite documentation
- [ ] Handbook

---

## Roadmap to v0.1.0

| Phase | Work | Status |
|---|---|---|
| 0 | Feature hygiene, CI matrix, API guardrails | In progress |
| 1 | Tensor/batch semantics, N→M ports, header time types | In progress |
| 2 | Trait surface stabilisation, metadata completeness | Planned |
| 3 | Robotics primitives (B0: freshness, liveness, criticality, urgency) | Planned |
| 4 | Runtime lifecycle semantics, baseline error propagation | Planned |
| 5 | Platform/backend finalisation | Planned |
| 6 | Tests overhaul, docs audit, release hygiene | Planned |

v0.1.0 is the first version where the public API is stable enough for downstream crates to build against without repeated breaking changes.

---

## Design Philosophy

**Core enables; core does not embed.** `limen-core` defines contracts and primitives. It does not contain a scheduler, a specific queue implementation chosen for you, or any platform code. Runtimes, platforms, and backends are downstream of core.

**Monomorphization over dynamic dispatch.** Every node, edge, queue, clock, and telemetry sink is a concrete generic type in the hot path. This is a deliberate choice for MCU targets where vtable dispatch and heap allocation are unacceptable.

**Feature flags as a first-class concern.** The same source file compiles correctly under no_std, alloc, and std. This is not an afterthought — it is enforced by the CI matrix.

**Contracts are enforced, not just observed.** Freshness, liveness, and criticality are not metrics emitted after the fact. They are policies checked at the scheduling boundary. A message that has expired should not reach a node.

**Codegen removes the abstraction tax.** The ergonomic cost of a strongly-typed, zero-dynamic-dispatch system is usually a wall of type parameters. Codegen pays that cost once, at build time, so application developers never see it.

---

## Contributing

Limen is currently in pre-release development. The repository will be made public alongside the v0.1.0 release.

If you are interested in early access, collaboration, or have a use case you would like to discuss, please open an issue or reach out directly.

---

## Licence

Licensed under either of:

- Apache License, Version 2.0
- MIT License

at your option.

---

*Limen — the threshold between the graph you define and the hardware it runs on.*
