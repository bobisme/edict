pub mod adapters;
pub mod cleanup;
pub mod context;
pub mod executor;
pub mod exit_policy;
pub mod finish;
pub mod merge;
pub mod render;
pub mod resume;
pub mod review;
pub mod review_gate;
pub mod shell;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context;
use clap::Subcommand;

use super::doctor::OutputFormat;
use crate::config::Config;

/// Shared flags for all protocol subcommands.
#[derive(Debug, clap::Args)]
pub struct ProtocolArgs {
    /// Agent name (default: $AGENT or config defaultAgent)
    #[arg(long)]
    pub agent: Option<String>,
    /// Project name (default: from .botbox.toml)
    #[arg(long)]
    pub project: Option<String>,
    /// Project root directory
    #[arg(long)]
    pub project_root: Option<PathBuf>,
    /// Output format
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

impl ProtocolArgs {
    /// Resolve the effective agent name from flag, env, or config.
    pub fn resolve_agent(&self, config: &crate::config::Config) -> String {
        if let Some(ref agent) = self.agent {
            return agent.clone();
        }
        if let Ok(agent) = std::env::var("AGENT") {
            return agent;
        }
        if let Ok(agent) = std::env::var("BOTBUS_AGENT") {
            return agent;
        }
        config.default_agent()
    }

    /// Resolve the effective project name from flag or config.
    pub fn resolve_project(&self, config: &crate::config::Config) -> String {
        if let Some(ref project) = self.project {
            return project.clone();
        }
        config.project.name.clone()
    }

    /// Resolve the effective output format from flag or TTY detection.
    pub fn resolve_format(&self) -> OutputFormat {
        self.format.unwrap_or_else(|| {
            if std::io::stdout().is_terminal() {
                OutputFormat::Pretty
            } else {
                OutputFormat::Text
            }
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum ProtocolCommand {
    /// Check state and output commands to start working on a bone
    Start {
        /// Bone ID to start working on
        bone_id: String,
        /// Omit bus send announcement (for dispatched workers)
        #[arg(long)]
        dispatched: bool,
        /// Execute the steps immediately instead of outputting guidance
        #[arg(long)]
        execute: bool,
        #[command(flatten)]
        args: ProtocolArgs,
    },
    /// Check state and output commands to finish a bone
    Finish {
        /// Bone ID to finish
        bone_id: String,
        /// Omit maw ws merge step (for dispatched workers whose lead handles merge)
        #[arg(long)]
        no_merge: bool,
        /// Output finish commands even without review approval
        #[arg(long)]
        force: bool,
        /// Execute finish commands directly instead of outputting them
        #[arg(long)]
        execute: bool,
        #[command(flatten)]
        args: ProtocolArgs,
    },
    /// Check state and output commands to request review
    Review {
        /// Bone ID to review
        bone_id: String,
        /// Override reviewer list (comma-separated)
        #[arg(long)]
        reviewers: Option<String>,
        /// Reference an existing review ID (skip creation)
        #[arg(long)]
        review_id: Option<String>,
        /// Execute the review commands instead of just outputting them
        #[arg(long)]
        execute: bool,
        #[command(flatten)]
        args: ProtocolArgs,
    },
    /// Check for held resources and output cleanup commands
    Cleanup {
        /// Execute cleanup steps instead of outputting them
        #[arg(long)]
        execute: bool,
        #[command(flatten)]
        args: ProtocolArgs,
    },
    /// Check for in-progress work from a previous session
    Resume {
        #[command(flatten)]
        args: ProtocolArgs,
    },
    /// Check preconditions and output commands to merge a worker's completed workspace
    Merge {
        /// Workspace name to merge
        workspace: String,
        /// Commit message for the merge (e.g. "feat: add login flow"). Use conventional commit
        /// prefix: feat:, fix:, chore:, etc. Required; opens $EDITOR on TTY if omitted.
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Merge even if bone is not closed or review is not approved
        #[arg(long)]
        force: bool,
        /// Execute merge commands directly instead of outputting them
        #[arg(long)]
        execute: bool,
        #[command(flatten)]
        args: ProtocolArgs,
    },
}

impl ProtocolCommand {
    pub fn execute(&self) -> anyhow::Result<()> {
        match self {
            ProtocolCommand::Start {
                bone_id,
                dispatched,
                execute,
                args,
            } => Self::execute_start(bone_id, *dispatched, *execute, args),
            ProtocolCommand::Finish {
                bone_id,
                no_merge,
                force,
                execute,
                args,
            } => {
                let project_root = match args.project_root.clone() {
                    Some(p) => p,
                    None => {
                        std::env::current_dir().context("could not determine current directory")?
                    }
                };

                let (config_path, _) = crate::config::find_config_in_project(&project_root)?;
                let config = Config::load(&config_path)?;

                let project = args.resolve_project(&config);
                let agent = args.resolve_agent(&config);
                let format = args.resolve_format();

                finish::execute(
                    bone_id, *no_merge, *force, *execute, &agent, &project, &config, format,
                )
            }
            ProtocolCommand::Review {
                bone_id,
                reviewers,
                review_id,
                execute,
                args,
            } => {
                let project_root = match args.project_root.clone() {
                    Some(p) => p,
                    None => {
                        std::env::current_dir().context("could not determine current directory")?
                    }
                };

                let (config_path, _) = crate::config::find_config_in_project(&project_root)?;
                let config = Config::load(&config_path)?;

                let agent = args.resolve_agent(&config);
                let project = args.resolve_project(&config);
                let format = args.resolve_format();

                review::execute(
                    bone_id,
                    reviewers.as_deref(),
                    review_id.as_deref(),
                    *execute,
                    &agent,
                    &project,
                    &config,
                    format,
                )
            }
            ProtocolCommand::Cleanup { execute, args } => {
                let project_root = match args.project_root.clone() {
                    Some(p) => p,
                    None => {
                        std::env::current_dir().context("could not determine current directory")?
                    }
                };

                let (config_path, _) = crate::config::find_config_in_project(&project_root)?;
                let config = crate::config::Config::load(&config_path)?;

                let agent = args.resolve_agent(&config);
                let project = args.resolve_project(&config);
                let format = args.resolve_format();
                cleanup::execute(*execute, &agent, &project, format)
            }
            ProtocolCommand::Merge {
                workspace,
                message,
                force,
                execute,
                args,
            } => {
                let project_root = match args.project_root.clone() {
                    Some(p) => p,
                    None => {
                        std::env::current_dir().context("could not determine current directory")?
                    }
                };

                let (config_path, _) = crate::config::find_config_in_project(&project_root)?;
                let config = Config::load(&config_path)?;

                let project = args.resolve_project(&config);
                let agent = args.resolve_agent(&config);
                let format = args.resolve_format();

                let resolved_message = merge::resolve_message(message.as_deref())?;

                merge::execute(
                    workspace,
                    &resolved_message,
                    *force,
                    *execute,
                    &agent,
                    &project,
                    &config,
                    format,
                )
            }
            ProtocolCommand::Resume { args } => {
                let project_root = match args.project_root.clone() {
                    Some(p) => p,
                    None => {
                        std::env::current_dir().context("could not determine current directory")?
                    }
                };

                let (config_path, _) = crate::config::find_config_in_project(&project_root)?;
                let config = crate::config::Config::load(&config_path)?;

                let agent = args.resolve_agent(&config);
                let project = args.resolve_project(&config);
                let format = args.resolve_format();
                resume::execute(&agent, &project, &config, format)
            }
        }
    }

    /// Execute the `botbox protocol start <bone-id>` command.
    ///
    /// Analyzes bone status and outputs shell commands to start work.
    /// All status outcomes (ready, blocked, resumable) exit 0 with status in stdout.
    /// Operational failures (config missing, tool unavailable) exit 1 via ProtocolExitError.
    ///
    /// If `execute` is true and status is Ready, runs the steps directly via the executor.
    fn execute_start(
        bone_id: &str,
        dispatched: bool,
        execute: bool,
        args: &ProtocolArgs,
    ) -> anyhow::Result<()> {
        // Determine project root and load config
        let project_root = match args.project_root.clone() {
            Some(p) => p,
            None => std::env::current_dir().context("could not determine current directory")?,
        };

        let config = match crate::config::find_config_in_project(&project_root) {
            Ok((config_path, _)) => Config::load(&config_path)?,
            Err(_) => {
                return Err(exit_policy::ProtocolExitError::operational(
                    "start",
                    format!(
                        "no .botbox.toml or .botbox.json found in {} or {}/ws/default",
                        project_root.display(),
                        project_root.display()
                    ),
                )
                .into_exit_error()
                .into());
            }
        };

        let project = args.resolve_project(&config);
        let agent = args.resolve_agent(&config);
        let format = args.resolve_format();

        // Collect state from bus and maw
        let ctx = context::ProtocolContext::collect(&project, &agent)?;

        // Check if bone exists and get its status
        let bone_info = match ctx.bone_status(bone_id) {
            Ok(bone) => bone,
            Err(_) => {
                let mut guidance = render::ProtocolGuidance::new("start");
                guidance.blocked(format!(
                    "bone {} not found. Check the ID with: maw exec default -- bn show {}",
                    bone_id, bone_id
                ));
                return exit_policy::render_guidance(&guidance, format);
            }
        };

        let mut guidance = render::ProtocolGuidance::new("start");
        guidance.bone = Some(render::BoneRef {
            id: bone_id.to_string(),
            title: bone_info.title.clone(),
        });

        // Status check: is bone done?
        if bone_info.state == "done" {
            guidance.blocked("bone is already done".to_string());
            return exit_policy::render_guidance(&guidance, format);
        }

        // Check for claim conflicts
        match ctx.check_bone_claim_conflict(bone_id) {
            Ok(Some(other_agent)) => {
                guidance.blocked(format!("bone already claimed by agent '{}'", other_agent));
                guidance.diagnostic(
                    "Check current claims with: bus claims list --format json".to_string(),
                );
                return exit_policy::render_guidance(&guidance, format);
            }
            Err(e) => {
                guidance.blocked(format!("failed to check claim conflict: {}", e));
                return exit_policy::render_guidance(&guidance, format);
            }
            Ok(None) => {
                // No conflict, proceed
            }
        }

        // Check if agent already holds a bone claim for this ID
        let held_workspace = ctx.workspace_for_bone(bone_id);

        if let Some(ws_name) = held_workspace {
            // RESUMABLE: agent already has this bone and workspace
            guidance.status = render::ProtocolStatus::Resumable;
            guidance.workspace = Some(ws_name.to_string());
            guidance.advise(format!(
                "Resume work in workspace {} with: botbox protocol resume",
                ws_name
            ));
            return exit_policy::render_guidance(&guidance, format);
        }

        // READY: generate start commands
        guidance.status = render::ProtocolStatus::Ready;

        // Build command steps: claim, create workspace, announce
        let mut steps = Vec::new();

        // 1. Stake bone claim
        steps.push(shell::claims_stake_cmd(
            &agent,
            &format!("bone://{}/{}", project, bone_id),
            bone_id,
        ));

        // 2. Create workspace
        steps.push(shell::ws_create_cmd());

        // 3. Capture workspace name (comment for human)
        steps.push(
            "# Capture workspace name from output above, then stake workspace claim:".to_string(),
        );

        // 4. Stake workspace claim (template with $WS placeholder - $WS is runtime-resolved)
        steps.push(shell::claims_stake_cmd(
            &agent,
            &format!("workspace://{}/$WS", project),
            bone_id,
        ));

        // 5. Update bone status
        steps.push(shell::bn_do_cmd(bone_id));

        // 6. Comment bone with workspace info
        steps.push(shell::bn_comment_cmd(bone_id, "Started in workspace $WS"));

        // 7. Announce on bus (unless --dispatched)
        if !dispatched {
            steps.push(shell::bus_send_cmd(
                &agent,
                &project,
                &format!("Working on {}: {}", bone_id, &bone_info.title),
                "task-claim",
            ));
        }

        guidance.steps(steps);
        guidance.advise(
            "Stake bone claim first, then create workspace, stake workspace claim, update bone status, and announce on bus.".to_string()
        );

        // If --execute is set and status is Ready, execute the steps
        if execute && guidance.status == render::ProtocolStatus::Ready {
            let report = executor::execute_steps(&guidance.steps)
                .map_err(|e| anyhow::anyhow!("step execution failed: {}", e))?;

            let output = executor::render_report(&report, format);
            println!("{}", output);

            // Return error if any step failed
            if !report.remaining.is_empty() || report.results.iter().any(|r| !r.success) {
                return Err(exit_policy::ProtocolExitError::operational(
                    "start",
                    "one or more steps failed during execution".to_string(),
                )
                .into_exit_error()
                .into());
            }

            Ok(())
        } else {
            // Otherwise, render guidance as usual
            exit_policy::render_guidance(&guidance, format)
        }
    }
}
