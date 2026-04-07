# Development and Testing Guide

> **Note:** Limen is in active pre-release development. Contracts and APIs
> may change without notice before v0.1.0.

---

## Prerequisites

- Rust toolchain (stable). See `rust-toolchain.toml` if present.
- Cargo (included with Rust).
- No external dependencies required for `no_std` builds.

---

## Quickstart: Run an Example Pipeline

```bash
# no_std single-threaded runtime (default features)
cargo test -p limen-examples -- codegen_core_pipeline_runs_with_nostd_runtime --nocapture

# std multi-threaded runtime (concurrent queues, scoped threads)
cargo test -p limen-examples --features std -- codegen_std_pipeline_runs_with_std_runtime --nocapture
```

Both tests build a three-node **Source → Model → Sink** pipeline from a
codegen-generated graph, wire it to a runtime, and step it through several
iterations. `--nocapture` prints edge occupancies and telemetry to stdout.

---

## Building

```bash
# Default: no_std, no heap
cargo build

# With alloc (heap-backed queues, Vec-based batches)
cargo build --features alloc

# With std (concurrent queues, threaded runtimes, I/O sinks)
cargo build --features std
```

Currently, `std` graphs require concurrent edge and memory manager types
(`ConcurrentEdge`, `ConcurrentMemoryManager`) which are not available in
`no_std` builds. A single graph definition does not yet compile across all
feature flags — [ADR-013](ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md)
addresses this.

---

## Testing

```bash
# Run all tests (default features)
cargo test --workspace

# Run all tests with std features
cargo test --workspace --features std

# Run tests for a specific crate
cargo test -p limen-core
cargo test -p limen-examples

# Run a single test by name
cargo test -p limen-core -- core_pipeline_runs_with_nostd_runtime
```

### Feature Flag Matrix

| Flag | What it enables | Test command |
|---|---|---|
| *(default)* | `no_std` path, static queues, static memory managers | `cargo test --workspace` |
| `alloc` | Heap-backed queues, `Vec` batch paths | `cargo test --workspace --features alloc` |
| `std` | Concurrent queues, scoped-thread runtimes, I/O sinks | `cargo test --workspace --features std` |
| `bench` | Test nodes, queues, and runtime impls for integration tests | Enabled automatically by `limen-examples` |

### Local CI

Before submitting a pull request, run the local CI script to validate the full
feature matrix:

```bash
./dev_utils/run-local-ci.sh
```

This runs fmt, build, test, and clippy across all feature flag combinations
(`no-default-features`, `alloc`, `std`, `spsc_raw`). Options:

| Flag | Effect |
|---|---|
| `--no-clippy-or-fmt` | Skip `cargo fmt` and `cargo clippy` checks |
| `--clean` | Run `cargo clean` after all checks pass |
| `--release` | Run `cargo build --release` after all checks pass |

---

## Linting and Formatting

```bash
# Lint
cargo clippy --workspace

# Lint with all features
cargo clippy --workspace --all-features

# Format
cargo fmt --all

# Check formatting (CI gate)
cargo fmt --all -- --check
```

---

## Code Style

- **Line width:** 100 columns.
- **Field init shorthand:** `use_field_init_shorthand = true`.
- **`no_std` guards:** All heap use behind `#[cfg(feature = "alloc")]`.
  All `std`-only code behind `#[cfg(feature = "std")]`.
- **No `println!` / `eprintln!`:** Use the `Telemetry` trait.
- **No `unsafe`:** Except in `spsc_raw` (gated behind its own feature flag).
- **No `Box<dyn Trait>`** in hot paths.
- **Feature flags are additive:** `std` implies `alloc`. Never write
  `#[cfg(any(feature = "alloc", feature = "std"))]` where
  `#[cfg(feature = "alloc")]` suffices.

---

## Defining a Graph

Three equivalent approaches:

### 1. Proc-Macro (inline)

```rust
use limen_build::define_graph;

define_graph! {
    pub struct MyGraph;
    nodes { ... }
    edges { ... }
}
```

### 2. Build Script (DSL string)

```rust
// build.rs
use limen_codegen::expand_str_to_file;
expand_str_to_file(DSL_STRING, &out_path)?;

// In source:
include!(concat!(env!("OUT_DIR"), "/my_graph.rs"));
```

### 3. Build Script (typed builder)

```rust
// build.rs
use limen_codegen::builder::{GraphBuilder, Node, Edge};

let ast = GraphBuilder::new(vis, name)
    .node(Node::new(0).ty::<MySource>().in_ports(0).out_ports(1)
        .in_payload::<()>().out_payload::<SensorData>())
    .edge(Edge::new(0).ty::<StaticRing<MessageToken, 8>>()
        .payload::<SensorData>().from(0, 0).to(1, 0))
    .finish();

limen_codegen::expand_ast_to_file(ast, &out_path)?;
```

See [Graph Codegen](architecture/codegen.md) for full details.

---

## Adding a New Node

1. **Implement `process_message`.** This is the only required method for most
   nodes:

   ```rust
   impl Node<1, 1, SensorData, ProcessedData> for MyProcessor {
       fn process_message<C: PlatformClock>(
           &mut self, msg: &Message<SensorData>, clock: &C,
       ) -> Result<ProcessResult<ProcessedData>, NodeError> {
           let result = self.transform(msg.payload());
           Ok(ProcessResult::Output(
               Message::new(msg.header().clone(), result)
           ))
       }
       // ... capabilities, lifecycle, policy methods
   }
   ```

2. **For sources:** Implement `Source<OutP, OUT>` and wrap with `SourceNode`.
3. **For sinks:** Implement `Sink<InP, IN>` and wrap with `SinkNode`.
4. **Wire into a graph** using any of the three codegen approaches.
5. Validate with the conformance test suite, `limen-core::node::contract_tests`.

---

## Adding a New Edge Implementation

1. Implement the `Edge` trait.
2. Validate with the conformance test suite:

   ```rust
   run_edge_contract_tests!(MyCustomQueue);
   ```

   The macro tests FIFO ordering, capacity limits, token coherence, occupancy
   accuracy, and admission correctness.

3. For concurrent use, also implement `ScopedEdge`.

---

## Project Structure

```
limen/
├── limen-core/         # Core contracts (traits, types, policies)
├── limen-node/         # Concrete node implementations
├── limen-runtime/      # Executors and schedulers
├── limen-platform/     # Platform adapters
├── limen-codegen/      # Code generator
├── limen-build/        # Proc-macro wrapper
├── limen-examples/     # Integration tests
├── docs/               # This documentation
└── claude_context_docs/ # Internal development context (not public)
```
