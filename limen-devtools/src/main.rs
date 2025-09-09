use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "limen-devtools", version, about = "Developer tools for the Limen runtime")]
struct CommandLineInterface {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    ValidateConfig {
        #[arg(long)]
        config: String,
        #[arg(long)]
        run_steps: Option<u64>,
        #[arg(long)]
        print_health: bool,
    },
    GenerateSim {
        #[arg(long)]
        total_frames: u64,
        #[arg(long)]
        frame_length_bytes: usize,
        #[arg(long, default_value = "zeros")]
        pattern: String,
        #[arg(long, default_value_t = 0)]
        constant_value: u8,
        #[arg(long)]
        output: Option<String>,
    },
    InspectModel {
        #[arg(long)]
        backend: String,
        #[arg(long = "parameter")]
        parameters: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = CommandLineInterface::parse();
    match cli.command {
        Commands::ValidateConfig { config, run_steps, print_health } => cmd_validate_config(&config, run_steps, print_health),
        Commands::GenerateSim { total_frames, frame_length_bytes, pattern, constant_value, output } => {
            cmd_generate_sim(total_frames, frame_length_bytes, &pattern, constant_value, output.as_deref())
        }
        Commands::InspectModel { backend, parameters } => cmd_inspect_model(&backend, &parameters),
    }
}

#[cfg(feature = "with-limen")]
fn cmd_validate_config(path: &str, run_steps: Option<u64>, print_health: bool) -> Result<()> {
    use std::fs;
    use limen::observability_init::initialize_observability;
    use limen::{Registries};
    use limen::config::RuntimeConfiguration;
    use limen::builder::build_from_configuration;

    initialize_observability().ok();
    let toml_text = fs::read_to_string(path).with_context(|| format!("failed to read configuration file: {path}"))?;
    let configuration = RuntimeConfiguration::from_toml_str(&toml_text).map_err(|e| anyhow!("TOML parse error: {e}"))?;

    let mut registries = Registries::new();
    #[cfg(feature = "with-processing")] { limen_processing::register_all(&mut registries)?; }
    #[cfg(feature = "with-sensors")] { limen_sensors::register_all(&mut registries)?; }
    #[cfg(feature = "with-output")] { limen_output::register_all(&mut registries)?; }
    #[cfg(feature = "with-platform")] { limen_platform::register_all(&mut registries)?; }
    #[cfg(feature = "with-models")] { limen_models::register_all(&mut registries)?; }

    let mut built = build_from_configuration(&configuration, &registries).map_err(|e| anyhow!("build_from_configuration failed: {e}"))?;

    if let Some(steps) = run_steps {
        built.runtime_mut().open()?;
        for _ in 0..steps { let _ = built.runtime_mut().run_step()?; }
        built.runtime_mut().request_stop();
        built.runtime_mut().drain()?;
        if print_health {
            let health = built.runtime().health();
            println!(
                "Runtime health: state={:?}, stats={{ sensor: {}, pre_out: {}, inf: {}, post_out: {}, written: {}, drop_pre_in: {}, drop_post_out: {} }}, pre_q={}/{}, post_q={}/{}",
                health.state,
                health.statistics.total_sensor_samples_received,
                health.statistics.total_preprocessor_outputs_produced,
                health.statistics.total_model_inferences_completed,
                health.statistics.total_postprocessor_outputs_produced,
                health.statistics.total_outputs_written,
                health.statistics.total_dropped_at_preprocessor_input_queue,
                health.statistics.total_dropped_at_postprocessor_output_queue,
                health.preprocessor_input_queue_length,
                health.preprocessor_input_queue_capacity,
                health.postprocessor_output_queue_length,
                health.postprocessor_output_queue_capacity
            );
        }
    }

    println!("Configuration validated successfully.");
    Ok(())
}

#[cfg(not(feature = "with-limen"))]
fn cmd_validate_config(_path: &str, _run_steps: Option<u64>, _print_health: bool) -> Result<()> {
    Err(anyhow!("validate-config requires the 'with-limen' feature."))
}

fn cmd_generate_sim(total_frames: u64, frame_length_bytes: usize, pattern: &str, constant_value: u8, output: Option<&str>) -> Result<()> {
    use std::io::{Write, BufWriter};
    use std::fs::File;
    if frame_length_bytes == 0 { return Err(anyhow!("frame_length_bytes must be greater than zero")); }

    let mut writer: Box<dyn Write> = match output {
        Some(path) => Box::new(BufWriter::new(File::create(path).with_context(|| format!("failed to create output file: {path}"))?)),
        None => Box::new(BufWriter::new(std::io::stdout())),
    };

    for frame_index in 0..total_frames {
        let line = match pattern.to_ascii_lowercase().as_str() {
            "zeros" => (0..frame_length_bytes).map(|_| "0".to_string()).collect::<Vec<_>>().join(","),
            "ramp"  => (0..frame_length_bytes).map(|i| ((i % 256) as u8).to_string()).collect::<Vec<_>>().join(","),
            "constant" => (0..frame_length_bytes).map(|_| constant_value.to_string()).collect::<Vec<_>>().join(","),
            other => return Err(anyhow!(format!("unsupported pattern '{other}' (use zeros|ramp|constant)"))),
        };
        writeln!(writer, "{line}").with_context(|| format!("failed to write frame {frame_index}"))?;
    }
    Ok(())
}

#[cfg(feature = "with-models")]
fn cmd_inspect_model(backend_name: &str, parameters: &[String]) -> Result<()> {
    use limen_core::traits::configuration::ComputeBackendConfiguration;
    use limen_core::traits::{ComputeBackend, ComputeBackendFactory, Model};
    use limen::{Registries};
    use std::collections::BTreeMap;

    let mut param_map = BTreeMap::<String, String>::new();
    for kv in parameters {
        let parts: Vec<&str> = kv.splitn(2, '=').collect();
        if parts.len() != 2 { return Err(anyhow!(format!("invalid parameter '{kv}', expected key=value"))); }
        param_map.insert(parts[0].to_string(), parts[1].to_string());
    }

    let mut registries = Registries::new();
    limen_models::register_all(&mut registries)?;

    let factory = registries.get_compute_backend_factory(backend_name)
        .ok_or_else(|| anyhow!(format!("backend factory not found: '{backend_name}'")))?.as_ref();

    let mut backend = factory.create_backend().map_err(|e| anyhow!(format!("failed to create backend: {e}")))?;

    let configuration = ComputeBackendConfiguration { label: Some(format!("devtools:{backend_name}")), parameters: param_map };
    let mut model = backend.load_model(&configuration, None).map_err(|e| anyhow!(format!("failed to load model: {e}")))?;
    let metadata = model.model_metadata();

    #[cfg(feature = "alloc")]
    {
        println!("Model metadata (may be empty):");
        println!("  inputs: {}", metadata.inputs.len());
        for (i, port) in metadata.inputs.iter().enumerate() {
            println!("    [{i}] name={:?}, data_type={}, shape={:?}, quant={:?}", port.name, port.data_type, port.shape, port.quantization);
        }
        println!("  outputs: {}", metadata.outputs.len());
        for (i, port) in metadata.outputs.iter().enumerate() {
            println!("    [{i}] name={:?}, data_type={}, shape={:?}, quant={:?}", port.name, port.data_type, port.shape, port.quantization);
        }
    }

    model.unload().ok();
    Ok(())
}

#[cfg(not(feature = "with-models"))]
fn cmd_inspect_model(_backend_name: &str, _parameters: &[String]) -> Result<()> {
    Err(anyhow!("inspect-model requires the 'with-models' feature."))
}
