use crate::subprocess::Tool;

/// Pre-gathered status snapshot injected into the Claude prompt.
pub struct StatusSnapshot;

impl StatusSnapshot {
    /// Gather a status snapshot: unfinished bones, claims, inbox, ready bones, active workers.
    pub fn gather(agent: &str, project: &str) -> Option<String> {
        let mut sections = Vec::new();

        // Unfinished bones (crash recovery)
        if let Some(s) = gather_unfinished_bones(agent) {
            sections.push(s);
        }

        // Active claims
        if let Some(s) = gather_claims(agent) {
            sections.push(s);
        }

        // Inbox
        if let Some(s) = gather_inbox(agent, project) {
            sections.push(s);
        }

        // Ready bones
        if let Some(s) = gather_ready_bones() {
            sections.push(s);
        }

        // Active workers
        if let Some(s) = gather_active_workers(agent) {
            sections.push(s);
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }
}

fn gather_unfinished_bones(agent: &str) -> Option<String> {
    let output = Tool::new("bn")
        .args(&["list", "--state", "doing", "--assignee", agent, "--json"])
        .in_workspace("default")
        .ok()?
        .run()
        .ok()?;

    if !output.success() {
        return None;
    }

    let bones: Vec<serde_json::Value> = serde_json::from_str(&output.stdout).ok()?;
    if bones.is_empty() {
        return None;
    }

    let lines: Vec<String> = bones
        .iter()
        .map(|b| {
            let id = b["id"].as_str().unwrap_or("?");
            let urgency = b["urgency"].as_str().unwrap_or("default");
            let title = b["title"].as_str().unwrap_or("?");
            format!("  {id} [{urgency}]: {title}")
        })
        .collect();

    Some(format!(
        "UNFINISHED BONES ({}):\n{}",
        bones.len(),
        lines.join("\n")
    ))
}

fn gather_claims(agent: &str) -> Option<String> {
    let output = Tool::new("rite")
        .args(&[
            "claims", "list", "--agent", agent, "--mine", "--format", "json",
        ])
        .run()
        .ok()?;

    if !output.success() {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
    let claims = parsed["claims"].as_array()?;

    let work_claims: Vec<_> = claims
        .iter()
        .filter(|c| {
            c["patterns"].as_array().is_some_and(|patterns| {
                patterns.iter().any(|p| {
                    let s = p.as_str().unwrap_or("");
                    s.starts_with("bone://") || s.starts_with("workspace://")
                })
            })
        })
        .collect();

    if work_claims.is_empty() {
        return None;
    }

    let lines: Vec<String> = work_claims
        .iter()
        .map(|c| {
            let patterns: Vec<&str> = c["patterns"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| p.as_str())
                        .filter(|p| !p.starts_with("agent://"))
                        .collect()
                })
                .unwrap_or_default();
            let ttl = c["expires_in_secs"]
                .as_u64()
                .map(|s| format!(" ({}m left)", s / 60))
                .unwrap_or_default();
            let memo = c["memo"]
                .as_str()
                .filter(|m| !m.is_empty())
                .map(|m| format!(" \u{2014} {m}"))
                .unwrap_or_default();
            format!("  {}{ttl}{memo}", patterns.join(", "))
        })
        .collect();

    Some(format!(
        "ACTIVE CLAIMS ({}):\n{}",
        work_claims.len(),
        lines.join("\n")
    ))
}

fn gather_inbox(agent: &str, project: &str) -> Option<String> {
    let output = Tool::new("rite")
        .args(&[
            "inbox",
            "--agent",
            agent,
            "--channels",
            project,
            "--format",
            "json",
        ])
        .run()
        .ok()?;

    if !output.success() {
        return None;
    }

    let inbox: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
    let total_unread = inbox["total_unread"].as_u64().unwrap_or(0);
    if total_unread == 0 {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(channels) = inbox["channels"].as_array() {
        for channel in channels {
            if let Some(messages) = channel["messages"].as_array() {
                for msg in messages.iter().take(5) {
                    let msg_agent = msg["agent"].as_str().unwrap_or("?");
                    let label = msg["label"]
                        .as_str()
                        .filter(|l| !l.is_empty())
                        .map(|l| format!("[{l}]"))
                        .unwrap_or_default();
                    let body = msg["body"].as_str().unwrap_or("");
                    let truncated = if body.len() > 80 {
                        &body[..body.floor_char_boundary(80)]
                    } else {
                        body
                    };
                    lines.push(format!("  {msg_agent} {label}: {truncated}"));
                }
            }
        }
    }

    Some(format!(
        "INBOX ({total_unread} unread):\n{}",
        lines.join("\n")
    ))
}

fn gather_ready_bones() -> Option<String> {
    let output = Tool::new("bn")
        .args(&["next", "--json"])
        .in_workspace("default")
        .ok()?
        .run()
        .ok()?;

    if !output.success() {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
    let bones = if let Some(arr) = parsed.as_array() {
        arr.clone()
    } else if let Some(arr) = parsed["issues"].as_array() {
        arr.clone()
    } else if let Some(arr) = parsed["bones"].as_array() {
        arr.clone()
    } else {
        return None;
    };

    if bones.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = bones
        .iter()
        .take(10)
        .map(|b| {
            let id = b["id"].as_str().unwrap_or("?");
            let urgency = b["urgency"].as_str().unwrap_or("default");
            let assignees = b["assignees"]
                .as_array()
                .filter(|a| !a.is_empty())
                .map(|a| {
                    let names: Vec<&str> = a.iter().filter_map(|v| v.as_str()).collect();
                    format!(" ({})", names.join(","))
                })
                .unwrap_or_default();
            let title = b["title"].as_str().unwrap_or("?");
            let labels = b["labels"]
                .as_array()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let labels_str: Vec<&str> = l.iter().filter_map(|v| v.as_str()).collect();
                    format!(" [{}]", labels_str.join(","))
                })
                .unwrap_or_default();
            format!("  {id} [{urgency}]{assignees}: {title}{labels}")
        })
        .collect();

    if bones.len() > 10 {
        lines.push(format!("  ... and {} more", bones.len() - 10));
    }

    Some(format!(
        "READY BONES ({}):\n{}",
        bones.len(),
        lines.join("\n")
    ))
}

fn gather_active_workers(agent: &str) -> Option<String> {
    let output = Tool::new("vessel")
        .args(&["list", "--format", "json"])
        .run()
        .ok()?;

    if !output.success() {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
    let agents = parsed["agents"].as_array()?;
    let prefix = format!("{agent}/");

    let workers: Vec<_> = agents
        .iter()
        .filter(|a| a["id"].as_str().is_some_and(|id| id.starts_with(&prefix)))
        .collect();

    if workers.is_empty() {
        return None;
    }

    let lines: Vec<String> = workers
        .iter()
        .map(|w| {
            let id = w["id"].as_str().unwrap_or("?");
            let status = w["status"].as_str().unwrap_or("running");
            format!("  {id} ({status})")
        })
        .collect();

    Some(format!(
        "ACTIVE WORKERS ({}):\n{}",
        workers.len(),
        lines.join("\n")
    ))
}
