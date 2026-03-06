use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::subprocess::run_command;


/// Detected runtime context for hooks.
struct HookContext {
    /// If in a maw repo, the path containing .manifold
    maw_root: Option<std::path::PathBuf>,
    /// If in an edict project, the loaded config
    edict_config: Option<Config>,
    /// Agent name from $AGENT or $BOTBUS_AGENT
    agent: Option<String>,
}

impl HookContext {
    fn detect() -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();

        let agent = std::env::var("AGENT")
            .or_else(|_| std::env::var("BOTBUS_AGENT"))
            .ok()
            .filter(|a| validate_agent_name(a));

        let maw_root = find_ancestor_with(&cwd, ".manifold");

        let edict_config = find_edict_config(&cwd)
            .and_then(|p| Config::load(&p).ok());

        Self {
            maw_root,
            edict_config,
            agent,
        }
    }

    fn channel(&self) -> Option<String> {
        self.edict_config.as_ref().map(|c| c.channel())
    }
}

/// Run session-start hook: maw guidance + agent identity + stake claim
pub fn run_session_start() -> Result<()> {
    let ctx = HookContext::detect();

    // 1. Maw repo guidance
    if ctx.maw_root.is_some() {
        println!(
            "This project uses Git + maw for version control. \
            Source files live in workspaces under ws/, not at the project root. \
            Use `maw exec <workspace> -- <command>` to run commands. \
            Run `maw --help` for more info. Do NOT run jj commands."
        );
    }

    // 2. Agent identity + project channel (if edict project and agent set)
    if let Some(ref agent) = ctx.agent {
        if let Some(ref config) = ctx.edict_config {
            println!("Agent ID for use with botbus/seal/bn: {agent}");
            println!("Project channel: {}", config.channel());
        }
    }

    // 3. Stake claim (if agent set)
    if let Some(ref agent) = ctx.agent {
        stake_claim(agent);
    }

    Ok(())
}

/// Run post-tool-call hook: check bus inbox + refresh claim
pub fn run_post_tool_call(hook_input: Option<&str>) -> Result<()> {
    let ctx = HookContext::detect();

    let Some(ref agent) = ctx.agent else {
        return Ok(());
    };

    // 1. Check bus inbox
    check_bus_inbox(&ctx, agent, hook_input)?;

    // 2. Refresh claim if expiring
    refresh_claim_if_needed(agent);

    Ok(())
}

/// Run session-end hook: release claim + clear status
pub fn run_session_end() -> Result<()> {
    let agent = std::env::var("AGENT")
        .or_else(|_| std::env::var("BOTBUS_AGENT"))
        .ok()
        .filter(|a| validate_agent_name(a));

    let Some(agent) = agent else {
        return Ok(());
    };

    let claim_uri = format!("agent://{agent}");
    let _ = run_command(
        "bus",
        &["claims", "release", "--agent", &agent, &claim_uri, "-q"],
        None,
    );
    let _ = run_command(
        "bus",
        &["statuses", "clear", "--agent", &agent, "-q"],
        None,
    );

    Ok(())
}

// --- Internal helpers ---

fn stake_claim(agent: &str) {
    let claim_uri = format!("agent://{agent}");
    let _ = run_command(
        "bus",
        &[
            "claims", "stake", "--agent", agent, &claim_uri, "--ttl", "600", "-q",
        ],
        None,
    );
}

fn refresh_claim_if_needed(agent: &str) {
    let claim_uri = format!("agent://{agent}");
    let refresh_threshold = 120;

    let list_output = run_command(
        "bus",
        &[
            "claims", "list", "--mine", "--agent", agent, "--format", "json",
        ],
        None,
    )
    .ok();

    if let Some(output) = list_output
        && let Ok(data) = serde_json::from_str::<serde_json::Value>(&output)
        && let Some(claims) = data["claims"].as_array()
    {
        for claim in claims {
            if let Some(patterns) = claim["patterns"].as_array()
                && patterns.iter().any(|p| p.as_str() == Some(&claim_uri))
                && let Some(expires_in) = claim["expires_in_secs"].as_i64()
                && expires_in < refresh_threshold
            {
                let _ = run_command(
                    "bus",
                    &[
                        "claims", "refresh", "--agent", agent, &claim_uri, "--ttl", "600", "-q",
                    ],
                    None,
                );
            }
        }
    }
}

fn check_bus_inbox(ctx: &HookContext, agent: &str, _hook_input: Option<&str>) -> Result<()> {
    let channel = match ctx.channel() {
        Some(ch) => ch,
        None => return Ok(()), // No edict project, skip inbox check
    };

    let agent_flag = format!("--agent={agent}");

    // Check unread count
    let count_output = run_command(
        "bus",
        &[
            "inbox",
            &agent_flag,
            "--count-only",
            "--mentions",
            "--channels",
            &channel,
        ],
        None,
    )
    .ok();

    let count: u32 = count_output
        .as_ref()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    if count == 0 {
        return Ok(());
    }

    // Fetch messages as JSON
    let inbox_json = run_command(
        "bus",
        &[
            "inbox",
            &agent_flag,
            "--mentions",
            "--channels",
            &channel,
            "--limit-per-channel",
            "5",
            "--format",
            "json",
        ],
        None,
    )
    .unwrap_or_default();

    let messages = parse_inbox_previews(&inbox_json, Some(agent));

    let mark_read_cmd = format!("bus inbox --agent {agent} --mentions --channels {channel} --mark-read");

    let context = format!(
        "STOP: You have {count} unread bus message(s) in #{channel}. Check if any need a response:\n{messages}\n\nTo read and respond: `{mark_read_cmd}`"
    );

    let hook_output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context
        }
    });

    println!("{}", serde_json::to_string(&hook_output)?);

    Ok(())
}

/// Walk up from `start` looking for a directory containing `marker`.
fn find_ancestor_with(start: &Path, marker: &str) -> Option<std::path::PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(marker).exists() {
            return Some(dir);
        }
        // Also check ws/default/ (bare repo layout)
        if dir.join("ws/default").join(marker).exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk up from `start` looking for an edict/botbox config file.
/// Returns the config file path if found.
fn find_edict_config(start: &Path) -> Option<std::path::PathBuf> {
    // Current name first, then legacy names in order of recency
    const CONFIG_NAMES: &[&str] = &[".edict.toml", ".botbox.toml", ".botbox.json"];
    let mut dir = start.to_path_buf();
    loop {
        for name in CONFIG_NAMES {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        // Also check ws/default/
        let ws_default = dir.join("ws/default");
        if ws_default.exists() {
            for name in CONFIG_NAMES {
                let p = ws_default.join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Validates an agent name against `[a-z0-9][a-z0-9-/]*`.
fn validate_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'/')
        && !name.starts_with('-')
        && !name.starts_with('/')
}

fn parse_inbox_previews(inbox_json: &str, agent: Option<&str>) -> String {
    let data: serde_json::Value = match serde_json::from_str(inbox_json) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };

    let mut previews = Vec::new();

    let messages: Vec<&serde_json::Map<String, serde_json::Value>> =
        if let Some(arr) = data["mentions"].as_array() {
            arr.iter()
                .filter_map(|m| m["message"].as_object())
                .collect()
        } else if let Some(arr) = data["messages"].as_array() {
            arr.iter().filter_map(|m| m.as_object()).collect()
        } else {
            Vec::new()
        };

    for msg in messages {
        let sender = msg
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let body = msg.get("body").and_then(|v| v.as_str()).unwrap_or("");

        let tag = if let Some(a) = agent {
            if body.contains(&format!("@{a}")) {
                "[MENTIONS YOU] "
            } else {
                ""
            }
        } else {
            ""
        };

        let mut preview = format!("{tag}{sender}: {body}");
        if preview.len() > 100 {
            preview.truncate(97);
            preview.push_str("...");
        }

        previews.push(format!("  - {preview}"));
    }

    previews.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_ancestor_with_direct() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".manifold")).unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_ancestor_with_ws_default() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("ws/default/.manifold")).unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_ancestor_with_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert!(result.is_none());
    }

    #[test]
    fn find_edict_config_edict_toml_preferred() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".edict.toml"), "").unwrap();
        fs::write(tmp.path().join(".botbox.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(result, Some(tmp.path().join(".edict.toml")));
    }

    #[test]
    fn find_edict_config_legacy_toml_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".botbox.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(result, Some(tmp.path().join(".botbox.toml")));
    }

    #[test]
    fn find_edict_config_ws_default() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("ws/default")).unwrap();
        fs::write(tmp.path().join("ws/default/.edict.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(
            result,
            Some(tmp.path().join("ws/default/.edict.toml"))
        );
    }

    #[test]
    fn find_edict_config_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_edict_config(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn parse_inbox_previews_empty() {
        let json = r#"{"mentions":[]}"#;
        let result = parse_inbox_previews(json, None);
        assert_eq!(result, "");
    }

    #[test]
    fn parse_inbox_previews_with_messages() {
        let json = r#"{
            "mentions": [
                {
                    "message": {
                        "agent": "alice",
                        "body": "Hey @bob, check this out"
                    }
                }
            ]
        }"#;
        let result = parse_inbox_previews(json, Some("bob"));
        assert!(result.contains("[MENTIONS YOU]"));
        assert!(result.contains("alice"));
    }

    #[test]
    fn parse_inbox_previews_truncation() {
        let long_body = "a".repeat(200);
        let json = format!(
            r#"{{"mentions": [{{"message": {{"agent": "sender", "body": "{}"}}}}]}}"#,
            long_body
        );
        let result = parse_inbox_previews(&json, None);
        assert!(result.len() < 150);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn validate_agent_name_accepts_valid() {
        assert!(validate_agent_name("botbox-dev"));
        assert!(validate_agent_name("botbox-dev/worker-1"));
        assert!(validate_agent_name("a"));
        assert!(validate_agent_name("agent123"));
    }

    #[test]
    fn validate_agent_name_rejects_invalid() {
        assert!(!validate_agent_name(""));
        assert!(!validate_agent_name("-starts-dash"));
        assert!(!validate_agent_name("/starts-slash"));
        assert!(!validate_agent_name("Has Uppercase"));
        assert!(!validate_agent_name("has space"));
        assert!(!validate_agent_name("$(inject)"));
        assert!(!validate_agent_name("--help"));
    }
}
