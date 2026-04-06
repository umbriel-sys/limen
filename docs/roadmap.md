# Limen Roadmap

## Vision

Limen aims to be the standard framework for portable, contract-enforcing
computation graphs in embedded AI systems. A single pipeline definition should
run — unchanged — from a bare-metal Cortex-M4 to a multi-threaded Linux
server, with inference, scheduling, and safety contracts handled by the
framework.

---

## Roadmap to v0.1.0

v0.1.0 is the first release where the public API is stable enough for
downstream crates to build against without repeated breaking changes.

### Phase Overview

| Phase | Focus | Key Issues | Status |
|---|---|---|---|
| **0** | Guardrails | Feature hygiene, CI matrix, `#[non_exhaustive]` | Done |
| **1** | Currency + Arity | Tensor-first payloads, N-to-M ports, time types | In progress |
| **2** | Trait Surface | Contract stabilisation, metadata completeness | Planned |
| **3** | Robotics Primitives | Freshness, liveness, criticality, urgency, mailbox | Planned |
| **4** | Runtime Semantics | Lifecycle contract, error propagation | Planned |
| **5** | Platform Finalisation | `PlatformBackend` stabilisation | Planned |
| **6** | Quality + Release | Tests overhaul, docs audit, MSRV, CI gates | Planned |

---

### Phase 0: Guardrails (Done)

Established the foundation for stable API evolution:

- Feature flag hygiene (`no_std` default enforced in CI).
- `#[non_exhaustive]` on all public enums and structs.
- Constructors and builders instead of public fields.
- CI matrix: `no_std`, `alloc`, `std` builds all pass.

### Phase 1: Currency + Arity (In Progress)

Finalise what flows through the graph and how ports are addressed:

- **C1 — Tensor-first currency.** Tensor and batch payloads as the primary
  currency for ML workloads. Scalar shortcuts removed. **Complete.**
- **C2 — N-to-M node arity.** Port model with indexed inputs/outputs,
  optional input semantics, fan-in/fan-out support.
- **R0a — MessageHeader baseline.** Lock `Timestamp` and `Duration` types,
  define `received_at` / `produced_at` semantics, set sequence ID scope.

### Phase 2: Trait Surface Stabilisation

Freeze the public trait signatures based on Phase 1 outcomes:

- **C3 — Contract stabilisation.** Finalise `Node`, `Edge`, `MemoryManager`,
  `GraphApi`, and `LimenRuntime` signatures. Sync-first core; async adapters
  above.
- **C4 — Metadata completeness.** Port definitions, batch constraints, shape
  metadata, quantisation metadata. All alloc-free by default.

### Phase 3: Robotics Primitives

Add first-class contracts for robotics and control system graphs:

- **R1** — Urgency in `MessageHeader` (priority ordering).
- **R2** — Freshness / expiry semantics (`max_age`, `valid_until`).
- **R4** — Mailbox semantics (`peek_latest`, `read_latest`, overwrite-on-full).
- **R5** — Per-input liveness policy (detect stuck sensors).
- **R6** — Node liveness policy (detect stalled nodes).
- **R7** — Edge peek APIs for scheduler decisions.
- **R8** — Criticality classes (safety-critical vs best-effort).
- **R9** — Standard telemetry event identifiers for robotics.

Optional (may defer to v0.2.0):
- R3 (clock domain identification), R10 (stale-skip convenience),
  R11 (execution measurement).

### Phase 4: Runtime Semantics

Define the execution lifecycle and error propagation contracts:

- **RS1 — Runtime lifecycle.** Five phases: `init` → `start` → `running` →
  `stop` → `reset`/`shutdown`. Soft vs hard reset semantics.
- **G1 — Error propagation.** Safety-critical failures halt or emit failsafe
  signal. Best-effort failures drop and emit telemetry events.

### Phase 5: Platform Finalisation

Stabilise the platform abstraction:

- **P1 — PlatformBackend.** Service provider (time, event hooks, capabilities),
  not policy engine. Reference implementation for Linux. Cortex-M4 stub.

### Phase 6: Quality + Release

Final gates before tagging v0.1.0:

- **Q1 — Tests overhaul.** Tensor-first, N-to-M graphs, mailbox, freshness,
  liveness, urgency ordering, deterministic time injection.
- **Q2 — Docs audit.** All public API documented. Feature flag requirements
  noted on every item. API freeze rules published.
- **Q3 — Release hygiene.** MSRV pinned. CI gates: fmt, clippy, test, doc.
  Feature matrix. Changelog. Examples compile under all features.

---

## v0.1.0 Definition of Done

All of the following must be satisfied:

1. All leaf issues closed or explicitly deferred with documented rationale.
2. Feature matrix builds clean in CI: `no_std`, `alloc`, `std`.
3. Comprehensive test coverage for tensor payloads, N-to-M graphs, mailbox
   semantics, staleness detection, liveness violations, and telemetry events.
4. Public API documented and frozen.
5. At least one platform crate satisfies the final `PlatformBackend` contract.

---

## Beyond v0.1.0

### v0.2.0 — Inference Backends and Platform Implementations

- **TFLite Micro backend** — `no_std`, static tensor arena, Cortex-M4 target.
- **Tract backend** — pure Rust, `std`, desktop and server targets.
- **Raspberry Pi platform** — full `PlatformBackend` implementation with
  GPIO, SPI, and I2C support.
- **Cortex-M4 platform** — SysTick/DWT clock, UART telemetry drain.

### v1.0 — Production Readiness

- Production `NoAllocRuntime` with full policy enforcement.
- `ThreadedRuntime` with EDF and throughput scheduling.
- Cross-platform IMU activity recognition reference example.
- External conformance test suite documentation for third-party
  implementations.
- Stability guarantees: semver compliance, deprecation policy.

### Post v0.1.0 (Stretch Goals)

- **Zero-lock concurrent graphs ([ADR-013](ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md))** —
  lock-free edge and memory manager enabling a single graph type across all
  execution models (no_std single-threaded, std multi-threaded, and no_std
  multi-core).
- Async node adapter for I/O-bound workloads.
- Device memory transfer orchestration (Host ↔ GPU/NPU).
- Dynamic graph reconfiguration (add/remove nodes at runtime).
- Visual graph editor / debugger.
- WASM target support.
