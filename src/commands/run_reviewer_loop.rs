//! Reviewer loop implementation - processes code reviews across workspaces

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::{Config, ReviewerAgentConfig};
use crate::subprocess::Tool;

/// Known reviewer roles that can be derived from agent names
const KNOWN_ROLES: &[&str] = &["security"];

/// Derive the reviewer role from an agent name.
/// e.g., "myproject-security" -> Some("security"), "myproject-dev" -> None
pub fn derive_role_from_agent_name(agent_name: &str) -> Option<String> {
    for role in KNOWN_ROLES {
        if agent_name.ends_with(&format!("-{}", role)) {
            return Some(role.to_string());
        }
    }
    None
}

/// Get the prompt name for a reviewer based on role.
/// e.g., Some("security") -> "reviewer-security", None -> "reviewer"
pub fn get_reviewer_prompt_name(role: Option<&str>) -> String {
    match role {
        Some(r) => format!("reviewer-{}", r),
        None => "reviewer".to_string(),
    }
}

/// Validate that a name matches expected agent/project pattern (alphanumeric + hyphens).
fn validate_name(name: &str, label: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'/')
        || name.starts_with('-')
    {
        anyhow::bail!("invalid {label} name {name:?}: must match [a-z0-9-/]+, max 64 chars");
    }
    Ok(())
}

/// Load a prompt template and substitute `{{ VARIABLE }}` placeholders.
pub fn load_prompt(
    prompt_name: &str,
    agent: &str,
    project: &str,
    prompts_dir: &Path,
    workspace: Option<&str>,
) -> Result<String> {
    // Validate inputs to prevent template injection
    validate_name(agent, "agent")?;
    validate_name(project, "project")?;
    if let Some(ws) = workspace {
        validate_name(ws, "workspace")?;
    }

    // Prevent path traversal in prompt name
    if prompt_name.contains('/') || prompt_name.contains('\\') || prompt_name.contains("..") {
        anyhow::bail!("invalid prompt name {prompt_name:?}");
    }

    let file_path = prompts_dir.join(format!("{}.md", prompt_name));

    let template =
        fs::read_to_string(&file_path).with_context(|| "reading prompt template".to_string())?;

    // Simple variable substitution (support both spaced and unspaced forms)
    let mut result = template;
    result = result.replace("{{ AGENT }}", agent);
    result = result.replace("{{AGENT}}", agent);
    result = result.replace("{{ PROJECT }}", project);
    result = result.replace("{{PROJECT}}", project);

    // Replace {{ WORKSPACE }} with actual workspace or fallback to $WS
    let ws_value = workspace.unwrap_or("$WS");
    result = result.replace("{{ WORKSPACE }}", ws_value);
    result = result.replace("{{WORKSPACE}}", ws_value);

    Ok(result)
}

/// Get XDG-compliant cache directory for this project.
fn get_cache_dir() -> Result<PathBuf> {
    let base = if let Ok(xdg) = env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else if cfg!(target_os = "macos") {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?
            .join("Library")
            .join("Caches")
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?
            .join(".cache")
    };

    // Canonicalize current dir to prevent path traversal via symlinks
    let current_dir = env::current_dir()?
        .canonicalize()
        .unwrap_or_else(|_| env::current_dir().unwrap_or_default());

    // Use a safe slug: replace path separators, strip leading dashes, limit length
    let slug = current_dir
        .to_string_lossy()
        .replace(['/', '\\'], "-")
        .trim_start_matches('-')
        .to_string();

    // Verify slug doesn't contain path traversal
    if slug.contains("..") {
        anyhow::bail!("invalid project directory: path traversal detected");
    }

    let cache_path = base.join("edict").join("projects").join(&slug);

    // Verify the result is within the expected cache directory
    if !cache_path.starts_with(base.join("edict").join("projects")) {
        anyhow::bail!("cache directory escaped expected boundaries");
    }

    Ok(cache_path)
}

/// Get the journal path for a specific agent.
fn get_journal_path(agent_name: &str) -> Result<PathBuf> {
    let role = derive_role_from_agent_name(agent_name);
    let role_suffix = role.as_deref().unwrap_or("reviewer");
    let cache_dir = get_cache_dir()?;
    Ok(cache_dir.join(format!("review-loop-{}.txt", role_suffix)))
}

/// Workspace information from maw ws list.
#[derive(Debug, Deserialize)]
struct WorkspaceInfo {
    name: String,
}

/// maw ws list JSON output envelope.
#[derive(Debug, Deserialize)]
struct WorkspaceList {
    workspaces: Vec<WorkspaceInfo>,
}

/// Review information from seal inbox.
#[derive(Debug, Deserialize)]
struct ReviewInfo {
    #[serde(alias = "id")]
    review_id: String,
    #[serde(default)]
    title: Option<String>,
}

/// Thread information from seal inbox.
#[derive(Debug, Deserialize)]
struct ThreadInfo {
    #[serde(alias = "id")]
    thread_id: String,
    #[serde(default)]
    review_id: Option<String>,
}

/// seal inbox JSON output.
#[derive(Debug, Deserialize)]
struct CritInbox {
    #[serde(default)]
    reviews_awaiting_vote: Vec<ReviewInfo>,
    #[serde(default)]
    threads_with_new_responses: Vec<ThreadInfo>,
}

/// Review or thread with workspace context.
#[derive(Debug)]
struct WorkItem {
    workspace: String,
    review_id: String,
    title: Option<String>,
    is_thread: bool,
    thread_id: Option<String>,
}

/// Find pending reviews and threads across all workspaces.
fn find_work(agent: &str) -> Result<Vec<WorkItem>> {
    // Get list of workspaces
    let workspaces = match Tool::new("maw")
        .args(&["ws", "list", "--format", "json"])
        .run()
    {
        Ok(output) if output.success() => {
            let ws_list: WorkspaceList = output.parse_json()?;
            ws_list.workspaces.into_iter().map(|w| w.name).collect()
        }
        _ => vec!["default".to_string()], // Fall back to default if maw fails
    };

    let mut work_items = Vec::new();
    let mut seen_reviews = std::collections::HashSet::new();
    let mut seen_threads = std::collections::HashSet::new();

    for ws in workspaces {
        // Sync seal index to pick up newly created reviews (avoids race
        // condition when reviewer spawns before seal has indexed a new review)
        let _ = Tool::new("seal").in_workspace(&ws)?.args(&["sync"]).run();

        // Check seal inbox in this workspace
        let result = Tool::new("seal")
            .in_workspace(&ws)?
            .args(&["inbox", "--agent", agent, "--format", "json"])
            .run();

        if let Ok(output) = result
            && output.success()
            && let Ok(inbox) = output.parse_json::<CritInbox>()
        {
            // Deduplicate reviews
            for review in inbox.reviews_awaiting_vote {
                if seen_reviews.insert(review.review_id.clone()) {
                    work_items.push(WorkItem {
                        workspace: ws.clone(),
                        review_id: review.review_id,
                        title: review.title,
                        is_thread: false,
                        thread_id: None,
                    });
                }
            }

            // Deduplicate threads
            for thread in inbox.threads_with_new_responses {
                if seen_threads.insert(thread.thread_id.clone()) {
                    work_items.push(WorkItem {
                        workspace: ws.clone(),
                        review_id: thread.review_id.unwrap_or_default(),
                        title: None,
                        is_thread: true,
                        thread_id: Some(thread.thread_id),
                    });
                }
            }
        }
        // Silently skip workspaces where seal fails (stale, no .seal, etc.)
    }

    Ok(work_items)
}

/// Build the reviewer prompt with workspace context and last iteration.
fn build_prompt(
    agent: &str,
    project: &str,
    work_items: &[WorkItem],
    last_iteration: Option<(&str, &str)>, // (content, age)
) -> Result<String> {
    let role = derive_role_from_agent_name(agent);
    let prompt_name = get_reviewer_prompt_name(role.as_deref());

    // Find prompts directory (handle maw v2 bare repo layout)
    let mut prompts_dir = PathBuf::from(".agents/edict/prompts");
    if !prompts_dir.exists() {
        prompts_dir = PathBuf::from("ws/default/.agents/edict/prompts");
    }

    // Determine target workspace from first work item
    let target_workspace = work_items.first().map(|w| w.workspace.as_str());

    // Try to load specialized prompt, fall back to base reviewer if not found
    let mut base_prompt = match load_prompt(
        &prompt_name,
        agent,
        project,
        &prompts_dir,
        target_workspace,
    ) {
        Ok(p) => p,
        Err(_) if role.is_some() => {
            eprintln!(
                "Warning: {}.md not found, using base reviewer prompt",
                prompt_name
            );
            load_prompt("reviewer", agent, project, &prompts_dir, target_workspace)?
        }
        Err(e) => return Err(e),
    };

    // Prepend workspace preamble so the agent sees it before any steps
    if let Some(ws) = target_workspace {
        let preamble = format!(
            "## WORKSPACE CONTEXT\n\
             All code for this review is in workspace **{ws}**.\n\
             Use `maw exec {ws} -- ...` for ALL seal commands.\n\
             Read source files from `ws/{ws}/...` — NOT `ws/default/`.\n\n",
        );
        base_prompt.insert_str(0, &preamble);
    }

    // Append workspace context
    if !work_items.is_empty() {
        base_prompt.push_str("\n\n## PENDING WORK (pre-discovered by reviewer-loop)\n\n");
        base_prompt.push_str("The following reviews and threads need your attention. Workspace names are provided — use `maw exec <workspace> -- seal ...` to work in the correct workspace.\n\n");

        let reviews: Vec<_> = work_items.iter().filter(|w| !w.is_thread).collect();
        let threads: Vec<_> = work_items.iter().filter(|w| w.is_thread).collect();

        if !reviews.is_empty() {
            base_prompt.push_str("### Reviews awaiting vote:\n");
            for item in reviews {
                let title = item.title.as_deref().unwrap_or("(no title)");
                base_prompt.push_str(&format!(
                    "- Review {} in workspace **{}**: {}\n",
                    item.review_id, item.workspace, title
                ));
                base_prompt.push_str(&format!(
                    "  → maw exec {} -- seal review {}\n",
                    item.workspace, item.review_id
                ));
            }
        }

        if !threads.is_empty() {
            base_prompt.push_str("### Threads with new responses:\n");
            for item in threads {
                let review_info = if !item.review_id.is_empty() {
                    format!(" (review {})", item.review_id)
                } else {
                    String::new()
                };
                let thread_id = item.thread_id.as_deref().unwrap_or("");
                base_prompt.push_str(&format!(
                    "- Thread {} in workspace **{}**{}\n",
                    thread_id, item.workspace, review_info
                ));
                base_prompt.push_str(&format!(
                    "  → maw exec {} -- seal review {}\n",
                    item.workspace, item.review_id
                ));
            }
        }
    }

    // Append previous iteration context if available
    if let Some((content, age)) = last_iteration {
        base_prompt.push_str(&format!(
            "\n\n## PREVIOUS ITERATION ({}, may be stale)\n\n{}\n",
            age, content
        ));
    }

    Ok(base_prompt)
}

/// Read the last iteration from the journal.
fn read_last_iteration(journal_path: &Path) -> Option<(String, String)> {
    if !journal_path.exists() {
        return None;
    }

    let content = fs::read_to_string(journal_path).ok()?;
    let metadata = fs::metadata(journal_path).ok()?;
    let modified = metadata.modified().ok()?;
    let age_secs = std::time::SystemTime::now()
        .duration_since(modified)
        .ok()?
        .as_secs();

    let age_minutes = age_secs / 60;
    let age_hours = age_minutes / 60;
    let age_str = if age_hours > 0 {
        format!("{}h ago", age_hours)
    } else {
        format!("{}m ago", age_minutes)
    };

    Some((content.trim().to_string(), age_str))
}

/// Cleanup handler - release claims, clear status, send sign-off.
fn cleanup(agent: &str, project: &str, already_signed_off: bool) -> Result<()> {
    eprintln!("Cleaning up...");

    // All subprocess spawns below use .new_process_group() so they run in their
    // own process group and survive the SIGTERM that triggered this cleanup
    // (vessel kill sends SIGTERM to the parent's process group, which would
    // otherwise kill these children before they complete).

    if !already_signed_off {
        let _ = Tool::new("rite")
            .args(&[
                "send",
                "--agent",
                agent,
                project,
                &format!("Reviewer {} signing off.", agent),
                "-L",
                "agent-idle",
            ])
            .new_process_group()
            .run();
    }

    let _ = Tool::new("rite")
        .args(&["statuses", "clear", "--agent", agent])
        .new_process_group()
        .run();

    let _ = Tool::new("rite")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("agent://{}", agent),
        ])
        .new_process_group()
        .run();

    eprintln!("Cleanup complete for {}.", agent);
    Ok(())
}

/// Main entry point for reviewer-loop.
pub fn run_reviewer_loop(
    project_root: Option<PathBuf>,
    agent_override: Option<String>,
    model_override: Option<String>,
) -> Result<()> {
    // Change to project root if specified
    if let Some(root) = project_root {
        env::set_current_dir(&root)
            .with_context(|| format!("changing to project root {}", root.display()))?;
    }

    // Load config
    let cwd = Path::new(".");
    let (config_path, _) = crate::config::find_config_in_project(cwd)?;

    let config = Config::load(&config_path)?;

    // Determine agent name
    let agent = agent_override
        .or_else(|| config.project.default_agent.clone())
        .unwrap_or_else(|| config.default_agent());

    // Set AGENT and RITE_AGENT env so spawned tools (seal, rite) resolve identity correctly
    // SAFETY: single-threaded at this point in startup, before spawning any threads
    unsafe {
        env::set_var("AGENT", &agent);
        env::set_var("RITE_AGENT", &agent);
    }

    // Apply config [env] vars to our own process
    for (k, v) in config.resolved_env() {
        // SAFETY: single-threaded at startup
        unsafe {
            env::set_var(&k, &v);
        }
    }

    let project = config.channel();

    // Get reviewer config
    let reviewer_config = config
        .agents
        .reviewer
        .clone()
        .unwrap_or(ReviewerAgentConfig {
            model: "opus".to_string(),
            max_loops: 20,
            pause: 2,
            timeout: 900,
            memory_limit: None,
        });

    let model_raw = model_override.unwrap_or(reviewer_config.model);
    let model = config.resolve_model(&model_raw);
    let max_loops = reviewer_config.max_loops;
    let pause_secs = reviewer_config.pause;
    let timeout = reviewer_config.timeout;

    let journal_path = get_journal_path(&agent)?;

    eprintln!("Reviewer:  {}", agent);
    eprintln!("Project:   {}", project);
    eprintln!("Max loops: {}", max_loops);
    eprintln!("Pause:     {}s", pause_secs);
    eprintln!("Model:     {}", model);
    eprintln!("Journal:   {}", journal_path.display());

    // Confirm identity
    let whoami = Tool::new("rite")
        .args(&["whoami", "--agent", &agent])
        .run()?;

    if !whoami.success() {
        anyhow::bail!("Failed to confirm agent identity: {}", whoami.stderr);
    }

    // Try to refresh claim, otherwise stake
    let refresh = Tool::new("rite")
        .args(&[
            "claims",
            "refresh",
            "--agent",
            &agent,
            &format!("agent://{}", agent),
        ])
        .run();

    if refresh.is_err() || !refresh.as_ref().unwrap().success() {
        let stake = Tool::new("rite")
            .args(&[
                "claims",
                "stake",
                "--agent",
                &agent,
                &format!("agent://{}", agent),
                "-m",
                &format!("reviewer-loop for {}", project),
            ])
            .run();

        if stake.is_err() || !stake.as_ref().unwrap().success() {
            eprintln!("Claim held by another agent, continuing");
        }
    }

    // Announce
    let _ = Tool::new("rite")
        .args(&[
            "send",
            "--agent",
            &agent,
            &project,
            &format!("Reviewer {} online, starting review loop", agent),
            "-L",
            "spawn-ack",
        ])
        .run();

    // Set starting status
    let _ = Tool::new("rite")
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

    // Truncate journal at start
    if journal_path.exists() {
        fs::write(&journal_path, "")?;
    }

    // Install signal handler for cleanup
    let cleanup_agent = agent.clone();
    let cleanup_project = project.clone();
    let _ = ctrlc::set_handler(move || {
        let _ = cleanup(&cleanup_agent, &cleanup_project, false);
        std::process::exit(0);
    });

    let mut already_signed_off = false;

    // Main loop
    for i in 1..=max_loops {
        eprintln!("\n--- Review loop {}/{} ---", i, max_loops);
        crate::telemetry::metrics::counter(
            "edict.reviewer.iterations_total",
            1,
            &[("agent", &agent)],
        );

        let work_items = find_work(&agent)?;

        if work_items.is_empty() {
            let _ = Tool::new("rite")
                .args(&["statuses", "set", "--agent", &agent, "Idle"])
                .run();

            eprintln!("No reviews pending. Exiting cleanly.");

            let _ = Tool::new("rite")
                .args(&[
                    "send",
                    "--agent",
                    &agent,
                    &project,
                    &format!("No reviews pending. Reviewer {} signing off.", agent),
                    "-L",
                    "agent-idle",
                ])
                .run();

            already_signed_off = true;
            break;
        }

        let review_count = work_items.iter().filter(|w| !w.is_thread).count();
        let thread_count = work_items.iter().filter(|w| w.is_thread).count();
        eprintln!(
            "  {} reviews awaiting vote, {} threads with responses",
            review_count, thread_count
        );

        // Build prompt
        let last_iteration = read_last_iteration(&journal_path);
        let last_iter_ref = last_iteration
            .as_ref()
            .map(|(content, age)| (content.as_str(), age.as_str()));

        let prompt = build_prompt(&agent, &project, &work_items, last_iter_ref)?;

        // Run agent via Pi (default runtime)
        let reviewer_start = crate::telemetry::metrics::time_start();
        let run_agent_result = crate::commands::run_agent::run_agent(
            "pi",
            &prompt,
            Some(&model),
            timeout,
            None,
            false,
        );
        crate::telemetry::metrics::time_record(
            "edict.reviewer.agent_run_duration_seconds",
            reviewer_start,
            &[("agent", &agent)],
        );

        match run_agent_result {
            Ok(_) => {
                eprintln!("✓ Review iteration complete");
            }
            Err(e) => {
                eprintln!("Error running Claude: {}", e);
                // Continue to next iteration on error
            }
        }

        // Pause between iterations (except for the last one)
        if i < max_loops {
            std::thread::sleep(Duration::from_secs(pause_secs.into()));
        }
    }

    cleanup(&agent, &project, already_signed_off)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_role_security() {
        assert_eq!(
            derive_role_from_agent_name("myproject-security"),
            Some("security".to_string())
        );
        assert_eq!(
            derive_role_from_agent_name("foo-bar-security"),
            Some("security".to_string())
        );
    }

    #[test]
    fn test_derive_role_no_match() {
        assert_eq!(derive_role_from_agent_name("myproject-dev"), None);
        assert_eq!(derive_role_from_agent_name("security"), None);
        assert_eq!(derive_role_from_agent_name("project-sec"), None);
    }

    #[test]
    fn test_get_reviewer_prompt_name() {
        assert_eq!(
            get_reviewer_prompt_name(Some("security")),
            "reviewer-security"
        );
        assert_eq!(get_reviewer_prompt_name(None), "reviewer");
    }
}
