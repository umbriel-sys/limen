# Limen

[![CI](https://github.com/umbriel-sys/limen/actions/workflows/ci.yml/badge.svg)](https://github.com/umbriel-sys/limen/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/limen-core.svg)](https://crates.io/crates/limen-core)
[![docs.rs](https://docs.rs/limen-core/badge.svg)](https://docs.rs/limen-core)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/MSRV-1.81-blue.svg)](#)

> **Portable, contract-enforcing computation graphs for AI-enabled embedded
> systems — from bare-metal microcontrollers to multi-threaded servers, in
> safe Rust.**

> **Alpha release.** Core contracts, code generator, and integration-tested
> runtimes are functional. APIs may change before v0.1.0.

---

## The Problem

Building AI-enabled robotics and edge computing systems today means
rebuilding the same software pipeline for every hardware target. A
perception-inference-actuation pipeline written for a Cortex-M4 cannot run
on a Raspberry Pi or a server without substantial rework — despite
implementing identical logic.

No existing framework bridges this gap:

| Framework | Portable Graphs | Inference Built-in | `no_std` | Contract Enforcement |
|---|---|---|---|---|
| ROS 2 | Yes | No | No | No |
| Embassy | No | No | Yes | No |
| RTIC | No | No | Yes | No |
| Burn / Tract | No | Yes | Partial | No |
| TFLite Micro | No | Yes | Yes | No |
| **Limen** | **Yes** | **Yes** | **Yes** | **Yes** |

Limen closes this gap.

---

## What Limen Delivers

- **Single graph definition, any target.** Node logic and policies are
  identical across bare-metal MCU and multi-threaded server targets. Edge
  and memory manager types must currently be switched between `no_std` and
  `std` builds; a planned zero-lock edge and memory manager
  ([ADR-013](docs/ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md)) will
  close this gap, enabling a single graph definition with zero code changes.
- **Zero dynamic dispatch in the hot path.** All nodes and edges are
  monomorphized at compile time. No vtables, no heap required.
- **Contract enforcement, not just observability.** Freshness, liveness,
  criticality, and backpressure are runtime-enforced policies — not logging
  conveniences.
- **Inference as a first-class citizen.** A `ComputeBackend` trait abstracts
  TFLite Micro on MCU and Tract on desktop behind the same model node.
- **Safe Rust throughout.** No `unsafe` in the hot path. Memory safety
  without a garbage collector.
- **Codegen removes all boilerplate.** Describe your graph in `build.rs` or
  a proc-macro; the framework generates fully type-safe wiring at build time.

---

## Quickstart

**Prerequisites:** Rust stable toolchain (1.81+). No external dependencies.

```bash
# Clone and enter the repository
git clone https://github.com/umbriel-sys/limen.git
cd limen

# Run the pipeline demo (requires std feature for concurrent execution)
cargo run -p limen-examples --features std --example pipeline_demo
```

The demo builds a **Sensor → Model → Actuator** pipeline, runs it
step-by-step showing backpressure and batching, then runs the same graph
concurrently with one thread per node. See the
[Quickstart Guide](docs/quickstart.md) for a full walkthrough of the output
and what each section demonstrates.

---

## Example: Define a Graph

A three-node pipeline (sensor → inference → output) defined in a build
script:

```rust
// build.rs
use limen_codegen::builder::*;

GraphBuilder::new("MyPipeline", GraphVisibility::Public)
    .node(Node::new(0)
        .ty::<ImuSourceNode<I2cBackend>>()
        .in_ports(0).out_ports(1)
        .in_payload::<()>().out_payload::<ImuFrame>()
        .name(Some("imu_source")))
    .node(Node::new(1)
        .ty::<InferenceModel<TflmBackend, ImuFrame, ActivityLabel, 32>>()
        .in_ports(1).out_ports(1)
        .in_payload::<ImuFrame>().out_payload::<ActivityLabel>()
        .name(Some("classifier")))
    .node(Node::new(2)
        .ty::<UartSinkNode<UartBackend>>()
        .in_ports(1).out_ports(0)
        .in_payload::<ActivityLabel>().out_payload::<()>()
        .name(Some("uart_out")))
    .edge(Edge::new(0)
        .ty::<StaticRing<MessageToken, 8>>()
        .payload::<ImuFrame>()
        .manager_ty::<StaticMemoryManager<ImuFrame, 8>>()
        .from(0, 0).to(1, 0)
        .policy(EdgePolicy::default()))
    .edge(Edge::new(1)
        .ty::<StaticRing<MessageToken, 4>>()
        .payload::<ActivityLabel>()
        .manager_ty::<StaticMemoryManager<ActivityLabel, 4>>()
        .from(1, 0).to(2, 0)
        .policy(EdgePolicy::default()))
    .finish()
    .write("my_pipeline")
    .unwrap();
```

Then run it on any target:

```rust
include!(concat!(env!("OUT_DIR"), "/generated/my_pipeline.rs"));

let graph = MyPipeline::new(imu_source, classifier, uart_out, q0, q1);
let mut runtime = NoAllocRuntime::new();
runtime.init(&mut graph, clock, telemetry).unwrap();
runtime.run(&mut graph).unwrap();
```

Same node logic. Same policies. `NoAllocRuntime` on a Cortex-M4.
`ThreadedRuntime` on Linux. Only edge and memory manager types differ
between targets today — ADR-013 will unify these.

---

## Architecture

Limen is organised as a layered workspace. The core contract crate owns no
implementations — runtimes, platforms, and backends are all downstream:

| Crate | Role |
|---|---|
| `limen-core` | Contracts: traits, types, edges, nodes, graph API, policies, telemetry |
| `limen-node` | Concrete node implementations (sensors, sinks, model adapters) |
| `limen-runtime` | Graph executors and schedulers |
| `limen-platform` | Platform adapters (clocks, I/O) |
| `limen-codegen` | Graph builder and Rust code generator |
| `limen-build` | Proc-macro wrapper for `limen-codegen` |
| `limen-examples` | Integration tests and cross-platform examples |

### Feature Flags

| Flag | Enables |
|---|---|
| *(default)* | `no_std`, no heap — bare-metal MCU targets |
| `alloc` | Heap-backed queues, `Vec`-based batch paths |
| `std` | Implies `alloc`. Concurrent queues, threaded runtimes, I/O sinks |

See the [Architecture Guide](docs/architecture/index.md) for a deep dive
into the memory model, node contracts, edge semantics, and graph execution
flow.

---

## Status

Limen has been in active development for seven months.

**Complete:**
- Core contract layer — edges, nodes, messages, policies, graph API,
  telemetry
- Multiple SPSC queue implementations with conformance test suite
- Source, Sink, and InferenceModel node adapters
- Code generator (build-script and proc-macro) with full graph validation
- `no_std` and `std` graph variants generated from the same definition
- Token-based zero-copy memory model with three manager implementations
- Integration-tested runtimes: single-threaded (`no_std`) and scoped-thread
  (`std`)
- `GraphTelemetry` with fixed-buffer and I/O writer sinks
- Linux monotonic clock implementation

**In progress:**
- Robotics primitives — freshness, liveness, criticality, urgency, mailbox
  semantics
- N-to-M node arity and optional input ports
- Platform backend finalisation
- Production `NoAllocRuntime` with full policy enforcement

**Planned:**
- TFLite Micro backend (Cortex-M4)
- Tract backend (desktop/server)
- Raspberry Pi and Cortex-M4 platform implementations
- Cross-platform IMU activity recognition example

See the full [Roadmap](docs/roadmap.md) for phased delivery to v0.1.0 and
beyond.

---

## Documentation

| Document | Description |
|---|---|
| [API Reference](https://umbriel-sys.github.io/limen/) | Rustdoc API documentation (GitHub Pages) |
| [Quickstart Guide](docs/quickstart.md) | Pipeline demo walkthrough and expected output |
| [Architecture Guide](docs/architecture/index.md) | System design, memory model, execution flow |
| [Decision Records](docs/ADRs/) | Rationale behind key design decisions |
| [Roadmap](docs/roadmap.md) | Phased plan to v0.1.0 and stretch goals |
| [Funding & Licensing](FUNDING.md) | Funded work commitments and how to support the project |
| [Development Guide](docs/dev_guide.md) | Building, testing, and contributing |

---

## Licensing and IP

Limen is released under the Apache License, Version 2.0
([LICENSE-APACHE](LICENSE-APACHE)).

The project is developed and owned by its original author. Contributions
are accepted under a Contributor Licence Agreement ([CLA](CLA.md)), which
ensures the project can be maintained, evolved, and (if necessary)
dual-licensed for commercial use.

All deliverables produced under accepted public grants will be released
under Apache-2.0 and remain available under Apache-2.0. Future major
versions may be distributed under different or additional licence terms.
See [FUNDING.md](FUNDING.md) for details.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and the contributor
licence agreement.

---

*Limen — the threshold between the graph you define and the hardware it
runs on.*

Copyright &copy; 2025–present Arlo Louis Byrne (idky137)
