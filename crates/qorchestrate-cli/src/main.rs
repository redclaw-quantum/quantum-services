mod cli;
mod monitor;
mod routes;
mod server;
mod xeb;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(name = "qorchestrate", version = "0.1.0", about = "Quantum pipeline orchestrator")]
struct Cli {
    /// quantum-api base URL
    #[arg(
        long,
        global = true,
        default_value = "http://localhost:8765",
        env = "QUANTUM_API_URL"
    )]
    quantum_api_url: String,
    /// QPUDIDP optimizer URL
    #[arg(
        long,
        global = true,
        default_value = "http://localhost:8420",
        env = "QPUDIDP_URL"
    )]
    qpudidp_url: String,
    /// Pipeline templates directory
    #[arg(
        long,
        global = true,
        default_value = "/nvme/quantum/quantum-services/templates",
        env = "TEMPLATES_DIR"
    )]
    templates_dir: PathBuf,
    /// Brain file for clawhdf5 artifact storage
    #[arg(
        long,
        global = true,
        default_value = "/nvme/quantum/data/brains/lab3-qpu.brain",
        env = "BRAIN_PATH"
    )]
    brain_path: String,
    /// Checkpoint directory
    #[arg(
        long,
        global = true,
        default_value = "/tmp/qorchestrate/checkpoints",
        env = "CHECKPOINT_DIR"
    )]
    checkpoint_dir: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP server
    Serve {
        #[arg(long, default_value = "8767")]
        port: u16,
        /// Chip ID to monitor for drift (default: lab3-qpu)
        #[arg(long)]
        monitor_chip: Option<String>,
        /// Monitor poll interval in seconds (default: 600)
        #[arg(long, default_value = "600")]
        monitor_interval: u64,
        /// Disable the drift-triggered recalibration monitor
        #[arg(long)]
        no_monitor: bool,
    },
    /// Run a pipeline by template name
    Run {
        /// Template name (e.g. design-to-chip)
        #[arg(long)]
        template: String,
        /// Pipeline parameters as key=value pairs
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        /// Stream events to stderr until completion
        #[arg(long)]
        follow: bool,
        /// Validate only, no execution
        #[arg(long)]
        dry_run: bool,
    },
    /// Resume a pipeline from its checkpoint
    Resume {
        #[arg(long)]
        run_id: uuid::Uuid,
    },
    /// Get status of a pipeline run
    Status {
        #[arg(long)]
        run_id: uuid::Uuid,
    },
    /// Get result of a completed pipeline run
    Result {
        #[arg(long)]
        run_id: uuid::Uuid,
    },
    /// Validate a pipeline template
    Validate {
        /// Template name
        #[arg(long, conflicts_with = "file")]
        template: Option<String>,
        /// Path to TOML file
        #[arg(long, conflicts_with = "template")]
        file: Option<PathBuf>,
    },
    /// Print Mermaid DAG for a template
    Dag {
        #[arg(long)]
        template: String,
    },
    /// List available pipeline templates
    ListTemplates,
    /// Run a Linear Cross-Entropy Benchmark (XEB) verification
    XebVerify {
        /// Number of qubits in the random circuit
        #[arg(long, default_value = "10")]
        n_qubits: usize,
        /// Number of independent random circuits to average over
        #[arg(long, default_value = "20")]
        n_circuits: usize,
        /// Number of measurement shots per circuit
        #[arg(long, default_value = "1000")]
        n_shots: usize,
        /// Total number of two-qubit gates in each circuit
        #[arg(long, default_value = "40")]
        n_gates: usize,
        /// Per-gate fidelity (depolarizing noise model)
        #[arg(long, default_value = "0.995")]
        fidelity: f64,
        /// RNG seed for reproducibility
        #[arg(long, default_value = "42")]
        seed: u64,
        /// Output results as JSON
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    qservices_common::tracing::init_with_default_filter("info");

    let cli_args = Cli::parse();

    // XebVerify is self-contained — no AppState needed.
    if let Command::XebVerify {
        n_qubits,
        n_circuits,
        n_shots,
        n_gates,
        fidelity,
        seed,
        json,
    } = cli_args.command
    {
        return xeb::run_xeb_verify(n_qubits, n_circuits, n_shots, n_gates, fidelity, seed, json);
    }

    let state = build_app_state(&cli_args).await?;

    match cli_args.command {
        Command::Serve {
            port,
            monitor_chip,
            monitor_interval,
            no_monitor,
        } => {
            if !no_monitor {
                let chip_id = monitor_chip.unwrap_or_else(|| "lab3-qpu".to_string());
                let monitor_config = monitor::MonitorConfig {
                    quantum_api_url: cli_args.quantum_api_url.clone(),
                    pipeline_api_url: format!("http://localhost:{port}"),
                    chip_id,
                    poll_interval_secs: monitor_interval,
                    brain_path: cli_args.brain_path.clone(),
                    ..monitor::MonitorConfig::default()
                };
                tokio::spawn(monitor::run_monitor(monitor_config));
                tracing::info!("Monitor daemon started");
            }
            server::serve(state, port).await?;
        }
        Command::Run {
            template,
            params,
            follow,
            dry_run,
        } => {
            cli::run_pipeline(state, template, params, follow, dry_run).await?;
        }
        Command::Resume { run_id } => {
            cli::resume_pipeline(state, run_id).await?;
        }
        Command::Status { run_id } => {
            cli::get_status(state, run_id).await?;
        }
        Command::Result { run_id } => {
            cli::get_result(state, run_id).await?;
        }
        Command::Validate { template, file } => {
            cli::validate(state, template, file).await?;
        }
        Command::Dag { template } => {
            cli::print_dag(state, template).await?;
        }
        Command::ListTemplates => {
            cli::list_templates(state).await?;
        }
        Command::XebVerify { .. } => {
            // Already handled above before AppState construction.
            unreachable!("XebVerify dispatched before state build")
        }
    }

    Ok(())
}

async fn build_app_state(cli_args: &Cli) -> Result<server::AppState> {
    use qorchestrate_executor::{CheckpointStore, StageRegistry};
    use qorchestrate_stages::{register_meta_stages, register_standard_stages};

    let checkpoint = Arc::new(CheckpointStore::new(&cli_args.checkpoint_dir)?);
    let api_url = cli_args.quantum_api_url.clone();
    let qpu_url = cli_args.qpudidp_url.clone();
    let templates_dir = cli_args.templates_dir.clone();
    let checkpoint_for_exec = checkpoint.clone();

    // Break the `Registry -> MetaStage -> Executor -> Registry` cycle:
    // build the executor via `Arc::new_cyclic` so meta stages can hold a
    // `Weak<PipelineExecutor>` back-reference without leaking the registry.
    let executor: Arc<qorchestrate_executor::PipelineExecutor> = Arc::new_cyclic(|weak_exec| {
        let mut registry = StageRegistry::new();
        register_standard_stages(&mut registry);
        register_meta_stages(&mut registry, weak_exec.clone(), templates_dir.clone());
        qorchestrate_executor::PipelineExecutor::new(
            Arc::new(registry),
            checkpoint_for_exec,
            api_url,
            qpu_url,
        )
    });

    Ok(server::AppState {
        executor,
        checkpoint,
        templates_dir: cli_args.templates_dir.clone(),
        brain_path: cli_args.brain_path.clone(),
        runs: Arc::new(RwLock::new(HashMap::new())),
        event_channels: Arc::new(RwLock::new(HashMap::new())),
    })
}
