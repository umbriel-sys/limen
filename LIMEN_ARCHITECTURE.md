# Limen — Architecture and Codebase Guide (as of current workspace)

This document explains the Limen workspace as it exists in the provided ZIP, crate by crate,
and shows how the parts fit together at build time and at runtime. It also lists public traits,
key types, configurable factories, and the supported components with their configuration keys.

> **Truthfulness note**: This guide reflects the exact code structure found in the uploaded project.
> Where something is optional behind features, it is called out, and where something is missing or not
> yet implemented, it is clearly indicated.

---

## 1) Workspace Layout

Top-level `Cargo.toml` defines a single workspace with these member crates:

- **`limen-core`** — foundational traits, types, errors, and the generic cooperative `Runtime`.
- **`limen`** — dynamic orchestration: configuration, boxed adapters, registries, builder, and basic observability.
- **`limen-processing`** — preprocessors and postprocessors (with a `register_all` helper).
- **`limen-sensors`** — sensor streams (simulated, CSV, serial port, MQTT/Paho).
- **`limen-output`** — output sinks (stdout, file, optional MQTT/Paho and GPIO via `rppal`).
- **`limen-models`** — model backends (native identity; optional Tract/ONNX).
- **`limen-platform`** — platform integration (desktop constraints backend).
- **`limen-examples`** — runnable example(s).
- **`limen-devtools`** — CLI utilities (validate config, generate simulated data, inspect models).
- **`limen-light`** — minimal embedded/no-alloc path with a distinct `no_alloc` runtime.

All crates use feature flags to opt into `std`/`alloc`, `serde`, and component-specific backends.

---

## 2) `limen-core` (foundational)

### 2.1 Traits (in `limen-core/src/traits`)

Core traits define the processing pipeline “shape”:

- **`SensorStream`**
  - `open(&mut self) -> Result<(), SensorError>`
  - `read_next(&mut self) -> Result<Option<SensorData<'_>>, SensorError>`
  - `reset(&mut self) -> Result<(), SensorError>`
  - `close(&mut self) -> Result<(), SensorError>`
  - `describe(&self) -> Option<SensorSampleMetadata>` (optional metadata)
- **`Preprocessor`**
  - `process(&mut self, SensorData<'_>) -> Result<TensorInput, ProcessingError>`
  - `reset(&mut self) -> Result<(), ProcessingError>`
- **`Model`**
  - `model_metadata(&self) -> ModelMetadata`
  - `infer(&mut self, &TensorInput) -> Result<TensorOutput, InferenceError>`
  - `unload(&mut self) -> Result<(), InferenceError>`
- **`Postprocessor`**
  - `process(&mut self, TensorOutput) -> Result<TensorOutput, ProcessingError>`
  - `reset(&mut self) -> Result<(), ProcessingError>`
- **`OutputSink`**
  - `write(&mut self, &TensorOutput) -> Result<(), OutputError>`
  - `flush(&mut self) -> Result<(), OutputError>` (default `Ok(())`)
  - `close(&mut self) -> Result<(), OutputError>` (default `Ok(())`)

Factory traits:

- `SensorStreamFactory`, `PreprocessorFactory`, `PostprocessorFactory`, `OutputSinkFactory` — each provides a `*_name()` and a `create_*(&Configuration) -> Result<Box<dyn *>, *_Error>`.
- `ComputeBackend` — `load_model(&mut self, &ComputeBackendConfiguration, metadata_hint: Option<&ModelMetadata>) -> Result<Box<dyn Model>, InferenceError>`.
- `ComputeBackendFactory` — `backend_name()`, `create_backend()`.
- `PlatformBackend` — exposes `constraints()` and lifecycle `initialize()`/`shutdown()`.
- `PlatformBackendFactory` — `platform_name()`, `create_platform_backend(&PlatformBackendConfiguration)`.

Configuration holder types used by factories are defined in `traits/configuration.rs`:
`SensorStreamConfiguration`, `PreprocessorConfiguration`, `PostprocessorConfiguration`,
`OutputSinkConfiguration`, `ComputeBackendConfiguration`, `PlatformBackendConfiguration`.
Each is a thin `label: Option<String>` + `parameters: BTreeMap<String, String>` container (behind `alloc`/`serde`).

### 2.2 Types (in `limen-core/src/types.rs`)

- **`DataType`**: `Boolean`, `Unsigned8/16/32/64`, `Signed8/16/32/64`, `Float16/BFloat16/Float32/Float64`, with `byte_size()`.
- **`SensorData<'a>`**: `timestamp`, `sequence_number`, `payload: Cow<'a, [u8]>`, `metadata: Option<SensorSampleMetadata>`.
- **`SensorSampleMetadata`**: optional `channel_count`, `sample_rate_hz`, `notes`.
- **`ModelMetadata`**: `inputs: Vec<ModelPort>`, `outputs: Vec<ModelPort>` (serde-enabled if feature on).
- **`ModelPort`**: `name: Option<String>`, `data_type: DataType`, `shape: Box<[usize]>`, `quantization: Option<QuantizationParams>`.
- **`TensorInput` / `TensorOutput`**: `data_type`, `shape`, `strides: Option<Box<[usize]>>`, `buffer: Vec<u8>`; both provide safe `new(...)` constructors that validate shape × dtype byte count.
- **`QuantizationParams`**: `scale: f32`, `zero_point: i32`.

### 2.3 Errors (in `limen-core/src/errors.rs`)

Error enums: `SensorError`, `ProcessingError`, `InferenceError`, `OutputError`, `RuntimeError`, each with meaningful variants (`OpenFailed`, `ReadFailed`, `ConfigurationInvalid`, `BackendUnavailable`, etc.) and `Display` impls (and `std::error::Error` impls when `std` feature is enabled).

### 2.4 Runtime (in `limen-core/src/runtime.rs`)

- **Flow control**:
  - `BackpressurePolicy` = `{ Block, DropNewest, DropOldest }`
  - Internal bounded queues: `pre_q: VecDeque<TensorInput>`, `post_q: VecDeque<TensorOutput>`
  - Policies are independently configurable per-queue.

- **State & stats**:
  - `RuntimeState` = `{ Initialized, Running, Stopping, Stopped }`
  - `FlowStatistics`:
    - `total_sensor_samples_received`,
    - `total_preprocessor_outputs_produced`,
    - `total_model_inferences_completed`,
    - `total_postprocessor_outputs_produced`,
    - `total_outputs_written`,
    - `total_dropped_at_preprocessor_input_queue`,
    - `total_dropped_at_postprocessor_output_queue`.
  - `RuntimeHealth` (snapshot): state + stats + lengths/capacities of the two queues.

- **Generic `Runtime<S,P,M,Q,O>`** where `S: SensorStream`, `P: Preprocessor`, `M: Model`, `Q: Postprocessor`, `O: OutputSink`.
  - `pub fn new(...) -> Self` (accepts components, queue capacities, and backpressure policies).
  - `pub fn open(&mut self) -> Result<(), RuntimeError>` — calls `open`/`initialize` across the pipeline.
  - `pub fn request_stop(&mut self)` — transition toward `Stopping`.
  - `pub fn run_step(&mut self) -> Result<bool, RuntimeError>` — cooperative single “tick”; drives: `SensorStream -> Preprocessor -> Model -> Postprocessor -> OutputSink` while respecting queue policies. Returns whether anything progressed.
  - `pub fn drain(&mut self) -> Result<(), RuntimeError>` — completes in-flight work until queues empty.
  - `pub fn run_blocking(&mut self, idle_sleep_duration_milliseconds: u64) -> Result<(), RuntimeError>` — repeatedly `run_step()` with a small idle sleep.
  - `pub fn health(&self) -> RuntimeHealth` and `pub fn statistics(&self) -> &FlowStatistics`.

---

## 3) `limen` (orchestration)

`limen` turns the generic runtime into a *config-driven* system with registries.

### 3.1 Configuration model (`limen/src/config.rs`)

- **`NamedConfiguration<T>`**: `{ factory_name: String, configuration: T }`.
- **`ModelRuntimeConfiguration`**:
  - `compute_backend_factory_name: String`
  - `compute_backend_configuration: ComputeBackendConfiguration`
  - `metadata_hint: Option<ModelMetadata>`
- **`BackpressurePolicySetting`** maps to `BackpressurePolicy`.
- **`RuntimeConfiguration`**:
  - `sensor_stream: NamedConfiguration<SensorStreamConfiguration>`
  - `preprocessor: NamedConfiguration<PreprocessorConfiguration>`
  - `model: ModelRuntimeConfiguration`
  - `postprocessor: NamedConfiguration<PostprocessorConfiguration>`
  - `output_sink: NamedConfiguration<OutputSinkConfiguration>`
  - `platform_backend: Option<NamedConfiguration<PlatformBackendConfiguration>>`
  - `preprocessor_input_queue_capacity: usize` (default 64)
  - `postprocessor_output_queue_capacity: usize` (default 64)
  - `backpressure_policy_for_preprocessor_input_queue: BackpressurePolicySetting`
  - `backpressure_policy_for_postprocessor_output_queue: BackpressurePolicySetting`
  - Helpers when `serde`+`toml` or `serde`+`json` are enabled:
    - `RuntimeConfiguration::from_toml_str(...)`
    - `RuntimeConfiguration::from_json_str(...)`

### 3.2 Registries (`limen/src/registry.rs`)

`Registries` holds BTreeMaps of factories for each component type and provides:

- `register_*_factory_with_name(name, factory)` and convenience `register_*_factory(factory)` to insert by factory-provided name.
- `get_*_factory(name)` lookup methods.
- Duplicate insertion is guarded by a `RegistryError::DuplicateFactoryRegistration`.

### 3.3 Boxed wrappers & builder

- `boxed.rs` contains `BoxedSensorStream`, `BoxedPreprocessor`, `BoxedModel`, `BoxedPostprocessor`, `BoxedOutputSink`
  that implement the corresponding traits by delegating to `Box<dyn ...>`.
- `builder.rs` constructs a `BuiltRuntime`, which wraps:
  - `runtime: Runtime<BoxedSensorStream, BoxedPreprocessor, BoxedModel, BoxedPostprocessor, BoxedOutputSink>`
  - `platform_backend: Option<Box<dyn PlatformBackend>>`
- `build_from_configuration(&RuntimeConfiguration, &Registries) -> Result<BuiltRuntime, BuildError>` pulls each named factory from the registries and wires the pipeline using the provided `parameters` maps.
- `observability_init.rs` exposes `initialize_observability()` (via `tracing` when enabled).

---

## 4) Component crates

Each component crate exposes concrete implementations and a `register_all(registries: &mut limen::Registries)` function (behind a `register` feature in most crates) to populate the registries with known components.

### 4.1 `limen-processing`

**Preprocessors**:
- `identity` — pass-through; parameters:  
  - `data_type` (e.g., `Unsigned8`, `Float32`)  
  - `shape` (CSV like `"1,16000"`)
- `normalize` — affine transform; parameters:  
  - `scale` (default `1.0`), `offset` (default `0.0`), optional `output_shape`
- `window` — fixed windowing; parameters:  
  - `window_length_elements`, `hop_length_elements`, optional `output_shape`

**Postprocessors**:
- `identity` — pass-through
- `threshold` — scalar threshold; parameters:  
  - `threshold` (float)
- `debounce` — count-based on/off debounce; parameters:  
  - `activation_count`, `deactivation_count`

### 4.2 `limen-sensors`

- `simulated` — emits synthetic frames. Parameters:
  - `frame_length_bytes`, `total_frames`, `pattern` (`zeros`|`ramp`|`constant`), `constant_value` (when `pattern = "constant"`),
  - `timestamp_step_nanoseconds`, `sequence_start`,
  - optional `sample_rate_hz`, `channel_count`.
- `csv` (requires `csv` feature + `std`) — reads comma-separated bytes per line from `path`. Parameters:
  - `path`, `skip_header` (`true`|`false`), `timestamp_step_nanoseconds`, `sequence_start`, optional `sample_rate_hz`, `channel_count`.
- `serial` (requires `serialport` + `std`) — reads from a serial port. Parameters:
  - `port_name`, `baud_rate`, `frame_length_bytes`, `timestamp_step_nanoseconds`, `sequence_start`, optional `sample_rate_hz`, `channel_count`.
- `mqtt` (requires `mqtt-paho` + `std`) — subscribes to a topic and uses payload bytes as frames. Parameters:
  - `server_uri`, `client_id`, `topic`, `qos`, `timestamp_step_nanoseconds`, `sequence_start`, optional `sample_rate_hz`, `channel_count`.

### 4.3 `limen-output` (sinks)

- `stdout` — prints a hex preview. Parameters:
  - `preview_bytes` (defaults to a small number if omitted).
- `file` — appends or writes binary frames to a file. Parameters:
  - `path`, `append` (`true`|`false`), optional `preview_bytes`.
- `mqtt` (requires `mqtt-paho` + `std`) — publishes output buffers to a topic. Parameters:
  - `server_uri`, `client_id`, `topic`, `qos`, `retain`.
- `gpio` (requires `gpio-rpi`/`rppal`) — toggles a GPIO pin. Parameters:
  - `pin` (u8), `active_high` (`true`|`false`).

### 4.4 `limen-models` (compute backends)

- `native-identity` (default) — “model” that returns the input tensor unchanged. Parameters: **none**. Optionally respects `metadata_hint`.
- `tract` (requires `backend-tract` feature) — loads an ONNX with `tract-onnx`. Common parameters:
  - `model_path`
  - optional `input_shape` (CSV like `"1,16000"`)
  - optional `input_data_type` (e.g., `Float32`)
- `onnxruntime` — placeholder factory (returns `BackendUnavailable`).

### 4.5 `limen-platform`

- `desktop` PlatformBackend (no-op initialize/shutdown). Exposes typical constraints such as `recommended_worker_thread_count` and flags for GPU / GPIO presence.

---

## 5) `limen-examples`

Contains `examples/simulated_identity_stdout.rs`. It demonstrates:
- Initializing tracing (`initialize_observability()`),
- Building `Registries` and calling each crate’s `register_all(...)`,
- Constructing a full `RuntimeConfiguration` for:
  - `sensor_stream = "simulated"`,
  - `preprocessor = "identity"`,
  - `model = "native-identity"`,
  - `postprocessor = "identity"`,
  - `output_sink = "stdout"`,
- Building with `build_from_configuration(...)` and running `runtime.run_blocking(2)`.

All crate features needed by the example are already declared in the example crate’s `Cargo.toml` dependencies (`std`, `register`, `stdout`, and `native-identity`).

---

## 6) `limen-devtools` (CLI)

`limen-devtools` provides a multi-command CLI. Features gate which dependencies are compiled:

- **Features**: `with-limen`, `with-processing`, `with-sensors`, `with-output`, `with-platform`, `with-models`, (`with-tract` optional for Tract).
- **Commands**:
  - `validate-config --config PATH [--run-steps N] [--print-health]`  
    Parses a TOML `RuntimeConfiguration`, builds a pipeline with registered components, optionally runs `N` steps, and can print a `RuntimeHealth` snapshot.
  - `generate-sim --total-frames N --frame-length-bytes M [--pattern zeros|ramp|constant] [--constant-value V] [--output FILE]`  
    Emits CSV of simulated frames to stdout or a file (each line is `M` comma-separated bytes).
  - `inspect-model --backend NAME --parameter key=value ...` *(with `with-models`)*  
    Instantiates a backend to print `ModelMetadata` for debugging.

---

## 7) `limen-light` (embedded/no-alloc path)

- Exposes `no_alloc` runtime and traits under `limen_light::no_alloc::*` (guarded by the `no_alloc` feature).
- Contains components: `sensor_simulated`, `pre_identity`, `model_identity`, `post_identity`, `sink_stdout` with fully static buffers and fixed shapes.
- Example `examples/no_alloc_queued_stdout.rs` shows constructing a `NoAllocQueuedRuntime<...>` with compile-time capacities and running a cooperative loop.

This path is intended for `#![no_std]` targets with optional `heapless` queues and no dynamic allocation.

---

## 8) Putting It Together at Runtime

A typical flow (dynamic, with `limen`):

1. **Factory registration**: call `register_all` from `limen-processing`, `limen-sensors`, `limen-output`, `limen-platform`, `limen-models` into a mutable `Registries`.
2. **Parse configuration**: build a `RuntimeConfiguration` (from TOML/JSON or programmatically).
3. **Builder**: `build_from_configuration(...)` resolves each factory by name, creates the concrete objects using `parameters`, and returns a `BuiltRuntime`.
4. **Run**: `built.runtime_mut().open()?; built.runtime_mut().run_blocking(2)?;` or a manual step loop.
5. **Observe**: query `runtime.health()` or `runtime.statistics()` for counters to wire into logs or metrics.
6. **Shutdown**: request stop, drain, and close.

Backpressure policies and queue capacities are specified at configuration time and enforced by the runtime around the pre/post queues only (sensors and sinks are pull/push driven inside `run_step`).

---

## 9) Configuration Keys (reference)

Below are the **recognized `parameters` keys** by component (derived from the code):

**Sensors**
- `simulated`: `frame_length_bytes`, `total_frames`, `pattern`, `constant_value`, `timestamp_step_nanoseconds`, `sequence_start`, `sample_rate_hz`, `channel_count`
- `csv`: `path`, `skip_header`, `timestamp_step_nanoseconds`, `sequence_start`, `sample_rate_hz`, `channel_count`
- `serial`: `port_name`, `baud_rate`, `frame_length_bytes`, `timestamp_step_nanoseconds`, `sequence_start`, `sample_rate_hz`, `channel_count`
- `mqtt`: `server_uri`, `client_id`, `topic`, `qos`, `timestamp_step_nanoseconds`, `sequence_start`, `sample_rate_hz`, `channel_count`

**Preprocessors**
- `identity`: `data_type`, `shape`
- `normalize`: `scale`, `offset`, `output_shape`
- `window`: `window_length_elements`, `hop_length_elements`, `output_shape`

**Postprocessors**
- `identity`: *(none)*
- `threshold`: `threshold`
- `debounce`: `activation_count`, `deactivation_count`

**Output sinks**
- `stdout`: `preview_bytes`
- `file`: `path`, `append`, `preview_bytes`
- `mqtt`: `server_uri`, `client_id`, `topic`, `qos`, `retain`
- `gpio`: `pin`, `active_high`

**Model backends**
- `native-identity`: *(none)*
- `tract`: `model_path`, `input_shape`, `input_data_type`

**Platform backend**
- `desktop`: *(none currently used)*

All numeric values are parsed using Rust’s `parse()` on strings from the `parameters` map. Shapes are CSV lists of positive integers (e.g., `"1,16000"`). Boolean-like flags accept `"1"`, `"true"`, `"0"`, or `"false"` (case-insensitive) depending on the individual parser.

---

## 10) Extending Limen

To add a new component:
1. Implement the appropriate trait(s) from `limen-core` (and a `*Factory`).
2. Expose a `register_all` that calls `registries.register_*_factory_with_name(...)`.
3. Gate with a crate feature if optional.
4. Use the type-safe `TensorInput`/`TensorOutput` constructors to avoid shape × dtype mismatches.

---

## 11) Notes on Features and Build Profiles

- `std` enables integration with the standard library (I/O, threads). Without `std`, crates compile for `no_std` if they also have `alloc` or `no_alloc` paths.
- `serde` is optional; when enabled on `limen` and `limen-core`, configuration and metadata types serialize/deserialize.
- Component crates use feature flags for optional dependencies (`mqtt-paho`, `serialport`, `rppal`, `backend-tract`).

---

## 12) Known Example Entrypoints

- `limen-examples/examples/simulated_identity_stdout.rs` (dynamic runtime with registries).
- `limen-light/examples/no_alloc_queued_stdout.rs` (embedded/no-alloc runtime).

See the Quickstart for exact commands and sample configurations.
