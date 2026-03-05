use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;

use crate::config::{
    self, AgentsConfig, Config, DevAgentConfig, MissionsConfig, ProjectConfig, ReviewConfig,
    ReviewerAgentConfig, ToolsConfig, WorkerAgentConfig,
};
use crate::error::ExitError;
use crate::subprocess::{Tool, run_command};
use crate::template::render_agents_md;

const PROJECT_TYPES: &[&str] = &["api", "cli", "frontend", "library", "monorepo", "tui"];
const AVAILABLE_TOOLS: &[&str] = &["bones", "maw", "crit", "botbus", "botty"];
const REVIEWER_ROLES: &[&str] = &["security"];
const LANGUAGES: &[&str] = &["rust", "python", "node", "go", "typescript", "java"];
const CONFIG_VERSION: &str = "1.0.16";

/// Validate that a name (project, reviewer role) matches [a-z0-9][a-z0-9-]* and is ≤64 chars.
/// Prevents command injection and path traversal via user-supplied names.
/// Infer project name from the current directory (or ws/default parent).
fn infer_project_name() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    // If we're in ws/default, go up two levels
    let dir = if cwd.ends_with("ws/default") {
        cwd.parent()?.parent()?
    } else {
        &cwd
    };
    let name = dir.file_name()?.to_str()?;
    // Lowercase and replace non-alphanumeric with hyphens
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-').to_string();
    if trimmed.is_empty() || validate_name(&trimmed, "project name").is_err() {
        return None;
    }
    Some(trimmed)
}

fn validate_name(name: &str, label: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("{label} must be 1-64 characters, got {}", name.len());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        anyhow::bail!("{label} must match [a-z0-9-], got {name:?}");
    }
    if name.starts_with('-') || name.ends_with('-') {
        anyhow::bail!("{label} must not start or end with '-', got {name:?}");
    }
    Ok(())
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project name
    #[arg(long)]
    pub name: Option<String>,
    /// Project types (comma-separated: api, cli, frontend, library, monorepo, tui)
    #[arg(long, value_delimiter = ',')]
    pub r#type: Vec<String>,
    /// Tools to enable (comma-separated: bones, maw, crit, botbus, botty)
    #[arg(long, value_delimiter = ',')]
    pub tools: Vec<String>,
    /// Reviewer roles (comma-separated: security)
    #[arg(long, value_delimiter = ',')]
    pub reviewers: Vec<String>,
    /// Languages for .gitignore generation (comma-separated: rust, python, node, go, typescript, java)
    #[arg(long, value_delimiter = ',')]
    pub language: Vec<String>,
    /// Install command (e.g., "just install")
    #[arg(long)]
    pub install_command: Option<String>,
    /// Check command run before merging (e.g., "just check", "cargo check")
    #[arg(long)]
    pub check_command: Option<String>,
    /// Non-interactive mode
    #[arg(long)]
    pub no_interactive: bool,
    /// Skip bones initialization
    #[arg(long, alias = "no-init-beads")]
    pub no_init_bones: bool,
    /// Skip seeding initial work bones
    #[arg(long)]
    pub no_seed_work: bool,
    /// Force overwrite existing config
    #[arg(long)]
    pub force: bool,
    /// Skip auto-commit
    #[arg(long)]
    pub no_commit: bool,
    /// Project root directory
    #[arg(long)]
    pub project_root: Option<PathBuf>,
}

/// Collected user choices for init
struct InitChoices {
    name: String,
    types: Vec<String>,
    tools: Vec<String>,
    reviewers: Vec<String>,
    languages: Vec<String>,
    install_command: Option<String>,
    check_command: Option<String>,
    init_bones: bool,
    seed_work: bool,
}

impl InitArgs {
    pub fn execute(&self) -> Result<()> {
        let project_dir = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current dir"));

        // Canonicalize project root and verify it contains config or is a new init target
        let project_dir = project_dir.canonicalize().unwrap_or(project_dir);

        // Detect maw v2 bare repo
        let ws_default = project_dir.join("ws/default");
        if config::find_config(&ws_default).is_some()
            || (ws_default.exists() && !project_dir.join(".agents/edict").exists())
        {
            return self.handle_bare_repo(&project_dir);
        }

        let agents_dir = project_dir.join(".agents/edict");
        let agents_md_path = project_dir.join("AGENTS.md");
        let is_reinit = agents_dir.exists();

        // Detect existing config from AGENTS.md on re-init
        let detected = if is_reinit && agents_md_path.exists() {
            let content = fs::read_to_string(&agents_md_path)?;
            detect_from_agents_md(&content)
        } else {
            DetectedConfig::default()
        };

        let interactive = !self.no_interactive && std::io::stdin().is_terminal();
        let choices = self.gather_choices(interactive, &detected)?;

        // Create .agents/edict/
        fs::create_dir_all(&agents_dir)?;
        println!("Created .agents/edict/");

        // Run sync to copy workflow docs, prompts, design docs, hooks
        // We create config first so sync can read it
        let config = build_config(&choices);

        // Write .edict.toml
        let config_path = project_dir.join(config::CONFIG_TOML);
        if !config_path.exists() || self.force {
            let toml_str = config.to_toml()?;
            fs::write(&config_path, &toml_str)?;
            println!("Generated {}", config::CONFIG_TOML);
        }

        // Copy workflow docs (reuse sync logic)
        sync_workflow_docs(&agents_dir)?;
        println!("Copied workflow docs");

        // Copy prompt templates
        sync_prompts(&agents_dir)?;
        println!("Copied prompt templates");

        // Copy design docs
        sync_design_docs(&agents_dir)?;
        println!("Copied design docs");

        // Install global agent hooks (idempotent)
        crate::commands::hooks::HooksCommand::Install {
            project_root: Some(project_dir.clone()),
        }
        .execute()
        .unwrap_or_else(|e| eprintln!("Warning: failed to install global hooks: {e}"));

        // Generate AGENTS.md
        if agents_md_path.exists() && !self.force {
            tracing::warn!(
                "AGENTS.md already exists. Use --force to overwrite, or run `edict sync` to update."
            );
        } else {
            let content = render_agents_md(&config)?;
            fs::write(&agents_md_path, content)?;
            println!("Generated AGENTS.md");
        }

        // Initialize bones
        if choices.init_bones && choices.tools.contains(&"bones".to_string()) {
            match run_command("bn", &["init"], Some(&project_dir)) {
                Ok(_) => println!("Initialized bones"),
                Err(_) => tracing::warn!("bn init failed (is bones installed?)"),
            }
        }

        // Initialize maw
        if choices.tools.contains(&"maw".to_string()) {
            match run_command("maw", &["init"], Some(&project_dir)) {
                Ok(_) => println!("Initialized maw"),
                Err(_) => tracing::warn!("maw init failed (is maw installed?)"),
            }
        }

        // Initialize crit
        if choices.tools.contains(&"crit".to_string()) {
            match run_command("crit", &["init"], Some(&project_dir)) {
                Ok(_) => println!("Initialized crit"),
                Err(_) => tracing::warn!("crit init failed (is crit installed?)"),
            }

            // Create .critignore
            let critignore_path = project_dir.join(".critignore");
            if !critignore_path.exists() {
                fs::write(
                    &critignore_path,
                    "# Ignore edict-managed files (prompts, scripts, hooks, journals)\n\
                     .agents/edict/\n\
                     \n\
                     # Ignore tool config and data files\n\
                     .crit/\n\
                     .maw.toml\n\
                     .edict.toml\n\
                     .botbox.json\n\
                     .claude/\n\
                     opencode.json\n",
                )?;
                println!("Created .critignore");
            }
        }

        // Register project on #projects channel (skip on re-init)
        if choices.tools.contains(&"botbus".to_string()) && !is_reinit {
            let abs_path = project_dir
                .canonicalize()
                .unwrap_or_else(|_| project_dir.clone());
            let tools_list = choices.tools.join(", ");
            let agent = format!("{}-dev", choices.name);
            let msg = format!(
                "project: {}  repo: {}  lead: {}  tools: {}",
                choices.name,
                abs_path.display(),
                agent,
                tools_list
            );
            match Tool::new("bus")
                .args(&[
                    "send",
                    "--agent",
                    &agent,
                    "projects",
                    &msg,
                    "-L",
                    "project-registry",
                ])
                .run()
            {
                Ok(output) if output.success() => {
                    println!("Registered project on #projects channel")
                }
                _ => tracing::warn!("failed to register on #projects (is bus installed?)"),
            }
        }

        // Seed initial work bones
        if choices.seed_work && choices.tools.contains(&"bones".to_string()) {
            let count = seed_initial_bones(&project_dir, &choices.name, &choices.types);
            if count > 0 {
                let suffix = if count > 1 { "s" } else { "" };
                println!("Created {count} seed bone{suffix}");
            }
        }

        // Register botbus hooks
        if choices.tools.contains(&"botbus".to_string()) {
            register_spawn_hooks(&project_dir, &choices.name, &choices.reviewers, &config);
        }

        // Generate .gitignore
        if !choices.languages.is_empty() {
            let gitignore_path = project_dir.join(".gitignore");
            if !gitignore_path.exists() {
                match fetch_gitignore(&choices.languages) {
                    Ok(content) => {
                        fs::write(&gitignore_path, content)?;
                        println!("Generated .gitignore for: {}", choices.languages.join(", "));
                    }
                    Err(e) => tracing::warn!("failed to generate .gitignore: {e}"),
                }
            } else {
                println!(".gitignore already exists, skipping generation");
            }
        }

        // Auto-commit
        if !is_reinit && !self.no_commit {
            auto_commit(&project_dir, &config)?;
        }

        println!("Done.");
        Ok(())
    }

    fn handle_bare_repo(&self, project_dir: &Path) -> Result<()> {
        let project_dir = project_dir
            .canonicalize()
            .context("canonicalizing project root")?;

        // Gather interactive choices HERE (where stdin is a terminal) so that
        // the inner `maw exec` invocation can run non-interactively.
        let ws_default = project_dir.join("ws/default");
        let agents_md_path = ws_default.join("AGENTS.md");
        let detected = if agents_md_path.exists() {
            let content = fs::read_to_string(&agents_md_path)?;
            detect_from_agents_md(&content)
        } else {
            DetectedConfig::default()
        };

        let interactive = !self.no_interactive && std::io::stdin().is_terminal();
        let choices = self.gather_choices(interactive, &detected)?;

        let mut args: Vec<String> = vec!["exec", "default", "--", "edict", "init"]
            .into_iter()
            .map(Into::into)
            .collect();

        // Always pass gathered choices as explicit args so inner invocation
        // doesn't need interactive input.
        args.push("--name".into());
        args.push(choices.name.clone());
        args.push("--type".into());
        args.push(choices.types.join(","));
        args.push("--tools".into());
        args.push(choices.tools.join(","));
        if !choices.reviewers.is_empty() {
            args.push("--reviewers".into());
            args.push(choices.reviewers.join(","));
        }
        if !choices.languages.is_empty() {
            args.push("--language".into());
            args.push(choices.languages.join(","));
        }
        if let Some(ref cmd) = choices.install_command {
            args.push("--install-command".into());
            args.push(cmd.clone());
        }
        if let Some(ref cmd) = choices.check_command {
            args.push("--check-command".into());
            args.push(cmd.clone());
        }
        if self.force {
            args.push("--force".into());
        }
        // Inner invocation is always non-interactive (stdin piped by maw exec)
        args.push("--no-interactive".into());
        if self.no_commit {
            args.push("--no-commit".into());
        }
        if !choices.init_bones {
            args.push("--no-init-bones".into());
        }
        if !choices.seed_work {
            args.push("--no-seed-work".into());
        }

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_command("maw", &arg_refs, Some(&project_dir))?;

        // Create bare root stubs
        let stub_content = "**Do not edit the root AGENTS.md for memories or instructions. Use the AGENTS.md in ws/default/.**\n@ws/default/AGENTS.md\n";
        let stub_agents = project_dir.join("AGENTS.md");
        if !stub_agents.exists() {
            fs::write(&stub_agents, stub_content)?;
            println!("Created bare-root AGENTS.md stub");
        }

        // Symlink .claude → ws/default/.claude
        let root_claude_dir = project_dir.join(".claude");
        let ws_claude_dir = project_dir.join("ws/default/.claude");
        if ws_claude_dir.exists() {
            let needs_symlink = match fs::read_link(&root_claude_dir) {
                Ok(target) => target != Path::new("ws/default/.claude"),
                Err(_) => true,
            };
            if needs_symlink {
                let tmp_link = project_dir.join(".claude.tmp");
                let _ = fs::remove_file(&tmp_link);
                #[cfg(unix)]
                std::os::unix::fs::symlink("ws/default/.claude", &tmp_link)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir("ws/default/.claude", &tmp_link)?;
                if let Err(e) = fs::rename(&tmp_link, &root_claude_dir) {
                    let _ = fs::remove_file(&tmp_link);
                    return Err(e).context("creating .claude symlink");
                }
                println!("Symlinked .claude → ws/default/.claude");
            }
        }

        // Symlink .pi → ws/default/.pi
        let root_pi_dir = project_dir.join(".pi");
        let ws_pi_dir = project_dir.join("ws/default/.pi");
        if ws_pi_dir.exists() {
            let needs_symlink = match fs::read_link(&root_pi_dir) {
                Ok(target) => target != Path::new("ws/default/.pi"),
                Err(_) => true,
            };
            if needs_symlink {
                let tmp_link = project_dir.join(".pi.tmp");
                let _ = fs::remove_file(&tmp_link);
                #[cfg(unix)]
                std::os::unix::fs::symlink("ws/default/.pi", &tmp_link)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir("ws/default/.pi", &tmp_link)?;
                if let Err(e) = fs::rename(&tmp_link, &root_pi_dir) {
                    let _ = fs::remove_file(&tmp_link);
                    return Err(e).context("creating .pi symlink");
                }
                println!("Symlinked .pi → ws/default/.pi");
            }
        }

        Ok(())
    }

    fn gather_choices(&self, interactive: bool, detected: &DetectedConfig) -> Result<InitChoices> {
        // Project name
        let name = if let Some(ref n) = self.name {
            validate_name(n, "project name")?;
            n.clone()
        } else if interactive {
            let n = prompt_input("Project name", detected.name.as_deref())?;
            validate_name(&n, "project name")?;
            n
        } else {
            let n = detected
                .name
                .clone()
                .or_else(|| infer_project_name())
                .ok_or_else(|| {
                    ExitError::Other("--name is required in non-interactive mode".into())
                })?;
            validate_name(&n, "project name")?;
            n
        };

        // Project types
        let types = if !self.r#type.is_empty() {
            validate_values(&self.r#type, PROJECT_TYPES, "project type")?;
            self.r#type.clone()
        } else if interactive {
            let defaults: Vec<bool> = PROJECT_TYPES
                .iter()
                .map(|t| detected.types.contains(&t.to_string()))
                .collect();
            prompt_multi_select(
                "Project type (select one or more)",
                PROJECT_TYPES,
                &defaults,
            )?
        } else {
            if detected.types.is_empty() {
                vec!["cli".to_string()]
            } else {
                detected.types.clone()
            }
        };

        // Tools
        let tools = if !self.tools.is_empty() {
            validate_values(&self.tools, AVAILABLE_TOOLS, "tool")?;
            self.tools.clone()
        } else if interactive {
            let defaults: Vec<bool> = AVAILABLE_TOOLS
                .iter()
                .map(|t| {
                    if detected.tools.is_empty() {
                        true // all enabled by default
                    } else {
                        detected.tools.contains(&t.to_string())
                    }
                })
                .collect();
            prompt_multi_select("Tools to enable", AVAILABLE_TOOLS, &defaults)?
        } else if detected.tools.is_empty() {
            AVAILABLE_TOOLS.iter().map(|s| s.to_string()).collect()
        } else {
            detected.tools.clone()
        };

        // Reviewers
        let reviewers = if !self.reviewers.is_empty() {
            validate_values(&self.reviewers, REVIEWER_ROLES, "reviewer role")?;
            for r in &self.reviewers {
                validate_name(r, "reviewer role")?;
            }
            self.reviewers.clone()
        } else if interactive {
            let defaults: Vec<bool> = REVIEWER_ROLES
                .iter()
                .map(|r| detected.reviewers.contains(&r.to_string()))
                .collect();
            prompt_multi_select("Reviewer roles", REVIEWER_ROLES, &defaults)?
        } else {
            detected.reviewers.clone()
        };

        // Languages
        let languages = if !self.language.is_empty() {
            validate_values(&self.language, LANGUAGES, "language")?;
            self.language.clone()
        } else if interactive {
            prompt_multi_select(
                "Languages/frameworks (for .gitignore generation)",
                LANGUAGES,
                &vec![false; LANGUAGES.len()],
            )?
        } else {
            Vec::new()
        };

        // Init bones
        let init_bones = if self.no_init_bones {
            false
        } else if interactive {
            prompt_confirm("Initialize bones?", true)?
        } else {
            false
        };

        // Seed work
        let seed_work = if self.no_seed_work {
            false
        } else if interactive {
            prompt_confirm("Seed initial work bones?", false)?
        } else {
            false
        };

        // Install command
        let install_command = if let Some(ref cmd) = self.install_command {
            Some(cmd.clone())
        } else if interactive {
            if prompt_confirm("Install locally after releases? (for CLI tools)", false)? {
                Some(prompt_input("Install command", Some("just install"))?)
            } else {
                None
            }
        } else {
            None
        };

        // Check command (auto-detect from language, allow override)
        let check_command = if let Some(ref cmd) = self.check_command {
            Some(cmd.clone())
        } else {
            let auto = detect_check_command(&languages);
            if interactive {
                let default = auto.as_deref().unwrap_or("just check");
                Some(prompt_input(
                    "Build/check command (run before merging)",
                    Some(default),
                )?)
            } else {
                auto
            }
        };

        Ok(InitChoices {
            name,
            types,
            tools,
            reviewers,
            languages,
            install_command,
            check_command,
            init_bones,
            seed_work,
        })
    }
}

// --- Interactive prompts using dialoguer ---

fn prompt_input(prompt: &str, default: Option<&str>) -> Result<String> {
    let mut builder = dialoguer::Input::<String>::new().with_prompt(prompt);
    if let Some(d) = default {
        builder = builder.default(d.to_string());
    }
    builder.interact_text().context("reading user input")
}

fn prompt_multi_select(prompt: &str, items: &[&str], defaults: &[bool]) -> Result<Vec<String>> {
    let selections = dialoguer::MultiSelect::new()
        .with_prompt(prompt)
        .items(items)
        .defaults(defaults)
        .interact()
        .context("reading user selection")?;

    Ok(selections
        .into_iter()
        .map(|i| items[i].to_string())
        .collect())
}

fn prompt_confirm(prompt: &str, default: bool) -> Result<bool> {
    dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact()
        .context("reading user confirmation")
}

// --- Validation ---

fn validate_values(values: &[String], valid: &[&str], label: &str) -> Result<()> {
    let invalid: Vec<&String> = values
        .iter()
        .filter(|v| !valid.contains(&v.as_str()))
        .collect();
    if !invalid.is_empty() {
        let inv = invalid
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let val = valid.join(", ");
        return Err(ExitError::Other(format!("Unknown {label}: {inv}. Valid: {val}")).into());
    }
    Ok(())
}

// --- Config detection from AGENTS.md header ---

#[derive(Debug, Default)]
struct DetectedConfig {
    name: Option<String>,
    types: Vec<String>,
    tools: Vec<String>,
    reviewers: Vec<String>,
}

fn detect_from_agents_md(content: &str) -> DetectedConfig {
    let mut config = DetectedConfig::default();

    for line in content.lines().take(20) {
        if line.starts_with("# ") && config.name.is_none() {
            config.name = Some(line[2..].trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Project type: ") {
            config.types = rest.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(rest) = line.strip_prefix("Tools: ") {
            config.tools = rest
                .split(',')
                .map(|s| s.trim().trim_matches('`').to_string())
                .collect();
        } else if let Some(rest) = line.strip_prefix("Reviewer roles: ") {
            config.reviewers = rest.split(',').map(|s| s.trim().to_string()).collect();
        }
    }

    config
}

// --- Language detection ---

/// Auto-detect the appropriate check command from the selected languages.
fn detect_check_command(languages: &[String]) -> Option<String> {
    for lang in languages {
        match lang.as_str() {
            "rust" => return Some("cargo clippy && cargo test".to_string()),
            "typescript" | "node" => return Some("npm run build && npm test".to_string()),
            "python" => return Some("python -m pytest".to_string()),
            "go" => return Some("go vet ./... && go test ./...".to_string()),
            "java" => return Some("mvn verify".to_string()),
            _ => {}
        }
    }
    None
}

// --- Config building ---

fn build_config(choices: &InitChoices) -> Config {
    Config {
        version: CONFIG_VERSION.to_string(),
        project: ProjectConfig {
            name: choices.name.clone(),
            project_type: choices.types.clone(),
            languages: choices.languages.clone(),
            default_agent: Some(format!("{}-dev", choices.name)),
            channel: Some(choices.name.clone()),
            install_command: choices.install_command.clone(),
            check_command: choices.check_command.clone(),
            critical_approvers: None,
        },
        tools: ToolsConfig {
            bones: choices.tools.contains(&"bones".to_string()),
            maw: choices.tools.contains(&"maw".to_string()),
            crit: choices.tools.contains(&"crit".to_string()),
            botbus: choices.tools.contains(&"botbus".to_string()),
            botty: choices.tools.contains(&"botty".to_string()),
        },
        review: ReviewConfig {
            enabled: !choices.reviewers.is_empty(),
            reviewers: choices.reviewers.clone(),
        },
        push_main: false,
        agents: AgentsConfig {
            dev: Some(DevAgentConfig {
                model: "strong".into(),
                max_loops: 100,
                pause: 2,
                timeout: 1800,
                missions: Some(MissionsConfig {
                    enabled: true,
                    max_workers: 4,
                    max_children: 12,
                    checkpoint_interval_sec: 30,
                }),
                multi_lead: None,
                memory_limit: None,
            }),
            worker: Some(WorkerAgentConfig {
                model: "fast".into(),
                timeout: 900,
                memory_limit: None,
            }),
            reviewer: Some(ReviewerAgentConfig {
                model: "strong".into(),
                max_loops: 100,
                pause: 2,
                timeout: 900,
                memory_limit: None,
            }),
            responder: None,
        },
        models: Default::default(),
        env: build_default_env(&choices.languages),
    }
}

/// Build default [env] vars based on project languages.
fn build_default_env(languages: &[String]) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    let has_rust = languages.iter().any(|l| l.eq_ignore_ascii_case("rust"));
    if has_rust {
        env.insert("CARGO_BUILD_JOBS".into(), "2".into());
        env.insert("RUSTC_WRAPPER".into(), "sccache".into());
        env.insert("SCCACHE_DIR".into(), "$HOME/.cache/sccache".into());
    }
    env
}

// --- Sync helpers (reuse embedded content from sync.rs) ---

// Re-embed the same workflow docs as sync.rs
use crate::commands::sync::{DESIGN_DOCS, REVIEWER_PROMPTS, WORKFLOW_DOCS};

fn sync_workflow_docs(agents_dir: &Path) -> Result<()> {
    for (name, content) in WORKFLOW_DOCS {
        let path = agents_dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }

    // Write version marker
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, content) in WORKFLOW_DOCS {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    let version = format!("{:x}", hasher.finalize());
    fs::write(agents_dir.join(".version"), &version[..32])?;

    Ok(())
}

fn sync_prompts(agents_dir: &Path) -> Result<()> {
    let prompts_dir = agents_dir.join("prompts");
    fs::create_dir_all(&prompts_dir)?;

    for (name, content) in REVIEWER_PROMPTS {
        let path = prompts_dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, content) in REVIEWER_PROMPTS {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    let version = format!("{:x}", hasher.finalize());
    fs::write(prompts_dir.join(".prompts-version"), &version[..32])?;

    Ok(())
}

fn sync_design_docs(agents_dir: &Path) -> Result<()> {
    let design_dir = agents_dir.join("design");
    fs::create_dir_all(&design_dir)?;

    for (name, content) in DESIGN_DOCS {
        let path = design_dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, content) in DESIGN_DOCS {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    let version = format!("{:x}", hasher.finalize());
    fs::write(design_dir.join(".design-docs-version"), &version[..32])?;

    Ok(())
}

// sync_hooks removed — hooks are now installed globally via `edict hooks install`

// --- Hook registration ---

fn register_spawn_hooks(project_dir: &Path, name: &str, reviewers: &[String], config: &Config) {
    let abs_path = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let agent = format!("{name}-dev");

    // Detect maw v2 workspace context
    let (hook_cwd, spawn_cwd) = detect_hook_paths(&abs_path);

    // Check if bus supports hooks
    if Tool::new("bus").arg("hooks").arg("list").run().is_err() {
        return;
    }

    // Register router hook
    let responder_memory_limit = config
        .agents
        .responder
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());
    register_router_hook(&hook_cwd, &spawn_cwd, name, &agent, responder_memory_limit);

    // Register reviewer hooks
    let reviewer_memory_limit = config
        .agents
        .reviewer
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());
    for role in reviewers {
        let reviewer_agent = format!("{name}-{role}");
        register_reviewer_hook(&hook_cwd, &spawn_cwd, name, &agent, &reviewer_agent, reviewer_memory_limit);
    }
}

fn detect_hook_paths(abs_path: &Path) -> (String, String) {
    // In maw v2, if we're inside ws/default/, use the bare root
    let abs_str = abs_path.display().to_string();
    if let Some(parent) = abs_path.parent()
        && parent.file_name().is_some_and(|n| n == "ws")
        && let Some(bare_root) = parent.parent()
        && bare_root.join(".manifold").exists()
    {
        let bare_str = bare_root.display().to_string();
        return (bare_str.clone(), bare_str);
    }
    (abs_str.clone(), abs_str)
}

pub(super) fn register_router_hook(
    hook_cwd: &str,
    spawn_cwd: &str,
    name: &str,
    agent: &str,
    memory_limit: Option<&str>,
) {
    let env_inherit = "BOTBUS_CHANNEL,BOTBUS_MESSAGE_ID,BOTBUS_HOOK_ID,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";
    let claim_uri = format!("agent://{name}-dev");
    let spawn_name = format!("{name}-responder");
    let description = format!("edict:{name}:responder");

    let mut args: Vec<&str> = vec![
        "--agent",
        agent,
        "--channel",
        name,
        "--claim",
        &claim_uri,
        "--claim-owner",
        agent,
        "--cwd",
        hook_cwd,
        "--ttl",
        "600",
        "--",
        "botty",
        "spawn",
        "--env-inherit",
        env_inherit,
    ];
    if let Some(limit) = memory_limit {
        args.push("--memory-limit");
        args.push(limit);
    }
    args.extend_from_slice(&[
        "--name",
        &spawn_name,
        "--cwd",
        spawn_cwd,
        "--",
        "edict",
        "run",
        "responder",
    ]);

    match crate::subprocess::ensure_bus_hook(&description, &args) {
        Ok((action, _id)) => println!("Router hook {action} for #{name}"),
        Err(e) => eprintln!("Warning: Failed to register router hook: {e}"),
    }
}

pub(super) fn register_reviewer_hook(
    hook_cwd: &str,
    spawn_cwd: &str,
    name: &str,
    agent: &str,
    reviewer_agent: &str,
    memory_limit: Option<&str>,
) {
    let env_inherit = "BOTBUS_CHANNEL,BOTBUS_MESSAGE_ID,BOTBUS_HOOK_ID,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";
    let claim_uri = format!("agent://{reviewer_agent}");
    // Extract role suffix from reviewer_agent (e.g., "myproject-security" → "security")
    let role = reviewer_agent
        .strip_prefix(&format!("{name}-"))
        .unwrap_or(reviewer_agent);
    let description = format!("edict:{name}:reviewer-{role}");

    let mut args: Vec<&str> = vec![
        "--agent",
        agent,
        "--channel",
        name,
        "--mention",
        reviewer_agent,
        "--claim",
        &claim_uri,
        "--claim-owner",
        reviewer_agent,
        "--ttl",
        "600",
        "--priority",
        "1",
        "--cwd",
        hook_cwd,
        "--",
        "botty",
        "spawn",
        "--env-inherit",
        env_inherit,
    ];
    if let Some(limit) = memory_limit {
        args.push("--memory-limit");
        args.push(limit);
    }
    args.extend_from_slice(&[
        "--name",
        reviewer_agent,
        "--cwd",
        spawn_cwd,
        "--",
        "edict",
        "run",
        "reviewer-loop",
        "--agent",
        reviewer_agent,
    ]);

    match crate::subprocess::ensure_bus_hook(&description, &args) {
        Ok((action, _id)) => println!("Reviewer hook for @{reviewer_agent} {action}"),
        Err(e) => eprintln!("Warning: Failed to register mention hook for @{reviewer_agent}: {e}"),
    }
}

// --- Seed bones ---

fn seed_initial_bones(project_dir: &Path, _name: &str, types: &[String]) -> usize {
    let mut count = 0;

    let create_bone = |title: &str, description: &str, urgency: &str| -> bool {
        Tool::new("bn")
            .args(&[
                "create",
                &format!("--title={title}"),
                &format!("--description={description}"),
                "--kind=task",
                &format!("--urgency={urgency}"),
            ])
            .run()
            .is_ok_and(|o| o.success())
    };

    // Scout for spec files
    for spec in ["spec.md", "SPEC.md", "specification.md", "design.md"] {
        if project_dir.join(spec).exists()
            && create_bone(
                &format!("Review {spec} and create implementation bones"),
                &format!(
                    "Read {spec}, understand requirements, and break down into actionable bones with acceptance criteria."
                ),
                "urgent",
            )
        {
            count += 1;
        }
    }

    // Scout for README
    if project_dir.join("README.md").exists()
        && create_bone(
            "Review README and align project setup",
            "Read README.md for project goals, architecture decisions, and setup requirements. Create bones for any gaps.",
            "default",
        )
    {
        count += 1;
    }

    // Scout for source structure
    if !project_dir.join("src").exists()
        && create_bone(
            "Create initial source structure",
            &format!(
                "Set up src/ directory and project scaffolding for project type: {}.",
                types.join(", ")
            ),
            "default",
        )
    {
        count += 1;
    }

    // Fallback
    if count == 0
        && create_bone(
            "Scout project and create initial bones",
            "Explore the repository, understand the project goals, and create actionable bones for initial implementation work.",
            "urgent",
        )
    {
        count += 1;
    }

    count
}

// --- .gitignore ---

fn fetch_gitignore(languages: &[String]) -> Result<String> {
    // Validate all language names against the allowlist before constructing the URL
    // to prevent SSRF via crafted language names (e.g., "../admin" or URL fragments)
    for lang in languages {
        if !LANGUAGES.contains(&lang.as_str()) {
            anyhow::bail!("unknown language for .gitignore: {lang:?}. Valid: {LANGUAGES:?}");
        }
    }
    let langs = languages.join(",");
    let url = format!("https://www.toptal.com/developers/gitignore/api/{langs}");
    let body = ureq::get(&url).call()?.into_body().read_to_string()?;
    Ok(body)
}

// --- Auto-commit ---

fn auto_commit(project_dir: &Path, config: &Config) -> Result<()> {
    let message = format!("chore: initialize edict v{}", config.version);

    // Try git first, fall back to jj for legacy projects
    if project_dir.join(".git").exists()
        || project_dir
            .ancestors()
            .any(|p| p.join(".git").exists() || p.join("repo.git").exists())
    {
        let _ = run_command("git", &["add", "-A"], Some(project_dir));
        match run_command("git", &["commit", "-m", &message], Some(project_dir)) {
            Ok(_) => println!("Committed: {message}"),
            Err(_) => eprintln!("Warning: Failed to auto-commit (git error)"),
        }
    } else if project_dir.join(".jj").exists() {
        match run_command("jj", &["describe", "-m", &message], Some(project_dir)) {
            Ok(_) => println!("Committed: {message}"),
            Err(_) => eprintln!("Warning: Failed to auto-commit (jj error)"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_from_agents_md() {
        let content = "# myproject\n\nProject type: cli, api\nTools: `bones`, `maw`, `crit`\nReviewer roles: security\n";
        let detected = detect_from_agents_md(content);
        assert_eq!(detected.name, Some("myproject".to_string()));
        assert_eq!(detected.types, vec!["cli", "api"]);
        assert_eq!(detected.tools, vec!["bones", "maw", "crit"]);
        assert_eq!(detected.reviewers, vec!["security"]);
    }

    #[test]
    fn test_detect_from_empty_agents_md() {
        let detected = detect_from_agents_md("");
        assert!(detected.name.is_none());
        assert!(detected.types.is_empty());
        assert!(detected.tools.is_empty());
        assert!(detected.reviewers.is_empty());
    }

    #[test]
    fn test_validate_values_ok() {
        let values = vec!["bones".to_string(), "maw".to_string()];
        assert!(validate_values(&values, AVAILABLE_TOOLS, "tool").is_ok());
    }

    #[test]
    fn test_validate_values_invalid() {
        let values = vec!["bones".to_string(), "invalid".to_string()];
        let result = validate_values(&values, AVAILABLE_TOOLS, "tool");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn test_build_config() {
        let choices = InitChoices {
            name: "test".to_string(),
            types: vec!["cli".to_string()],
            tools: vec!["bones".to_string(), "maw".to_string()],
            reviewers: vec!["security".to_string()],
            languages: vec!["rust".to_string()],
            install_command: Some("just install".to_string()),
            check_command: Some("cargo test && cargo clippy -- -D warnings".to_string()),
            init_bones: true,
            seed_work: false,
        };

        let config = build_config(&choices);
        assert_eq!(config.project.name, "test");
        assert_eq!(config.project.default_agent, Some("test-dev".to_string()));
        assert_eq!(config.project.channel, Some("test".to_string()));
        assert!(config.tools.bones);
        assert!(config.tools.maw);
        assert!(!config.tools.crit);
        assert!(config.review.enabled);
        assert_eq!(config.review.reviewers, vec!["security"]);
        assert_eq!(
            config.project.install_command,
            Some("just install".to_string())
        );
        assert_eq!(config.project.languages, vec!["rust"]);

        let dev = config.agents.dev.unwrap();
        assert_eq!(dev.model, "strong");
        assert_eq!(dev.max_loops, 100);
        assert!(dev.missions.is_some());

        // Rust project should seed [env] with cargo/sccache defaults
        assert_eq!(config.env.get("CARGO_BUILD_JOBS").unwrap(), "2");
        assert_eq!(config.env.get("RUSTC_WRAPPER").unwrap(), "sccache");
        assert_eq!(
            config.env.get("SCCACHE_DIR").unwrap(),
            "$HOME/.cache/sccache"
        );
    }

    #[test]
    fn test_build_config_no_env_for_non_rust() {
        let choices = InitChoices {
            name: "jsapp".to_string(),
            types: vec!["web".to_string()],
            tools: vec!["bones".to_string()],
            reviewers: vec![],
            languages: vec!["typescript".to_string()],
            install_command: None,
            check_command: None,
            init_bones: false,
            seed_work: false,
        };
        let config = build_config(&choices);
        assert!(config.env.is_empty());
    }

    #[test]
    fn test_config_version_matches() {
        // Ensure CONFIG_VERSION is a valid semver-ish string
        assert!(CONFIG_VERSION.starts_with("1.0."));
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("myproject", "test").is_ok());
        assert!(validate_name("my-project", "test").is_ok());
        assert!(validate_name("project123", "test").is_ok());
        assert!(validate_name("a", "test").is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        assert!(validate_name("", "test").is_err()); // empty
        assert!(validate_name("-starts-dash", "test").is_err()); // leading dash
        assert!(validate_name("ends-dash-", "test").is_err()); // trailing dash
        assert!(validate_name("Has Uppercase", "test").is_err()); // uppercase
        assert!(validate_name("has space", "test").is_err()); // space
        assert!(validate_name("path/../traversal", "test").is_err()); // path chars
        assert!(validate_name("a;rm -rf /", "test").is_err()); // injection
        assert!(validate_name(&"a".repeat(65), "test").is_err()); // too long
    }

    #[test]
    fn test_fetch_gitignore_validates_languages() {
        // Unknown language should be rejected before URL construction
        let result = fetch_gitignore(&["malicious/../../etc".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown language"));
    }
}
