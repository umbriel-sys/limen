# Inference Node Model

Inference is a first-class citizen in Limen. The `ComputeBackend` and
`ComputeModel` traits abstract inference engines — from TFLite Micro on a
Cortex-M4 to Tract on a desktop server — behind a uniform interface. The
`InferenceModel` adapter handles batching, telemetry, and lifecycle
automatically, so backend implementors focus only on inference.

---

## ComputeBackend

Factory and capability trait for loading models:

```rust
pub trait ComputeBackend<InP: Payload, OutP: Payload> {
    type Model: ComputeModel<InP, OutP>;
    type Error;
    type ModelDescriptor<'desc> where Self: 'desc;

    fn capabilities(&self) -> BackendCapabilities;
    fn load_model<'desc>(
        &self, desc: Self::ModelDescriptor<'desc>,
    ) -> Result<Self::Model, Self::Error>;
}
```

No dynamic dispatch: `Model` is a concrete associated type. `ModelDescriptor`
is backend-defined — `&[u8]` on `no_std`, `&ModelArtifact` on `std`.

---

## ComputeModel

The hot-path inference trait:

```rust
pub trait ComputeModel<InP: Payload, OutP: Payload> {
    fn init(&mut self) -> Result<(), InferenceError>;
    fn infer_one(&mut self, inp: &InP, out: &mut OutP) -> Result<(), InferenceError>;
    fn infer_batch(
        &mut self, inps: Batch<'_, InP>, outs: &mut [OutP],
    ) -> Result<(), InferenceError>;
    fn drain(&mut self) -> Result<(), InferenceError>;
    fn reset(&mut self) -> Result<(), InferenceError>;
    fn metadata(&self) -> ModelMetadata;
}
```

`infer_one` is the primary hot path — takes `&InP` (not `&Message`) and writes
to `&mut OutP`. `infer_batch` has a default implementation that loops
`infer_one`.

---

## InferenceModel Adapter

`InferenceModel<B, InP, OutP, MAX_BATCH>` wraps any `ComputeBackend` into a
`Node<1, 1, InP, OutP>`:

```rust
pub struct InferenceModel<B, InP, OutP, const MAX_BATCH: usize>
where
    B: ComputeBackend<InP, OutP>,
    InP: Payload,
    OutP: Payload + Default + Copy,
{ ... }
```

- `MAX_BATCH` is a compile-time cap. `step_batch` caps to
  `min(policy.fixed_n, backend.max_batch, MAX_BATCH)`.
- `process_message` calls `model.infer_one()` and reuses the input header for
  the output message.
- `start()` calls `model.init()`; lifecycle drain and reset are handled
  automatically.
- `NodeKind::Model`.

---

## Supporting Types

### BackendCapabilities

```rust
pub struct BackendCapabilities {
    device_streams: bool,       // Supports async device streams
    max_batch: Option<usize>,   // Max supported batch size
    dtype_mask: u64,            // Bitfield of supported data types
}
```

### ModelMetadata

```rust
pub struct ModelMetadata {
    preferred_input: MemoryClass,
    preferred_output: MemoryClass,
    max_input_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
}
```

### ModelArtifact (std)

```rust
pub struct ModelArtifact {
    data: Arc<Vec<u8>>,
    label: Option<String>,
}
```

Shared, reference-counted model bytes for loading into backends.

---

## Planned Backends

| Backend | Feature | Target | Status |
|---|---|---|---|
| TFLite Micro | `no_std` | Cortex-M4, static tensor arena | Planned |
| Tract | `std` | Desktop / server, pure Rust | Planned |

---

## Related

- [Node Model](node.md) — the full `Node` trait that `InferenceModel` implements
- [Message and Payload](message_payload.md) — tensor payload types
- [Policy Guide](policy.md) — batching policies for inference nodes
