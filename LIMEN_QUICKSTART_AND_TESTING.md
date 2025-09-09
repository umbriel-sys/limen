# Limen — Quickstart and Testing

This guide shows how to **build**, **run**, and **exercise** the code that exists in the workspace.

> All commands below assume you are at the workspace root (where the top-level `Cargo.toml` lives).
> You need a recent stable Rust toolchain and, for optional components, system dependencies (e.g., `libpaho-mqtt` is vendor-bundled via the `bundled` feature).

---

## 1) Run the dynamic example

The example wires the simulated sensor → identity pre → native-identity model → identity post → stdout sink.

```bash
cargo run -p limen-examples --example simulated_identity_stdout
```

Expected behavior: it initializes tracing, builds a runtime from an in-Rust configuration, and prints hex previews of output tensors until the simulated stream completes.

---

## 2) Drive the runtime with a TOML config (using devtools)

First, create a configuration TOML (or use `example_configs/simulated_identity_stdout.toml` in this ZIP’s docs). Then run:

```bash
# Build devtools with all integrations
cargo run -p limen-devtools --features "with-limen,with-processing,with-sensors,with-output,with-platform,with-models" -- \
  validate-config --config example_configs/simulated_identity_stdout.toml --run-steps 64 --print-health
```

- `--run-steps N` executes N cooperative `run_step` iterations.
- `--print-health` prints a `RuntimeHealth` snapshot (state, counters, queue lengths).

You can also generate CSV test data:

```bash
cargo run -p limen-devtools -- \
  generate-sim --total-frames 8 --frame-length-bytes 16 --pattern ramp --output /tmp/sim.csv
```

And inspect a compute backend (if enabled):

```bash
cargo run -p limen-devtools --features "with-models" -- \
  inspect-model --backend native-identity
```

For Tract ONNX inference, add crate features (`limen-models/backend-tract`) in your consuming binary and supply `model_path`, `input_shape`, etc., in your model configuration.

---

## 3) Example TOML configurations

### 3.1 Simulated → Stdout

```toml
# example_configs/simulated_identity_stdout.toml
sensor_stream = { factory_name = "simulated", configuration = { parameters = {
  frame_length_bytes = "16",
  total_frames = "32",
  pattern = "ramp",
  timestamp_step_nanoseconds = "1000000",
  sequence_start = "0",
  sample_rate_hz = "0",
  channel_count = "1"
}}}

preprocessor = { factory_name = "identity", configuration = { parameters = {
  data_type = "Unsigned8",
  shape = "1,16"
}}}

model = { compute_backend_factory_name = "native-identity",
          compute_backend_configuration = { parameters = {} } }

postprocessor = { factory_name = "identity", configuration = { parameters = {} }}

output_sink = { factory_name = "stdout", configuration = { parameters = {
  preview_bytes = "16"
}}}

# Optional platform (desktop) - currently no parameters
# platform_backend = { factory_name = "desktop", configuration = { parameters = {} } }

preprocessor_input_queue_capacity = 64
postprocessor_output_queue_capacity = 64

backpressure_policy_for_preprocessor_input_queue = "Block"
backpressure_policy_for_postprocessor_output_queue = "DropOldest"
```

### 3.2 CSV → Stdout

```toml
# example_configs/csv_to_stdout.toml
sensor_stream = { factory_name = "csv", configuration = { parameters = {
  path = "path/to/frames.csv",
  skip_header = "false",
  timestamp_step_nanoseconds = "1000000",
  sequence_start = "0",
  sample_rate_hz = "0",
  channel_count = "1"
}}}

preprocessor = { factory_name = "identity", configuration = { parameters = {
  data_type = "Unsigned8",
  shape = "1,16"
}}}

model = { compute_backend_factory_name = "native-identity",
          compute_backend_configuration = { parameters = {} } }

postprocessor = { factory_name = "identity", configuration = { parameters = {} }}

output_sink = { factory_name = "stdout", configuration = { parameters = {
  preview_bytes = "16"
}}}

preprocessor_input_queue_capacity = 64
postprocessor_output_queue_capacity = 64
backpressure_policy_for_preprocessor_input_queue = "Block"
backpressure_policy_for_postprocessor_output_queue = "DropOldest"
```

> For CSV, create `frames.csv` with lines like:  
> `0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15`

---

## 4) Run the `limen-light` no-alloc example

```bash
cargo run -p limen-light --example no_alloc_queued_stdout --features no_alloc
```

This constructs the static, `no_alloc` runtime, runs a cooperative loop, and prints previews to stdout.

---

## 5) Writing your own component

1. Create a crate that depends on `limen-core`.
2. Implement the trait(s) (e.g., `SensorStream`) and a corresponding `*Factory`.
3. In your binary, call a `register_all(&mut Registries)` function to insert your factories.
4. Refer to the **Configuration Keys** in the Architecture doc to parse your parameters.

---

## 6) Sanity Testing Checklist

- `cargo check --workspace` (basic validation)
- Dynamic path: run the example (section 1) and the devtools `validate-config` (section 2).
- No-alloc path: run the `limen-light` example (section 4).
- Optional components:
  - CSV: ensure the `limen-sensors` crate is built with `csv` feature in your consuming binary.
  - Serial: `serialport` feature and device present.
  - MQTT: `mqtt-paho` features enabled; broker reachable.
  - GPIO: `gpio-rpi` with access to GPIO pins on Raspberry Pi.

---

## 7) Troubleshooting

- **Factory not found**: ensure the crate exposing that factory was compiled **and** its `register_all` was called.
- **Invalid shape or dtype**: `TensorInput::new(...)` and `TensorOutput::new(...)` validate `shape × dtype == buffer length`.
- **Backpressure drops**: inspect `RuntimeHealth` counters and adjust queue capacities or policies.
- **no_std build issues**: enable the right feature combinations (`alloc` vs `no_alloc`) and remove uses of `std` components.
