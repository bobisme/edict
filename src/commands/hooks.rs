use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;

use crate::config::Config;
use crate::error::ExitError;
use crate::hooks::HookRegistry;
use crate::subprocess::run_command;

pub(crate) const PI_EDICT_HOOKS_EXTENSION: &str =
    include_str!("../templates/extensions/edict-hooks.ts");

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    /// Install/update global agent hooks in ~/.claude/settings.json
    Install {
        /// Project root directory (for botbus hook registration only)
        #[arg(long)]
        project_root: Option<PathBuf>,
    },
    /// Remove global agent hooks from ~/.claude/settings.json
    Uninstall,
    /// Audit hook registrations and report issues
    Audit {
        /// Project root directory
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Output format
        #[arg(long, value_enum, default_value_t = super::doctor::OutputFormat::Pretty)]
        format: super::doctor::OutputFormat,
    },
    /// Run a hook directly (called by Claude Code / Pi hooks infrastructure)
    Run {
        /// Hook name (session-start, post-tool-call, session-end)
        hook_name: String,
        /// Project root directory (deprecated, ignored — hooks auto-detect context)
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Release claims (for Pi session shutdown)
        #[arg(long)]
        release: bool,
    },
}

impl HooksCommand {
    pub fn execute(&self) -> anyhow::Result<()> {
        match self {
            HooksCommand::Install { project_root } => install_hooks(project_root.as_deref()),
            HooksCommand::Uninstall => uninstall_hooks(),
            HooksCommand::Audit {
                project_root,
                format,
            } => audit_hooks(project_root.as_deref(), *format),
            HooksCommand::Run {
                hook_name,
                release,
                ..
            } => run_hook(hook_name, *release),
        }
    }
}

/// Install global agent hooks into ~/.claude/settings.json (and Pi extensions).
///
/// If project_root is provided, also registers botbus hooks (router + reviewers).
fn install_hooks(project_root: Option<&Path>) -> Result<()> {
    // Install global Claude Code hooks
    let home = dirs::home_dir().context("could not determine home directory")?;
    let settings_path = home.join(".claude/settings.json");
    install_global_claude_hooks(&settings_path)?;
    println!("Installed global hooks in {}", settings_path.display());

    // Install Pi extension globally
    let pi_ext_path = home.join(".pi/agent/extensions/edict-hooks.ts");
    install_pi_extension(&pi_ext_path)?;
    println!("Installed Pi extension at {}", pi_ext_path.display());

    // If in a botbox project, also register botbus hooks (router + reviewers)
    if let Some(root) = project_root {
        let root = resolve_project_root(Some(root))?;
        let config = load_config(&root)?;
        register_botbus_hooks(&root, &config)?;
    } else if let Ok(root) = resolve_project_root(None) {
        if let Ok(config) = load_config(&root) {
            register_botbus_hooks(&root, &config)?;
        }
    }

    println!("Hooks installed successfully");
    Ok(())
}

/// Remove global agent hooks from ~/.claude/settings.json and Pi extensions.
fn uninstall_hooks() -> Result<()> {
    let home = dirs::home_dir().context("could not determine home directory")?;

    // Remove from ~/.claude/settings.json
    let settings_path = home.join(".claude/settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        let mut settings: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|_| json!({}));

        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            for (_event, entries) in hooks.iter_mut() {
                if let Some(arr) = entries.as_array_mut() {
                    arr.retain(|entry| !is_botbox_hook_entry(entry));
                }
            }
            // Remove empty event arrays
            hooks.retain(|_, v| {
                v.as_array().map(|a| !a.is_empty()).unwrap_or(true)
            });
        }

        // Remove hooks key entirely if empty
        if settings
            .get("hooks")
            .and_then(|h| h.as_object())
            .is_some_and(|h| h.is_empty())
        {
            settings.as_object_mut().unwrap().remove("hooks");
        }

        fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
        println!("Removed botbox hooks from {}", settings_path.display());
    }

    // Remove Pi extension
    let pi_ext_path = home.join(".pi/agent/extensions/edict-hooks.ts");
    if pi_ext_path.exists() {
        fs::remove_file(&pi_ext_path)?;
        println!("Removed {}", pi_ext_path.display());
    }

    println!("Hooks uninstalled successfully");
    Ok(())
}

fn audit_hooks(project_root: Option<&Path>, format: super::doctor::OutputFormat) -> Result<()> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    let mut issues = Vec::new();

    // Check global settings.json
    let settings_path = home.join(".claude/settings.json");
    if !settings_path.exists() {
        issues.push("Missing ~/.claude/settings.json".to_string());
    } else {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        let settings: serde_json::Value = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", settings_path.display()))?;

        for hook_entry in &HookRegistry::all() {
            let found = hook_entry.events.iter().any(|event| {
                settings["hooks"][event.as_str()]
                    .as_array()
                    .is_some_and(|arr| {
                        arr.iter().any(|entry| {
                            entry["hooks"].as_array().is_some_and(|hooks| {
                                hooks.iter().any(|h| is_botbox_hook_command(h, hook_entry.name))
                            })
                        })
                    })
            });

            if !found {
                issues.push(format!(
                    "Hook '{}' not registered in ~/.claude/settings.json",
                    hook_entry.name
                ));
            }
        }
    }

    // Check botbus hooks (if in a botbox project)
    if let Some(root) = project_root
        .and_then(|p| resolve_project_root(Some(p)).ok())
        .or_else(|| resolve_project_root(None).ok())
    {
        if let Ok(config) = load_config(&root) {
            if config.tools.botbus {
                check_botbus_hooks(&root, &config, &mut issues)?;
            }
        }
    }

    match format {
        super::doctor::OutputFormat::Json => {
            let result = json!({
                "issues": issues,
                "status": if issues.is_empty() { "ok" } else { "issues_found" }
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        super::doctor::OutputFormat::Pretty | super::doctor::OutputFormat::Text => {
            if issues.is_empty() {
                println!("✓ All hooks configured correctly");
            } else {
                eprintln!("Hook audit found {} issue(s):", issues.len());
                for issue in &issues {
                    eprintln!("  - {issue}");
                }
                return Err(ExitError::AuditFailed.into());
            }
        }
    }

    Ok(())
}

fn run_hook(hook_name: &str, release: bool) -> Result<()> {
    // Read stdin with a size limit (64KB) for defense-in-depth
    let stdin_input = {
        use std::io::Read;
        let mut buf = String::new();
        let mut handle = std::io::stdin().take(64 * 1024);
        handle.read_to_string(&mut buf).ok();
        if buf.is_empty() { None } else { Some(buf) }
    };

    match hook_name {
        "session-start" => crate::hooks::run_session_start(),
        "post-tool-call" => crate::hooks::run_post_tool_call(stdin_input.as_deref()),
        "session-end" => crate::hooks::run_session_end(),
        // Backwards compat: old hook names map to new ones
        "init-agent" | "check-jj" => crate::hooks::run_session_start(),
        "check-bus-inbox" => crate::hooks::run_post_tool_call(stdin_input.as_deref()),
        "claim-agent" => {
            if release {
                crate::hooks::run_session_end()
            } else {
                // claim-agent on SessionStart/PostToolUse — handled by session-start/post-tool-call
                crate::hooks::run_session_start()
            }
        }
        _ => Err(ExitError::Config(format!("unknown hook: {hook_name}")).into()),
    }
}

// --- Helper functions ---

fn resolve_project_root(project_root: Option<&Path>) -> Result<PathBuf> {
    let path = project_root
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().expect("get cwd"));
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving project root: {}", path.display()))?;
    match crate::config::find_config_in_project(&canonical) {
        Ok((_config_path, config_dir)) => Ok(config_dir),
        Err(_) => anyhow::bail!(
            "no .edict.toml or .botbox.toml found at {} or ws/default/ — is this an edict project?",
            canonical.display()
        ),
    }
}

fn load_config(root: &Path) -> Result<Config> {
    let (config_path, _config_dir) = crate::config::find_config_in_project(root)
        .map_err(|_| ExitError::Config("no .edict.toml or .botbox.toml found".into()))?;
    Config::load(&config_path)
}

/// Install global Claude Code hooks into ~/.claude/settings.json
fn install_global_claude_hooks(settings_path: &Path) -> Result<()> {
    let hooks = HookRegistry::all();

    let mut hooks_config: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for hook_entry in &hooks {
        for event in hook_entry.events.iter() {
            let entry = json!({
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": format!("edict hooks run {}", hook_entry.name)
                }]
            });
            hooks_config
                .entry(event.as_str().to_string())
                .or_default()
                .push(entry);
        }
    }

    // Load existing settings or create new
    let mut settings = if settings_path.exists() {
        let content = fs::read_to_string(settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        serde_json::from_str::<serde_json::Value>(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Merge: preserve non-botbox hooks, replace botbox hooks
    let existing_hooks = settings.get("hooks").cloned().unwrap_or_else(|| json!({}));
    let mut merged_hooks = existing_hooks.as_object().cloned().unwrap_or_default();

    for (event, new_entries) in &hooks_config {
        let existing_entries: Vec<serde_json::Value> = merged_hooks
            .get(event)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|entry| !is_botbox_hook_entry(entry))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let mut combined = existing_entries;
        combined.extend(new_entries.iter().cloned());
        merged_hooks.insert(event.clone(), serde_json::Value::Array(combined));
    }

    settings["hooks"] = serde_json::Value::Object(merged_hooks);

    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    fs::write(settings_path, serde_json::to_string_pretty(&settings)?)
        .with_context(|| format!("writing {}", settings_path.display()))?;

    Ok(())
}

fn install_pi_extension(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, PI_EDICT_HOOKS_EXTENSION)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Check if a hook entry is edict-managed (current or legacy botbox)
fn is_botbox_hook_entry(entry: &serde_json::Value) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                let cmd = &h["command"];
                if let Some(cmd_str) = cmd.as_str() {
                    cmd_str.contains("edict hooks run") || cmd_str.contains("botbox hooks run")
                } else if let Some(cmd_arr) = cmd.as_array() {
                    cmd_arr.len() >= 3
                        && (cmd_arr[0].as_str() == Some("edict")
                            || cmd_arr[0].as_str() == Some("botbox"))
                        && cmd_arr[1].as_str() == Some("hooks")
                        && cmd_arr[2].as_str() == Some("run")
                } else {
                    false
                }
            })
        })
}

/// Check if a specific hook command matches a hook name (edict or legacy botbox)
fn is_botbox_hook_command(h: &serde_json::Value, name: &str) -> bool {
    let cmd = &h["command"];
    if let Some(cmd_str) = cmd.as_str() {
        cmd_str.contains(&format!("run {name}"))
    } else if let Some(cmd_arr) = cmd.as_array() {
        cmd_arr.len() >= 4
            && (cmd_arr[0].as_str() == Some("edict") || cmd_arr[0].as_str() == Some("botbox"))
            && cmd_arr[1].as_str() == Some("hooks")
            && cmd_arr[2].as_str() == Some("run")
            && cmd_arr[3].as_str() == Some(name)
    } else {
        false
    }
}

/// Validates a name against `[a-z0-9][a-z0-9-]*` to prevent shell injection.
fn validate_name(name: &str, label: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || name.starts_with('-')
    {
        anyhow::bail!("invalid {label} {name:?}: must match [a-z0-9][a-z0-9-]*");
    }
    Ok(())
}

fn register_botbus_hooks(root: &Path, config: &Config) -> Result<()> {
    if !config.tools.botbus {
        return Ok(());
    }

    let channel = config.channel();
    let project_name = &config.project.name;
    let agent = config.default_agent();

    validate_name(project_name, "project name")?;
    validate_name(&channel, "channel name")?;
    for reviewer in &config.review.reviewers {
        validate_name(reviewer, "reviewer name")?;
    }

    let env_inherit = "BOTBUS_CHANNEL,BOTBUS_MESSAGE_ID,BOTBUS_HOOK_ID,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";
    let root_str = root.display().to_string();

    // Register router hook (claim-based)
    let router_claim = format!("agent://{project_name}-router");
    let spawn_name = format!("{project_name}-router");
    let description = format!("edict:{project_name}:responder");

    let responder_memory_limit = config
        .agents
        .responder
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    let mut router_args: Vec<&str> = vec![
        "--agent",
        &agent,
        "--channel",
        &channel,
        "--claim",
        &router_claim,
        "--claim-owner",
        &agent,
        "--cwd",
        &root_str,
        "--ttl",
        "600",
        "--",
        "botty",
        "spawn",
        "--env-inherit",
        env_inherit,
    ];
    if let Some(limit) = responder_memory_limit {
        router_args.push("--memory-limit");
        router_args.push(limit);
    }
    router_args.extend_from_slice(&[
        "--name",
        &spawn_name,
        "--cwd",
        &root_str,
        "--",
        "edict",
        "run",
        "responder",
    ]);

    match crate::subprocess::ensure_bus_hook(&description, &router_args) {
        Ok((action, _)) => println!("Router hook {action} for #{channel}"),
        Err(e) => eprintln!("Warning: failed to register router hook: {e}"),
    }

    // Register reviewer hooks (mention-based)
    let reviewer_memory_limit = config
        .agents
        .reviewer
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    for reviewer in &config.review.reviewers {
        let reviewer_agent = format!("{project_name}-{reviewer}");
        let claim_uri = format!("agent://{reviewer_agent}");
        let desc = format!("edict:{project_name}:reviewer-{reviewer}");

        let mut reviewer_args: Vec<&str> = vec![
            "--agent",
            &agent,
            "--channel",
            &channel,
            "--mention",
            &reviewer_agent,
            "--claim",
            &claim_uri,
            "--claim-owner",
            &reviewer_agent,
            "--ttl",
            "600",
            "--priority",
            "1",
            "--cwd",
            &root_str,
            "--",
            "botty",
            "spawn",
            "--env-inherit",
            env_inherit,
        ];
        if let Some(limit) = reviewer_memory_limit {
            reviewer_args.push("--memory-limit");
            reviewer_args.push(limit);
        }
        reviewer_args.extend_from_slice(&[
            "--name",
            &reviewer_agent,
            "--cwd",
            &root_str,
            "--",
            "edict",
            "run",
            "reviewer-loop",
            "--agent",
            &reviewer_agent,
        ]);

        match crate::subprocess::ensure_bus_hook(&desc, &reviewer_args) {
            Ok((action, _)) => println!("Reviewer hook for @{reviewer_agent} {action}"),
            Err(e) => {
                eprintln!("Warning: failed to register reviewer hook for @{reviewer_agent}: {e}")
            }
        }
    }

    Ok(())
}

fn check_botbus_hooks(root: &Path, config: &Config, issues: &mut Vec<String>) -> Result<()> {
    let output = run_command("bus", &["hooks", "list", "--format", "json"], Some(root));

    let hooks_data = match output {
        Ok(json) => serde_json::from_str::<serde_json::Value>(&json).ok(),
        Err(_) => None,
    };

    if hooks_data.is_none() {
        issues.push("Failed to fetch botbus hooks".to_string());
        return Ok(());
    }

    let hooks_data = hooks_data.unwrap();
    let empty_vec = vec![];
    let hooks = hooks_data["hooks"].as_array().unwrap_or(&empty_vec);

    let router_claim = format!("agent://{}-router", config.project.name);
    let has_router = hooks.iter().any(|h| {
        h["condition"]["claim"]
            .as_str()
            .map(|c| c == router_claim)
            .unwrap_or(false)
    });

    if !has_router {
        issues.push(format!(
            "Missing botbus router hook (claim: {router_claim})"
        ));
    }

    for reviewer in &config.review.reviewers {
        let mention_name = format!("{}-{reviewer}", config.project.name);
        let has_reviewer = hooks.iter().any(|h| {
            h["condition"]["mention"]
                .as_str()
                .map(|m| m == mention_name)
                .unwrap_or(false)
        });

        if !has_reviewer {
            issues.push(format!("Missing botbus reviewer hook for @{mention_name}"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_valid() {
        assert!(validate_name("botbox", "test").is_ok());
        assert!(validate_name("my-project", "test").is_ok());
        assert!(validate_name("a", "test").is_ok());
        assert!(validate_name("project123", "test").is_ok());
    }

    #[test]
    fn validate_name_rejects_invalid() {
        assert!(validate_name("", "test").is_err());
        assert!(validate_name("-starts-dash", "test").is_err());
        assert!(validate_name("Has Uppercase", "test").is_err());
        assert!(validate_name("has space", "test").is_err());
        assert!(validate_name("$(inject)", "test").is_err());
        assert!(validate_name("; rm -rf /", "test").is_err());
        assert!(validate_name("name\nwith\nnewlines", "test").is_err());
    }

    #[test]
    fn is_botbox_hook_entry_detects_edict_string_command() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": "edict hooks run session-start"}]
        });
        assert!(is_botbox_hook_entry(&entry));
    }

    #[test]
    fn is_botbox_hook_entry_detects_edict_array_command() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": ["edict", "hooks", "run", "session-start"]}]
        });
        assert!(is_botbox_hook_entry(&entry));
    }

    #[test]
    fn is_botbox_hook_entry_detects_legacy_botbox_string_command() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": "botbox hooks run session-start"}]
        });
        assert!(is_botbox_hook_entry(&entry));
    }

    #[test]
    fn is_botbox_hook_entry_detects_legacy_botbox_array_command() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": ["botbox", "hooks", "run", "session-start"]}]
        });
        assert!(is_botbox_hook_entry(&entry));
    }

    #[test]
    fn is_botbox_hook_entry_preserves_non_botbox() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": "my-custom-hook"}]
        });
        assert!(!is_botbox_hook_entry(&entry));
    }

    #[test]
    fn is_botbox_hook_entry_detects_old_format() {
        let entry = json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": "botbox hooks run init-agent --project-root /tmp"}]
        });
        assert!(is_botbox_hook_entry(&entry));
    }
}
