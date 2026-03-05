#[allow(dead_code)]
mod dispatch;
mod journal;
#[allow(dead_code)]
mod merge;
#[allow(dead_code)]
mod mission;
#[allow(dead_code)]
mod monitor;
mod prompt;
#[allow(dead_code)]
mod release;
mod status;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;

use crate::config::Config;
use crate::subprocess::Tool;

use journal::Journal;
use status::StatusSnapshot;

/// Run the dev-loop (lead agent).
///
/// Triages work, dispatches parallel workers, monitors progress,
/// merges completed work, and manages releases.
pub fn run(
    project_root: Option<&Path>,
    agent_override: Option<&str>,
    model_override: Option<&str>,
) -> anyhow::Result<()> {
    let project_root = resolve_project_root(project_root)?;
    let (config, config_dir) = load_config(&project_root)?;

    let agent = resolve_agent(&config, agent_override)?;

    // Set AGENT and BOTBUS_AGENT env so spawned tools resolve identity correctly
    // SAFETY: single-threaded at this point in startup, before spawning any threads
    unsafe {
        std::env::set_var("AGENT", &agent);
        std::env::set_var("BOTBUS_AGENT", &agent);
    }

    // Apply config [env] vars to our own process so tools we invoke (cargo, etc.) inherit them
    for (k, v) in config.resolved_env() {
        // SAFETY: single-threaded at startup
        unsafe {
            std::env::set_var(&k, &v);
        }
    }

    let project = config.channel();
    let model = resolve_model(&config, model_override);
    let worker_model = resolve_worker_model(&config);

    let dev_config = config.agents.dev.clone().unwrap_or_default();
    let max_loops = dev_config.max_loops;
    let pause_secs = dev_config.pause;
    let timeout_secs = dev_config.timeout;
    let review_enabled = config.review.enabled;
    let push_main = config.push_main;

    let missions_config = dev_config.missions.clone();
    let missions_enabled = missions_config.as_ref().is_none_or(|m| m.enabled);
    let multi_lead_config = dev_config.multi_lead.clone();
    let multi_lead_enabled = multi_lead_config.as_ref().is_some_and(|m| m.enabled);

    let check_command = config.project.check_command.clone();
    let worker_timeout = config.agents.worker.as_ref().map_or(900, |w| w.timeout);

    let spawn_env = config.resolved_env();
    let worker_memory_limit = {
        let configured = config
            .agents
            .worker
            .as_ref()
            .and_then(|w| w.memory_limit.clone());
        if configured.is_some() && !is_systemd_dbus_available() {
            eprintln!(
                "Warning: worker memory limit configured but systemd D-Bus is not available \
                 (DBUS_SESSION_BUS_ADDRESS / XDG_RUNTIME_DIR not set) — skipping --memory-limit. \
                 To fix: add XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS to your project's \
                 [env] config so they are forwarded to spawned agents."
            );
            None
        } else {
            configured
        }
    };

    let ctx = LoopContext {
        agent: agent.clone(),
        project: project.clone(),
        model,
        worker_model,
        worker_timeout,
        review_enabled,
        push_main,
        check_command,
        missions_enabled,
        missions_config,
        multi_lead_enabled,
        multi_lead_config,
        project_dir: project_root.display().to_string(),
        spawn_env,
        worker_memory_limit,
    };

    eprintln!("Agent:     {agent}");
    eprintln!("Project:   {project}");
    eprintln!("Max loops: {max_loops}");
    eprintln!("Pause:     {pause_secs}s");
    eprintln!(
        "Model:     {}",
        if ctx.model.is_empty() {
            "system default"
        } else {
            &ctx.model
        }
    );
    eprintln!("Review:    {review_enabled}");
    if multi_lead_enabled {
        let max_leads = ctx.multi_lead_config.as_ref().map_or(3, |c| c.max_leads);
        eprintln!("Multi-lead: enabled (max {max_leads} slots)");
    }

    // Confirm identity
    Tool::new("bus")
        .args(&["whoami", "--agent", &agent])
        .run_ok()
        .context("confirming agent identity")?;

    // Stake agent claim (ignore failure — may already be held)
    let _ = Tool::new("bus")
        .args(&[
            "claims",
            "stake",
            "--agent",
            &agent,
            &format!("agent://{agent}"),
            "-m",
            &format!("dev-loop for {project}"),
        ])
        .run();

    // Announce
    Tool::new("bus")
        .args(&[
            "send",
            "--agent",
            &agent,
            &project,
            &format!("Dev agent {agent} online, starting dev loop"),
            "-L",
            "spawn-ack",
        ])
        .run_ok()?;

    // Set starting status
    let _ = Tool::new("bus")
        .args(&[
            "statuses",
            "set",
            "--agent",
            &agent,
            "Starting loop",
            "--ttl",
            "10m",
        ])
        .run();

    // Capture baseline commits for release tracking
    let baseline_commits = get_commits_since_origin();

    // Initialize journal
    let journal = Journal::new(&project_root);
    journal.truncate();

    // Install signal handler for cleanup
    let cleanup_agent = agent.clone();
    let cleanup_project = project.clone();
    let _ = ctrlc::set_handler(move || {
        // Best-effort cleanup on signal
        let _ = cleanup(&cleanup_agent, &cleanup_project);
        std::process::exit(0);
    });

    let mut idle_count: u32 = 0;
    let idle_delays = [10u64, 20, 40, 60, 60];
    let max_idle: u32 = 5;

    // Main loop
    for i in 1..=max_loops {
        eprintln!("\n--- Dev loop {i}/{max_loops} ---");
        crate::telemetry::metrics::counter(
            "edict.dev_loop.iterations_total",
            1,
            &[("agent", &agent), ("project", &project)],
        );

        // Refresh agent claim TTL
        let _ = Tool::new("bus")
            .args(&[
                "claims",
                "refresh",
                "--agent",
                &agent,
                &format!("agent://{agent}"),
            ])
            .run();

        if !has_work(&agent, &project)? {
            idle_count += 1;
            if idle_count >= max_idle {
                let _ = Tool::new("bus")
                    .args(&["statuses", "set", "--agent", &agent, "Idle"])
                    .run();
                eprintln!("No work after {max_idle} idle checks. Exiting cleanly.");
                let _ = Tool::new("bus")
                    .args(&[
                        "send", "--agent", &agent, &project,
                        &format!("No work remaining after {max_idle} checks. Dev agent {agent} signing off."),
                        "-L", "agent-idle",
                    ])
                    .run();
                break;
            }
            let delay = idle_delays[idle_count.saturating_sub(1) as usize % idle_delays.len()];
            eprintln!(
                "No work available (idle {idle_count}/{max_idle}). Waiting {delay}s before retrying..."
            );
            let _ = Tool::new("bus")
                .args(&[
                    "statuses",
                    "set",
                    "--agent",
                    &agent,
                    &format!("Idle ({idle_count}/{max_idle})"),
                    "--ttl",
                    &format!("{delay}s"),
                ])
                .run();
            std::thread::sleep(Duration::from_secs(delay));
            continue;
        }
        idle_count = 0;

        // Guard: if a review is pending, don't run Claude — just wait
        if let Some(pending_bead) = has_pending_review(&agent)? {
            eprintln!("Review pending for {pending_bead} — waiting (not running Claude)");
            let _ = Tool::new("bus")
                .args(&[
                    "statuses",
                    "set",
                    "--agent",
                    &agent,
                    &format!("Waiting: review for {pending_bead}"),
                    "--ttl",
                    "10m",
                ])
                .run();
            std::thread::sleep(Duration::from_secs(30));
            continue;
        }

        // Build prompt and run Claude
        let last_iteration = journal.read_last();
        let sibling_leads = if multi_lead_enabled {
            discover_sibling_leads(&agent)?
        } else {
            Vec::new()
        };
        let status_snapshot = StatusSnapshot::gather(&agent, &project);

        let prompt_text = prompt::build(
            &ctx,
            last_iteration.as_ref(),
            &sibling_leads,
            status_snapshot.as_deref(),
        );

        let agent_start = crate::telemetry::metrics::time_start();
        match run_agent_subprocess(&prompt_text, &ctx.model, timeout_secs) {
            Ok(output) => {
                // Check completion signals in the tail of the output
                let signal_region = if output.len() > 1000 {
                    let start = output.floor_char_boundary(output.len() - 1000);
                    &output[start..]
                } else {
                    &output
                };

                if signal_region.contains("<promise>COMPLETE</promise>") {
                    eprintln!("\u{2713} Dev cycle complete - no more work");
                    break;
                } else if signal_region.contains("<promise>END_OF_STORY</promise>") {
                    eprintln!("\u{2713} Iteration complete - more work remains");
                    // Verify work actually remains
                    if !has_work(&agent, &project)? {
                        eprintln!("No remaining work found despite END_OF_STORY — exiting cleanly");
                        break;
                    }
                } else {
                    eprintln!("Warning: No completion signal found in output");
                }

                // Extract and append iteration summary to journal
                if let Some(summary) = extract_iteration_summary(&output) {
                    journal.append(&summary);
                }
            }
            Err(err) => {
                eprintln!("Error running Claude: {err:#}");
                let err_str = format!("{err:#}");
                let is_fatal = err_str.contains("API Error")
                    || err_str.contains("rate limit")
                    || err_str.contains("overloaded");
                if is_fatal {
                    eprintln!("Fatal error detected, posting to botbus and exiting...");
                    let _ = Tool::new("bus")
                        .args(&[
                            "send",
                            "--agent",
                            &agent,
                            &project,
                            &format!("Dev loop error: {err_str}. Agent {agent} going offline."),
                            "-L",
                            "agent-error",
                        ])
                        .run();
                    break;
                }
                // Continue on non-fatal errors
            }
        }
        crate::telemetry::metrics::time_record(
            "edict.dev_loop.agent_run_duration_seconds",
            agent_start,
            &[("agent", &agent), ("project", &project)],
        );

        if i < max_loops {
            std::thread::sleep(Duration::from_secs(pause_secs.into()));
        }
    }

    // Show commits that landed this session
    let final_commits = get_commits_since_origin();
    let new_commits: Vec<_> = final_commits
        .iter()
        .filter(|c| !baseline_commits.contains(c))
        .collect();
    if !new_commits.is_empty() {
        eprintln!("\n--- Commits landed this session ---");
        for commit in &new_commits {
            eprintln!("  {commit}");
        }
        eprintln!("\nIf any are user-visible (feat/fix), consider a release.");
    }

    cleanup(&agent, &project)?;
    Ok(())
}

/// Context shared across the dev-loop iteration.
pub struct LoopContext {
    pub agent: String,
    pub project: String,
    pub model: String,
    pub worker_model: String,
    pub worker_timeout: u64,
    pub review_enabled: bool,
    pub push_main: bool,
    pub check_command: Option<String>,
    pub missions_enabled: bool,
    pub missions_config: Option<crate::config::MissionsConfig>,
    pub multi_lead_enabled: bool,
    pub multi_lead_config: Option<crate::config::MultiLeadConfig>,
    pub project_dir: String,
    /// Pre-resolved env vars from config [env] section.
    pub spawn_env: std::collections::HashMap<String, String>,
    /// Memory limit for worker agents (e.g. "4G"). Passed as --memory-limit to botty spawn.
    pub worker_memory_limit: Option<String>,
}

/// Info about a sibling lead agent.
pub struct SiblingLead {
    pub name: String,
    pub memo: String,
}

/// Resolve the project root directory.
fn resolve_project_root(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    std::env::current_dir().context("getting current directory")
}

/// Load config from .edict.toml/.botbox.toml (checking both project root and ws/default/).
/// Returns (config, config_dir) where config_dir is the directory containing the config file.
fn load_config(project_root: &Path) -> anyhow::Result<(Config, PathBuf)> {
    let (config_path, config_dir) = crate::config::find_config_in_project(project_root)?;
    Ok((Config::load(&config_path)?, config_dir))
}

/// Resolve the agent name from config or generate one.
fn resolve_agent(config: &Config, agent_override: Option<&str>) -> anyhow::Result<String> {
    if let Some(name) = agent_override {
        return Ok(name.to_string());
    }
    let from_config = config.default_agent();
    if !from_config.is_empty() {
        return Ok(from_config);
    }
    // Generate a name via bus
    let output = Tool::new("bus")
        .arg("generate-name")
        .run_ok()
        .context("generating agent name")?;
    Ok(output.stdout.trim().to_string())
}

/// Resolve the model for the lead dev, expanding tier names.
fn resolve_model(config: &Config, model_override: Option<&str>) -> String {
    let raw = if let Some(m) = model_override {
        m.to_string()
    } else {
        config
            .agents
            .dev
            .as_ref()
            .map_or_else(String::new, |d| d.model.clone())
    };
    if raw.is_empty() {
        raw
    } else {
        config.resolve_model(&raw)
    }
}

/// Get the raw worker model config value (tier name or explicit model).
///
/// Returns the unresolved value so the lead prompt can show tier names
/// like "fast"/"balanced"/"strong". The worker loop resolves them at runtime
/// through the tier pool for cross-provider load balancing.
fn resolve_worker_model(config: &Config) -> String {
    config
        .agents
        .worker
        .as_ref()
        .map_or_else(String::new, |w| w.model.clone())
}

/// Check if there is any work to do (inbox, claims, ready bones).
fn has_work(agent: &str, project: &str) -> anyhow::Result<bool> {
    // Check claims (bone:// or workspace:// means active work)
    if let Ok(output) = Tool::new("bus")
        .args(&[
            "claims", "list", "--agent", agent, "--mine", "--format", "json",
        ])
        .run()
        && output.success()
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&output.stdout)
    {
        let claims = parsed["claims"].as_array();
        if let Some(claims) = claims {
            let has_work_claims = claims.iter().any(|c| {
                c["patterns"].as_array().is_some_and(|patterns| {
                    patterns.iter().any(|p| {
                        let s = p.as_str().unwrap_or("");
                        s.starts_with("bone://") || s.starts_with("workspace://")
                    })
                })
            });
            if has_work_claims {
                return Ok(true);
            }
        }
    }

    // Check inbox
    if let Ok(output) = Tool::new("bus")
        .args(&[
            "inbox",
            "--agent",
            agent,
            "--channels",
            project,
            "--count-only",
            "--format",
            "json",
        ])
        .run()
        && output.success()
    {
        let count = parse_inbox_count(&output.stdout);
        if count > 0 {
            return Ok(true);
        }
    }

    // Check ready bones
    if let Ok(output) = Tool::new("bn")
        .args(&["next", "--json"])
        .in_workspace("default")?
        .run()
        && output.success()
    {
        let count = parse_ready_count(&output.stdout);
        if count > 0 {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Parse inbox count from JSON response.
fn parse_inbox_count(json: &str) -> u64 {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(n) = v.as_u64() {
            return n;
        }
        if let Some(n) = v["total_unread"].as_u64() {
            return n;
        }
    }
    0
}

/// Parse ready bones count from JSON response.
///
/// `bn next --json` returns `{"mode": "...", "assignments": [...]}` (bones v0.17.5+).
fn parse_ready_count(json: &str) -> usize {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(arr) = v["assignments"].as_array() {
            return arr.len();
        }
        if let Some(arr) = v.as_array() {
            return arr.len();
        }
        if let Some(arr) = v["issues"].as_array() {
            return arr.len();
        }
        if let Some(arr) = v["bones"].as_array() {
            return arr.len();
        }
    }
    0
}

/// Check if there's a pending review that should block running Claude.
fn has_pending_review(agent: &str) -> anyhow::Result<Option<String>> {
    // Get in-progress bones owned by this agent
    let output = Tool::new("bn")
        .args(&["list", "--state", "doing", "--assignee", agent, "--json"])
        .in_workspace("default")?
        .run();

    let output = match output {
        Ok(o) if o.success() => o,
        _ => return Ok(None),
    };

    let bones: Vec<serde_json::Value> = match serde_json::from_str(&output.stdout) {
        Ok(v) => {
            if let serde_json::Value::Array(arr) = v {
                arr
            } else {
                Vec::new()
            }
        }
        Err(_) => return Ok(None),
    };

    for bone in &bones {
        let id = match bone["id"].as_str() {
            Some(id) => id,
            None => continue,
        };

        let comments_output = Tool::new("bn")
            .args(&["bone", "comment", "list", id, "--json"])
            .in_workspace("default")?
            .run();

        let comments_output = match comments_output {
            Ok(o) if o.success() => o,
            _ => continue,
        };

        let comments = parse_comments(&comments_output.stdout);
        let has_review = comments
            .iter()
            .any(|c| c.contains("Review created:") || c.contains("Review requested:"));
        if !has_review {
            continue;
        }

        let has_completed = comments.iter().any(|c| c.contains("Completed by"));
        if has_completed {
            continue;
        }

        // Has a review but no completion — pending
        return Ok(Some(id.to_string()));
    }

    Ok(None)
}

/// Parse comment bodies from JSON output.
fn parse_comments(json: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        let arr = if let Some(a) = v.as_array() {
            a.clone()
        } else if let Some(a) = v["comments"].as_array() {
            a.clone()
        } else {
            return bodies;
        };
        for item in &arr {
            if let Some(body) = item["body"].as_str().or(item["content"].as_str()) {
                bodies.push(body.to_string());
            }
        }
    }
    bodies
}

/// Discover sibling lead agents (multi-lead mode).
fn discover_sibling_leads(agent: &str) -> anyhow::Result<Vec<SiblingLead>> {
    let output = Tool::new("bus")
        .args(&["claims", "list", "--format", "json"])
        .run()?;

    if !output.success() {
        return Ok(Vec::new());
    }

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).unwrap_or_default();
    let claims = parsed["claims"].as_array().cloned().unwrap_or_default();

    // Extract base agent name (strip /N suffix)
    let base_agent = agent.rfind('/').map_or(agent, |pos| {
        let suffix = &agent[pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            &agent[..pos]
        } else {
            agent
        }
    });

    let prefix = format!("agent://{base_agent}/");
    let mut siblings = Vec::new();

    for claim in &claims {
        let patterns = claim["patterns"].as_array().cloned().unwrap_or_default();
        for p in &patterns {
            let p_str = p.as_str().unwrap_or("");
            if p_str.starts_with(&prefix) {
                let lead_name_suffix = &p_str["agent://".len()..];
                if lead_name_suffix != agent {
                    siblings.push(SiblingLead {
                        name: lead_name_suffix.to_string(),
                        memo: claim["memo"].as_str().unwrap_or("").to_string(),
                    });
                }
            }
        }
    }

    Ok(siblings)
}

/// Run agent via `edict run agent` (Pi by default).
fn run_agent_subprocess(prompt: &str, model: &str, timeout_secs: u64) -> anyhow::Result<String> {
    let mut args = vec!["run", "agent", prompt];

    // Pass the full model string (e.g. "anthropic/claude-sonnet-4-6:medium") — Pi handles :suffix natively
    if !model.is_empty() {
        args.push("-m");
        args.push(model);
    }

    let timeout_str = timeout_secs.to_string();
    args.push("-t");
    args.push(&timeout_str);

    // Spawn the process, streaming stdout through
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let mut child = Command::new("edict")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning edict run agent")?;

    let stdout = child.stdout.take().context("capturing stdout")?;
    let reader = BufReader::new(stdout);
    let mut output = String::new();

    for line in reader.lines() {
        let line = line.context("reading stdout line")?;
        println!("{line}");
        output.push_str(&line);
        output.push('\n');
    }

    let status = child.wait().context("waiting for edict run agent")?;
    if status.success() {
        Ok(output)
    } else {
        let code = status.code().unwrap_or(-1);
        anyhow::bail!("edict run agent exited with code {code}")
    }
}

/// Extract iteration summary from Claude output.
fn extract_iteration_summary(output: &str) -> Option<String> {
    let start_tag = "<iteration-summary>";
    let end_tag = "</iteration-summary>";
    let start = output.find(start_tag)? + start_tag.len();
    let end = output[start..].find(end_tag)? + start;
    Some(output[start..end].trim().to_string())
}

/// Get commits on main since origin (for release tracking).
fn get_commits_since_origin() -> Vec<String> {
    let output = Tool::new("git")
        .args(&["log", "--oneline", "origin/main..main"])
        .in_workspace("default")
        .ok()
        .and_then(|t| t.run().ok());

    match output {
        Some(o) if o.success() => o
            .stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Cleanup: kill child workers, release claims.
fn cleanup(agent: &str, project: &str) -> anyhow::Result<()> {
    eprintln!("Cleaning up...");

    // Kill child workers
    kill_child_workers(agent);

    // All subprocess spawns below use .new_process_group() so they run in their
    // own process group and survive the SIGTERM that triggered this cleanup
    // (botty kill sends SIGTERM to the parent's process group, which would
    // otherwise kill these children before they complete).

    // Sign off
    let _ = Tool::new("bus")
        .args(&[
            "send",
            "--agent",
            agent,
            project,
            &format!("Dev agent {agent} signing off."),
            "-L",
            "agent-idle",
        ])
        .new_process_group()
        .run();

    // Clear status
    let _ = Tool::new("bus")
        .args(&["statuses", "clear", "--agent", agent])
        .new_process_group()
        .run();

    // Release merge mutex if held
    let _ = Tool::new("bus")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("workspace://{project}/default"),
        ])
        .new_process_group()
        .run();

    // Release agent claim
    let _ = Tool::new("bus")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("agent://{agent}"),
        ])
        .new_process_group()
        .run();

    // Release all remaining claims
    let _ = Tool::new("bus")
        .args(&["claims", "release", "--agent", agent, "--all"])
        .new_process_group()
        .run();

    // bn is event-sourced — no sync step needed

    eprintln!("Cleanup complete for {agent}.");
    Ok(())
}

/// Check whether the systemd user session D-Bus is available.
///
/// `--memory-limit` passes resource limits via systemd transient scopes, which requires
/// D-Bus. When botty-spawned agents don't inherit the session D-Bus address (e.g. because
/// `$DBUS_SESSION_BUS_ADDRESS` / `$XDG_RUNTIME_DIR` were not forwarded), the spawn fails
/// immediately with a "Failed to connect to user scope bus" error.
fn is_systemd_dbus_available() -> bool {
    if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok() {
        return true;
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if std::path::Path::new(&xdg).join("bus").exists() {
            return true;
        }
    }
    false
}

/// Kill child workers spawned by this dev-loop (hierarchical name pattern: AGENT/suffix).
fn kill_child_workers(agent: &str) {
    let output = Tool::new("botty").args(&["list", "--format", "json"]).run();

    let output = match output {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).unwrap_or_default();
    let agents = parsed["agents"].as_array().cloned().unwrap_or_default();
    let prefix = format!("{agent}/");

    for a in &agents {
        let name = a["id"].as_str().or(a["name"].as_str()).unwrap_or("");
        if name.starts_with(&prefix) {
            if let Err(_) = Tool::new("botty").args(&["kill", name]).run() {
                // Worker may have already exited
            }
            eprintln!("Killed child worker: {name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ready_count_assignments_envelope() {
        // bn next --json format since bones v0.17.5
        let json = r#"{"mode": "balanced", "assignments": [{"agent_slot": 1, "id": "bn-3smm"}]}"#;
        assert_eq!(parse_ready_count(json), 1);
    }

    #[test]
    fn parse_ready_count_assignments_multiple() {
        let json = r#"{"mode": "balanced", "assignments": [{"agent_slot": 1, "id": "bn-abc"}, {"agent_slot": 2, "id": "bn-def"}]}"#;
        assert_eq!(parse_ready_count(json), 2);
    }

    #[test]
    fn parse_ready_count_empty() {
        assert_eq!(parse_ready_count(r#"{"mode": "balanced", "assignments": []}"#), 0);
        assert_eq!(parse_ready_count("{}"), 0);
        assert_eq!(parse_ready_count("[]"), 0);
        assert_eq!(parse_ready_count(""), 0);
        assert_eq!(parse_ready_count("null"), 0);
    }

    #[test]
    fn parse_inbox_count_total_unread() {
        let json = r#"{"total_unread": 3}"#;
        assert_eq!(parse_inbox_count(json), 3);
    }

    #[test]
    fn parse_inbox_count_bare_number() {
        assert_eq!(parse_inbox_count("5"), 5);
    }
}
