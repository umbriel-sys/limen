# Limen Workspace

This is the complete Limen workspace assembled from our conversation.

Crates:
- `limen-core` – core traits, types, errors, and the generic runtime.
- `limen` – dynamic orchestration (config + registries + builder).
- `limen-processing` – preprocessors/postprocessors.
- `limen-sensors` – sensors (simulated, csv, serial, mqtt).
- `limen-output` – sinks (stdout, file, mqtt, gpio).
- `limen-models` – model backends (native-identity; Tract real backend; ORT placeholder).
- `limen-platform` – platform glue (desktop).
- `limen-examples` – runnable examples.
- `limen-devtools` – CLI tools (validate-config, generate-sim, inspect-model).
- `limen-light` – minimal, embedded-friendly wrapper; and a no-alloc path with optional heapless queues.
