use std::path::PathBuf;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// Run an agent with stream output parsing (auto-selects runner from model provider)
    Agent {
        /// Prompt to send to the agent
        prompt: String,
        /// Model to use (tier name: fast/balanced/strong, or provider/model-id e.g. anthropic/claude-sonnet-4-6:medium)
        #[arg(short, long)]
        model: Option<String>,
        /// Timeout in seconds
        #[arg(short, long, default_value = "600")]
        timeout: u64,
        /// Output format (pretty or text)
        #[arg(long)]
        format: Option<String>,
        /// Runtime: 'auto' (default, picks runner from model provider), 'pi', or 'claude'
        #[arg(long, default_value = "auto")]
        runner: String,
        /// Skip Claude Code permission checks (only for --runner claude)
        #[arg(long)]
        skip_permissions: bool,
    },
    /// Run the dev-loop (lead agent)
    DevLoop {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Agent name override
        #[arg(long)]
        agent: Option<String>,
        /// Model to use
        #[arg(long)]
        model: Option<String>,
    },
    /// Run the worker-loop (agent-loop)
    WorkerLoop {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Agent name override
        #[arg(long)]
        agent: Option<String>,
        /// Model to use
        #[arg(long)]
        model: Option<String>,
    },
    /// Run the reviewer-loop
    ReviewerLoop {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Agent name override
        #[arg(long)]
        agent: Option<String>,
        /// Model to use
        #[arg(long)]
        model: Option<String>,
    },
    /// Run the responder (message router)
    Responder {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Agent name override
        #[arg(long)]
        agent: Option<String>,
        /// Model to use
        #[arg(long)]
        model: Option<String>,
    },
    /// Run triage (bone scoring and recommendations)
    Triage {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
    },
    /// Run iteration-start (combined status snapshot)
    IterationStart {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Agent name override
        #[arg(long)]
        agent: Option<String>,
    },
}

impl RunCommand {
    /// Execute the selected run subcommand.
    ///
    /// # Errors
    ///
    /// Returns an error if the dispatched subcommand fails.
    pub fn execute(&self) -> anyhow::Result<()> {
        match self {
            Self::Agent {
                prompt,
                model,
                timeout,
                format,
                runner,
                skip_permissions,
            } => crate::commands::run_agent::run_agent(
                runner,
                prompt,
                model.as_deref(),
                *timeout,
                format.as_deref(),
                *skip_permissions,
            ),
            Self::DevLoop {
                project_root,
                agent,
                model,
            } => crate::commands::dev_loop::run(
                project_root.as_deref(),
                agent.as_deref(),
                model.as_deref(),
            ),
            Self::WorkerLoop {
                project_root,
                agent,
                model,
            } => crate::commands::worker_loop::run_worker_loop(
                project_root.clone(),
                agent.clone(),
                model.clone(),
            ),
            Self::ReviewerLoop {
                project_root,
                agent,
                model,
            } => crate::commands::run_reviewer_loop::run_reviewer_loop(
                project_root.clone(),
                agent.clone(),
                model.clone(),
            ),
            Self::Responder {
                project_root,
                agent,
                model,
            } => crate::commands::responder::run_responder(
                project_root.clone(),
                agent.clone(),
                model.clone(),
            ),
            Self::Triage { .. } => crate::commands::triage::run_triage(),
            Self::IterationStart { agent, .. } => {
                crate::commands::iteration_start::run_iteration_start(agent.as_deref())
            }
        }
    }
}
