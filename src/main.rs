mod commands;
mod config;
mod error;
mod hooks;
mod subprocess;
mod telemetry;
mod template;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use commands::doctor::DoctorArgs;
use commands::hooks::HooksCommand;
use commands::init::InitArgs;
use commands::protocol::ProtocolCommand;
use commands::run::RunCommand;
use commands::status::StatusArgs;
use commands::sync::SyncArgs;

#[derive(Debug, Parser)]
#[command(
    name = "edict",
    version,
    about = "Setup and sync tool for multi-agent workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run agent loops (dev, worker, reviewer, responder, triage, iteration-start)
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Sync docs, scripts, hooks, and config to a project
    Sync(SyncArgs),
    /// Initialize a new edict project
    Init(InitArgs),
    /// Validate project config and companion tools
    Doctor(DoctorArgs),
    /// Show project status
    Status(StatusArgs),
    /// Manage hooks (install, audit)
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    /// Check protocol state and output guidance commands
    Protocol {
        #[command(subcommand)]
        command: ProtocolCommand,
    },
    /// Run triage (bone scoring and recommendations)
    Triage,
    /// Print the JSON Schema for .edict.toml
    Schema,
}

impl Commands {
    const fn name(&self) -> &'static str {
        match self {
            Self::Run { .. } => "run",
            Self::Sync(_) => "sync",
            Self::Init(_) => "init",
            Self::Doctor(_) => "doctor",
            Self::Status(_) => "status",
            Self::Hooks { .. } => "hooks",
            Self::Protocol { .. } => "protocol",
            Self::Triage => "triage",
            Self::Schema => "schema",
        }
    }
}

fn main() -> ExitCode {
    let _telemetry = telemetry::init();

    let cli = Cli::parse();

    let _span = tracing::info_span!("command", name = cli.command.name()).entered();

    let result = match cli.command {
        Commands::Run { command } => command.execute(),
        Commands::Sync(args) => args.execute(),
        Commands::Init(args) => args.execute(),
        Commands::Doctor(args) => args.execute(),
        Commands::Status(args) => args.execute(),
        Commands::Hooks { command } => command.execute(),
        Commands::Protocol { command } => command.execute(),
        Commands::Triage => commands::triage::run_triage(),
        Commands::Schema => commands::schema::run_schema(),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if let Some(exit_err) = e.downcast_ref::<error::ExitError>() {
                eprintln!("error: {exit_err}");
                exit_err.exit_code()
            } else {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        }
    }
}
