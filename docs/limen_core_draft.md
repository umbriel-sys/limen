  # limen-core Architecture

  ## Module Overview

  | Module | Responsibility |
  |---|---|
  | **types** | Leaf newtypes: identifiers, ticks, QoS, MessageToken, DataType/DType, F16/BF16 |
  | **errors** | Error enums for every subsystem (queue, node, memory, graph, runtime, scheduler) |
  | **memory** | Memory placement model and manager contracts for token-based zero-copy |
  | **policy** | Pure-data policy structs: admission, batching, budgets, watermarks, queue caps |
  | **message** | Message envelope (header + payload), Payload trait, BatchView, Tensor |
  | **edge** | SPSC queue contract and implementations (static, alloc, std, priority, concurrent) |
  | **node** | Node contract, step lifecycle, execution context |
  | **compute** | Inference backend abstraction (model load, infer, drain) |
  | **graph** | Compile-time typed topology, validation, and per-node context construction |
  | **platform** | Clock, timer, and affinity abstractions |
  | **scheduling** | Readiness assessment and next-node selection for the runtime loop |
  | **telemetry** | Counters, gauges, latency recording, structured event emission |
  | **runtime** | Top-level executor contract: init, step loop, cooperative stop |
  | **prelude** | Flat re-export of all public API |

  ---

  ## Entity Relationship Diagram

                       ┌──────────────────────────┐
                       │     LimenRuntime         │
                       │                          │
                       │ Owns a Graph, Clock,     │
                       │ Telemetry. Drives the    │
                       │ step loop to completion. │
                       └────┬──────────┬──────────┘
                            │          │
                  steps via │          │ selects next node via
                            │          │
                            ▼          ▼
            ┌───────────────────┐   ┌──────────────┐
            │     GraphApi      │   │ DequeuePolicy│
            │                   │   │ (Scheduler)  │
            │ Typed topology.   │   │              │
            │ Builds StepContext│   │ Picks which  │
            │ per node, drives  │   │ node to step │
            │ step_node_by_idx. │   │ next from    │
            │ Validates graph   │   │ NodeSummary  │
            │ invariants.       │   │ snapshots.   │
            └───┬───────┬───────┘   └──────────────┘
                │       │
     builds     │       │ indexes into
                ▼       ▼
    ┌───────────────────┐   ┌──────────────────────────────────────┐
    │   StepContext     │   │              Node                    │
    │                   │   │                                      │
    │ Per-step execution│   │ Stateful processing unit.            │
    │ environment.      │   │ Receives a StepContext, returns      │
    │ Holds refs to     │   │ StepResult.                          │
    │ input/output      │   │                                      │
    │ edges, policies,  │   │ step()        — single-message loop  │
    │ clock, telemetry. │   │ step_batch()  — batched loop         │
    │                   │   │ process_message() — per-item logic   │
    │ Provides the      │   │                                      │
    │ node's window     │   │ Variants: Source, Sink, Model,       │
    │ into the graph.   │   │ Process, Split, Join, External       │
    └──┬──────┬─────────┘   └───────────┬────────────────────────-─┘
       │      │                         │
       │      │ reads/writes via        │ Model nodes delegate to
       │      │                         ▼
       │      │              ┌──────────────────────┐
       │      │              │    ComputeModel      │
       │      │              │                      │
       │      │              │ Inference backend.   │
       │      │              │ init, infer_one,     │
       │      │              │ infer_batch, drain.  │
       │      │              │ Loaded by a          │
       │      │              │ ComputeBackend.      │
       │      │              └──────────────────────┘
       │      │
       │      ▼
       │   ┌───────────────────────────────────────────────────────┐
       │   │                     Edge                              │
       │   │                                                       │
       │   │ SPSC queue contract. Connects one node's output       │
       │   │ to another node's input.                              │
       │   │                                                       │
       │   │ try_push(token, policy, headers) → EnqueueResult      │
       │   │ try_pop(headers)                 → MessageToken       │
       │   │ try_pop_batch(policy, headers)   → BatchView          │
       │   │ occupancy(policy)                → EdgeOccupancy      │
       │   │                                                       │
       │   │ Queues store MessageToken handles, not payloads.      │
       │   │ Byte accounting via HeaderStore lookups.              │
       │   │                                                       │
       │   │ Impls: StaticRing, VecDeque [A], RingBuf [S],         │
       │   │        SpscRaw [S+unsafe], Priority2, Concurrent [S]  │
       │   └────────────────────────┬──────────────────────────────┘
       │                            │
       │              governed by   │  uses for byte accounting
       │                            ▼
       │   ┌─────────────────┐    ┌───────────────────────────────────┐
       │   │  EdgePolicy     │    │        MemoryManager              │
       │   │                 │    │        (& HeaderStore)            │
       │   │ QueueCaps,      │    │                                   │
       │   │ AdmissionPolicy,│    │ Owns Message storage.             │
       │   │ OverBudgetAction│    │ Issues MessageToken on store().   │
       │   │                 │    │ HeaderStore::peek_header() gives  │
       │   │ Drives admit /  │    │ edges access to header metadata   │
       │   │ reject / evict  │    │ without owning the payload.       │
       │   │ decisions and   │    │                                   │
       │   │ watermark state.│    │ Impls: StaticMemoryManager,       │
       │   └─────────────────┘    │        HeapMemoryManager [A],     │
       │                          │        ConcurrentManager [S]      │
       │                          └──────────────┬────────────────────┘
       │                                         │
       │                            stores / retrieves
       │                                         ▼
       │                          ┌───────────────────────────────┐
       │                          │        Message                │
       │                          │                               │
       │                          │ MessageHeader + Payload.      │
       │                          │ Header: trace_id, tick,       │
       │                          │ deadline, QoS, flags,         │
       │                          │ payload_size_bytes.           │
       │                          │                               │
       │                          │ Payload trait: buffer_desc(). │
       │                          │ Key impl: Tensor<T, N, R>     │
       │                          │ (owned, inline, Copy, ranked) │
       │                          └───────────────────────────────┘
       │
       │  also receives
       ▼
    ┌───────────────────┐    ┌────────────────────┐    ┌───────────────┐
    │  PlatformClock    │    │   Telemetry        │    │  NodePolicy   │
    │                   │    │                    │    │               │
    │ Monotonic tick    │    │ Counters, gauges,  │    │ BatchingPolicy│
    │ source. Nodes     │    │ latency, events.   │    │ BudgetPolicy  │
    │ timestamp via     │    │ StepContext emits  │    │ DeadlinePolicy│
    │ clock; runtime    │    │ per-step telemetry;│    │               │
    │ uses for budget   │    │ runtime emits      │    │ Controls how  │
    │ enforcement.      │    │ lifecycle events.  │    │ the node's    │
    │                   │    │                    │    │ step behaves. │
    │ Impls: NoopClock, │    │ Impls: NoopTelem,  │    └───────────────┘
    │ LinuxMonotonic    │    │ ConcurrentTelem[S] │
    └───────────────────┘    └────────────────────┘

  Legend: [A] = alloc feature   [S] = std feature

  ### Data Flow (One Step)

  Runtime
    │
    ├─ Scheduler selects node index from NodeSummary snapshots
    │
    ├─ GraphApi builds StepContext for that node
    │     (binds input edges, output edges, policies, clock, telemetry)
    │
    ├─ Node::step(ctx) executes:
    │     │
    │     ├─ ctx pops MessageToken from input Edge
    │     │     └─ Edge uses HeaderStore (MemoryManager) for byte accounting
    │     │
    │     ├─ MemoryManager retrieves Message by token
    │     │
    │     ├─ Node processes payload (or delegates to ComputeModel)
    │     │
    │     ├─ MemoryManager stores result Message, returns new token
    │     │
    │     └─ ctx pushes token into output Edge
    │           └─ EdgePolicy governs admit / reject / evict
    │
    └─ Runtime records StepResult, emits telemetry, loops
