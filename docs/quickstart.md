# Quickstart Guide

This guide walks through the **pipeline demo** — a runnable example that
demonstrates Limen's core capabilities in under a minute.

---

## Prerequisites

- **Rust stable toolchain** (1.81+). Install via [rustup](https://rustup.rs/).
- No external dependencies — everything builds from source.

```bash
git clone https://github.com/umbriel-sys/limen.git
cd limen
```

---

## Running the Demo

```bash
cargo run -p limen-examples --features std --example pipeline_demo
```

The `std` feature enables concurrent queues and threaded runtimes. Without
it, only the single-threaded `no_std` path is available.

---

## What the Demo Does

The demo builds a three-node **Sensor → Model → Actuator** inference
pipeline — the canonical pattern for edge AI and robotics systems:

```
[Sensor] --edge 0--> [Model] --edge 1--> [Actuator]
```

| Node | Role | Real-world analogue |
|---|---|---|
| **Sensor** | Produces tensor data from an upstream backlog | IMU, camera, LiDAR |
| **Model** | Identity inference pass-through (batched) | TFLite Micro, Tract |
| **Actuator** | Consumes results | Motor controller, UART, MQTT |

**Configuration:**
- **Edges:** SPSC (single-producer single-consumer) queues, capacity 16
- **Backpressure:** DropOldest — when the queue is full, the oldest item is
  evicted. This is the correct policy for real-time systems where stale
  sensor data should be discarded in favour of fresh readings.
- **Batching:** Fixed size 4 on sensor and model nodes — items are grouped
  into batches of 4 before processing.
- **Memory:** Static (no-alloc) managers with 20 slots per edge — no heap
  allocation required.

The same graph definition, node logic, and policies work identically on
bare-metal `no_std` microcontrollers and multi-threaded Linux servers.

---

## Expected Output Walkthrough

### Header

```
====================================================================
  Limen Pipeline Demo -- Edge Inference Runtime
====================================================================

  Graph topology:

    [Sensor] --edge 0--> [Model] --edge 1--> [Actuator]

  Edges:    SPSC queues, capacity 16, DropOldest backpressure
  Batching: fixed size 4 on sensor and model nodes
  Memory:   static (no-alloc) managers, 20 slots per edge
```

### Part 1: Step-by-step execution

This section runs the pipeline one step at a time using a single-threaded
`no_std`-compatible runtime. The source is pre-loaded with 20 items (more
than the edge capacity of 16), so backpressure will engage.

```
    Step |    sensor->model |      model->actuator
  -------+------------------+---------------------
       1 |          4 items |              0 items
       2 |          0 items |              4 items
       3 |          0 items |              3 items
       4 |          4 items |              3 items
       5 |          0 items |              7 items
       ...
      15 |          0 items |             15 items

  Pipeline complete:
    Messages consumed by actuator: 5
    Backpressure policy: DropOldest (stale data evicted)
```

**What to notice:**

- **Batching in action:** The sensor pushes 4 items per batch (visible in
  steps 1, 4, 7, 10, 13 where `sensor→model` jumps by 4).
- **Pipeline flow:** The model consumes from edge 0 and produces to edge 1.
  The actuator consumes from edge 1 one item at a time.
- **Round-robin scheduling:** The single-threaded runtime visits each node
  in turn. One step = one pass through all three nodes.
- **Accumulation:** Items accumulate in `model→actuator` because the
  actuator consumes one item per step while the model produces batches.
- **Backpressure:** The source generates more items than can fit (the
  source also adds 1–2 random items to its backlog each step to simulate
  continuous sensor input). The DropOldest policy ensures that stale data
  is evicted rather than blocking the pipeline.

### Part 2: Concurrent execution

This section rebuilds the graph with thread-safe concurrent edges and runs
it with one worker thread per node for 500ms.

```
  Launching 3 worker threads (one per node)...
  Running for 500ms with continuous sensor input.

  Concurrent pipeline stopped gracefully via RuntimeStopHandle.
  Messages processed by actuator: ~400
```

**What to notice:**

- **Thread-per-node:** Each node runs on its own OS thread with a backoff
  scheduler (sleeps briefly when idle or backpressured).
- **Graceful shutdown:** A `RuntimeStopHandle` is cloned before `run()` and
  passed to a timer thread that requests cooperative stop after 500ms. All
  threads drain cleanly.
- **Throughput:** The concurrent pipeline processes hundreds of messages in
  500ms despite simulated processing delays in each node.

### Telemetry Summary

The final section prints per-node and per-edge metrics collected during the
concurrent run:

```
graph id: 0
  node id: 0 processed=~800 dropped=~360 ingress=0 egress=~800 ...
  node id: 1 processed=~400 dropped=0 ingress=~400 egress=~400 ...
  node id: 2 processed=~400 dropped=0 ingress=~400 egress=0 ...
  edge id: 0 queue_depth=0
  edge id: 1 queue_depth=16
  edge id: 2 queue_depth=2
```

**What to notice:**

- **Sensor (node 0):** `dropped=~360` — nearly half the produced messages
  were dropped by the DropOldest admission policy. This is correct
  behaviour: the model processes batches slower than the sensor produces,
  so stale readings are evicted.
- **Model (node 1):** `dropped=0` — the model never drops; it processes
  everything it receives. `processed` is roughly half of the sensor's
  output because it processes in batches of 4.
- **Actuator (node 2):** Consumes everything the model produces.
- **Edge depths:** Edge 1 (`sensor→model`) is drained (0). Edge 2
  (`model→actuator`) is at capacity (16) — the actuator is the bottleneck.
- **Latency metrics:** `lat_sum`, `lat_cnt`, `lat_max` track per-node
  processing time in nanoseconds.

> Exact numbers vary between runs due to thread scheduling and simulated
> processing delays.

---

## Next Steps

| Topic | Resource |
|---|---|
| Define your own graph | See `limen-examples/examples/pipeline_demo.rs` for the `define_graph!` DSL |
| Architecture deep dive | [Architecture Guide](architecture/index.md) |
| API reference | `cargo doc --workspace --open` or [GitHub Pages](https://umbriel-sys.github.io/limen/) |
| Design decisions | [ADRs](ADRs/) |
| Full roadmap | [Roadmap](roadmap.md) |

### Running the Integration Tests

The full test suite exercises all graph generation methods (proc-macro,
build-script codegen, hand-written) across all feature profiles:

```bash
# no_std (default) — single-threaded, static queues
cargo test -p limen-examples

# std — concurrent queues, threaded runtimes
cargo test -p limen-examples --features std

# All features including unsafe lock-free ring buffer
cargo test -p limen-examples --features spsc_raw
```
