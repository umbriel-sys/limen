use limen::builder::build_from_configuration;
use limen::observability_init::initialize_observability;
use limen::Registries;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    initialize_observability().unwrap();

    let mut registries = Registries::new();
    limen_processing::register_all(&mut registries)?;
    limen_sensors::register_all(&mut registries)?;
    limen_output::register_all(&mut registries)?;
    limen_platform::register_all(&mut registries)?;
    limen_models::register_all(&mut registries)?;

    use limen::config::{
        BackpressurePolicySetting, ModelRuntimeConfiguration, NamedConfiguration,
        RuntimeConfiguration,
    };
    use limen_core::traits::configuration::{
        ComputeBackendConfiguration, OutputSinkConfiguration, PlatformBackendConfiguration,
        PostprocessorConfiguration, PreprocessorConfiguration, SensorStreamConfiguration,
    };
    use std::collections::BTreeMap;

    let mut sensor_params = BTreeMap::new();
    sensor_params.insert("frame_length_bytes".to_string(), "16".to_string());
    sensor_params.insert("total_frames".to_string(), "5".to_string());
    sensor_params.insert("pattern".to_string(), "ramp".to_string());

    let mut pre_params = BTreeMap::new();
    pre_params.insert("data_type".to_string(), "Unsigned8".to_string());

    let model_params = BTreeMap::new();
    let post_params = BTreeMap::new();

    let mut sink_params = BTreeMap::new();
    sink_params.insert("preview_bytes".to_string(), "8".to_string());

    let sensor_stream = NamedConfiguration::<SensorStreamConfiguration> {
        factory_name: "simulated".to_string(),
        configuration: SensorStreamConfiguration {
            label: Some("sim".to_string()),
            parameters: sensor_params,
        },
    };
    let preprocessor = NamedConfiguration::<PreprocessorConfiguration> {
        factory_name: "identity".to_string(),
        configuration: PreprocessorConfiguration {
            label: Some("identity_pre".to_string()),
            parameters: pre_params,
        },
    };
    let model = ModelRuntimeConfiguration {
        compute_backend_factory_name: "native-identity".to_string(),
        compute_backend_configuration: ComputeBackendConfiguration {
            label: Some("native".to_string()),
            parameters: model_params,
        },
        metadata_hint: None,
    };
    let postprocessor = NamedConfiguration::<PostprocessorConfiguration> {
        factory_name: "identity".to_string(),
        configuration: PostprocessorConfiguration {
            label: Some("identity_post".to_string()),
            parameters: post_params,
        },
    };
    let output_sink = NamedConfiguration::<OutputSinkConfiguration> {
        factory_name: "stdout".to_string(),
        configuration: OutputSinkConfiguration {
            label: Some("stdout_sink".to_string()),
            parameters: sink_params,
        },
    };
    let platform_backend = Some(NamedConfiguration::<PlatformBackendConfiguration> {
        factory_name: "desktop".to_string(),
        configuration: PlatformBackendConfiguration {
            label: Some("desktop".to_string()),
            parameters: BTreeMap::new(),
        },
    });

    let configuration = RuntimeConfiguration {
        sensor_stream,
        preprocessor,
        model,
        postprocessor,
        output_sink,
        platform_backend,
        preprocessor_input_queue_capacity: 8,
        postprocessor_output_queue_capacity: 8,
        backpressure_policy_for_preprocessor_input_queue: BackpressurePolicySetting::Block,
        backpressure_policy_for_postprocessor_output_queue: BackpressurePolicySetting::DropOldest,
    };

    let mut built = build_from_configuration(&configuration, &registries)
        .map_err(|e| format!("build failed: {e}"))?;
    built
        .runtime_mut()
        .run_blocking(2)
        .map_err(|e| format!("runtime error: {e}"))?;
    Ok(())
}
