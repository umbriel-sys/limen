# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

> **Note:** Limen is in alpha. APIs may change before v0.1.0.

## [0.1.0-alpha.1] — 2026-04-08

### Added

- **limen-core:** Core contract layer — traits and types for edges, nodes,
  messages, policies, graph API, telemetry, scheduling, memory management,
  platform clocks, and runtime lifecycle.
- **limen-core:** Multiple SPSC queue implementations (`SpscArrayQueue`,
  `SpscVecDeque`, `SpscRingbufQueue`, `SpscConcurrentQueue`, `SpscPriority2`)
  with shared conformance test suite.
- **limen-core:** Token-based zero-copy memory model with three manager
  implementations (`StaticMemoryManager`, `HeapMemoryManager`,
  `ConcurrentMemoryManager`).
- **limen-core:** `Tensor` and `Batch` message payload types with shape,
  stride, and quantization support.
- **limen-core:** `GraphTelemetry` with fixed-buffer and I/O writer sinks.
- **limen-core:** `NoStdLinuxMonotonicClock` platform clock implementation.
- **limen-core:** Source, Sink, and InferenceModel node adapter traits.
- **limen-codegen:** Graph builder API and DSL parser for declarative graph
  definitions.
- **limen-codegen:** Full graph validation (contiguous indices, port counts,
  payload type matching, queue type consistency).
- **limen-codegen:** Code generator emitting fully-typed `no_std` graph structs
  with optional `std`-only concurrent graph module.
- **limen-build:** `define_graph!` proc-macro wrapper for `limen-codegen`.
- **limen-runtime:** Skeleton runtime and scheduler module structure.
- **limen-platform:** Skeleton Linux platform adapter module.
- **limen-examples:** Integration tests exercising hand-written, codegen, and
  proc-macro graph definitions across `no_std` and `std` feature variants.
