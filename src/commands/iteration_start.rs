use std::io::IsTerminal;

use serde::Deserialize;

use crate::config::Config;
use crate::subprocess::Tool;

// ===== Data Structures =====

#[derive(Debug, Deserialize)]
pub struct InboxResponse {
    pub total_unread: i32,
    pub channels: Option<Vec<InboxChannel>>,
}

#[derive(Debug, Deserialize)]
pub struct InboxChannel {
    pub messages: Option<Vec<InboxMessage>>,
}

#[derive(Debug, Deserialize)]
pub struct InboxMessage {
    pub agent: String,
    pub label: Option<String>,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewsResponse {
    pub reviews_awaiting_vote: Option<Vec<ReviewInfo>>,
    pub threads_with_new_responses: Option<Vec<ThreadInfo>>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewInfo {
    pub review_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ThreadInfo {}

#[derive(Debug, Deserialize)]
pub struct ClaimsResponse {
    pub claims: Option<Vec<Claim>>,
}

#[derive(Debug, Deserialize)]
pub struct Claim {
    pub patterns: Option<Vec<String>>,
    pub expires_in_secs: Option<i32>,
}

// ANSI color codes — conditionally applied based on TTY detection
pub struct Colors {
    pub reset: &'static str,
    pub bold: &'static str,
    pub dim: &'static str,
    pub cyan: &'static str,
    pub green: &'static str,
}

impl Colors {
    pub fn detect() -> Self {
        if std::io::stdout().is_terminal() {
            Self {
                reset: "\x1b[0m",
                bold: "\x1b[1m",
                dim: "\x1b[2m",
                cyan: "\x1b[36m",
                green: "\x1b[32m",
            }
        } else {
            Self {
                reset: "",
                bold: "",
                dim: "",
                cyan: "",
                green: "",
            }
        }
    }
}

pub fn h1(c: &Colors, s: &str) -> String {
    format!("{}{}# {}{}", c.bold, c.cyan, s, c.reset)
}

pub fn h2(c: &Colors, s: &str) -> String {
    format!("{}{}## {}{}", c.bold, c.green, s, c.reset)
}

pub fn hint(c: &Colors, s: &str) -> String {
    format!("{}> {}{}", c.dim, s, c.reset)
}

/// Fetch config from .edict.toml/.botbox.toml or ws/default/
fn load_config() -> anyhow::Result<Config> {
    let cwd = std::path::Path::new(".");
    let (config_path, _) = crate::config::find_config_in_project(cwd)?;
    Config::load(&config_path)
}

/// Helper to run a tool and parse JSON output, returning None on failure
fn run_json_tool(tool: &str, args: &[&str]) -> Option<String> {
    if tool == "bn" || tool == "seal" {
        // These need to be run in the default workspace
        let mut output = Tool::new(tool);
        for arg in args {
            output = output.arg(arg);
        }
        output = output.arg("--format").arg("json");

        let result = output.in_workspace("default").ok()?.run().ok()?;

        if result.success() {
            Some(result.stdout)
        } else {
            None
        }
    } else {
        // Direct tool execution
        let mut output = Tool::new(tool);
        for arg in args {
            output = output.arg(arg);
        }
        output = output.arg("--format").arg("json");

        let result = output.run().ok()?;
        if result.success() {
            Some(result.stdout)
        } else {
            None
        }
    }
}

/// Run iteration-start with optional overrides
pub fn run_iteration_start(agent_override: Option<&str>) -> anyhow::Result<()> {
    let config = load_config()?;
    let default_agent = config.default_agent();
    let agent = agent_override.unwrap_or(default_agent.as_str());
    let project = config.channel();
    let c = Colors::detect();

    println!("{}", h1(&c, &format!("Iteration Start: {}", agent)));
    println!();

    // 1. Unread rite messages
    let inbox_output = run_json_tool("rite", &["inbox", "--agent", agent, "--channels", &project]);
    let mut unread_count = 0;

    if let Some(output) = &inbox_output
        && let Ok(inbox) = serde_json::from_str::<InboxResponse>(output)
    {
        unread_count = inbox.total_unread;
    }

    println!(
        "{}",
        h2(&c, &format!("Unread Bus Messages ({})", unread_count))
    );

    if let Some(output) = inbox_output {
        if let Ok(inbox) = serde_json::from_str::<InboxResponse>(&output) {
            if inbox.total_unread > 0 {
                if let Some(channels) = inbox.channels {
                    for channel in channels {
                        if let Some(messages) = channel.messages {
                            for msg in messages.iter().take(5) {
                                let label = msg
                                    .label
                                    .as_ref()
                                    .map(|l| format!("[{}]", l))
                                    .unwrap_or_default();
                                let body = if msg.body.len() > 60 {
                                    format!("{}...", &msg.body[..msg.body.floor_char_boundary(60)])
                                } else {
                                    msg.body.clone()
                                };
                                println!(
                                    "   {}{}{} {}: {}",
                                    c.dim, msg.agent, c.reset, label, body
                                );
                            }
                        }
                    }
                }
            } else {
                println!("   {}No unread messages{}", c.dim, c.reset);
            }
        } else {
            println!("   {}No unread messages{}", c.dim, c.reset);
        }
    } else {
        println!("   {}No unread messages{}", c.dim, c.reset);
    }
    println!();

    // 2. Bones (via bn triage)
    if let Err(e) = super::triage::run_triage() {
        println!("   {}Could not fetch triage data: {}{}", c.dim, e, c.reset);
    }
    println!();

    // 3. Pending reviews
    println!("{}", h2(&c, "Pending Reviews"));
    let reviews_output = run_json_tool("seal", &["inbox", "--agent", agent]);
    let mut has_reviews = false;

    if let Some(output) = reviews_output {
        if let Ok(reviews) = serde_json::from_str::<ReviewsResponse>(&output) {
            let awaiting = reviews.reviews_awaiting_vote.unwrap_or_default();
            let threads = reviews.threads_with_new_responses.unwrap_or_default();

            if !awaiting.is_empty() || !threads.is_empty() {
                has_reviews = true;
                if !awaiting.is_empty() {
                    println!("   {} review(s) awaiting vote", awaiting.len());
                    for r in awaiting.iter().take(3) {
                        let no_title = "(no title)".to_string();
                        let title = r
                            .title
                            .as_ref()
                            .or(r.description.as_ref())
                            .unwrap_or(&no_title);
                        println!("   {}: {}", r.review_id, title);
                    }
                }
                if !threads.is_empty() {
                    println!("   {} thread(s) with new responses", threads.len());
                }
            } else {
                println!("   {}No pending reviews{}", c.dim, c.reset);
            }
        } else {
            println!("   {}No pending reviews{}", c.dim, c.reset);
        }
    } else {
        println!("   {}Could not fetch reviews{}", c.dim, c.reset);
    }
    println!();

    // 4. Active claims
    println!("{}", h2(&c, "Active Claims"));
    let claims_output = run_json_tool("rite", &["claims", "list", "--agent", agent, "--mine"]);

    if let Some(output) = claims_output {
        if let Ok(claims_data) = serde_json::from_str::<ClaimsResponse>(&output) {
            if let Some(claims) = claims_data.claims {
                // Filter out agent identity claims (those that start with "agent://")
                let resource_claims: Vec<_> = claims
                    .iter()
                    .filter(|cl| {
                        cl.patterns
                            .as_ref()
                            .map(|p| !p.iter().all(|pat| pat.starts_with("agent://")))
                            .unwrap_or(true)
                    })
                    .collect();

                if !resource_claims.is_empty() {
                    println!("   {} active claim(s)", resource_claims.len());
                    for claim in resource_claims.iter().take(5) {
                        if let Some(patterns) = &claim.patterns {
                            let resource_patterns: Vec<_> = patterns
                                .iter()
                                .filter(|p| !p.starts_with("agent://"))
                                .collect();
                            for pattern in resource_patterns {
                                let expires = claim
                                    .expires_in_secs
                                    .map(|s| format!("({}m left)", s / 60))
                                    .unwrap_or_default();
                                println!("   {} {}", pattern, expires);
                            }
                        }
                    }
                } else {
                    println!("   {}No resource claims{}", c.dim, c.reset);
                }
            } else {
                println!("   {}No active claims{}", c.dim, c.reset);
            }
        } else {
            println!("   {}No active claims{}", c.dim, c.reset);
        }
    } else {
        println!("   {}No active claims{}", c.dim, c.reset);
    }
    println!();

    // Summary hint
    if unread_count > 0 {
        println!(
            "{}",
            hint(
                &c,
                &format!(
                    "Get unread messages and mark them as read: rite inbox --agent {} --channels {} --mark-read",
                    agent, project
                )
            )
        );
    } else if has_reviews {
        println!(
            "{}",
            hint(
                &c,
                &format!(
                    "Start review: maw exec default -- seal inbox --agent {}",
                    agent
                )
            )
        );
    } else {
        println!("{}", hint(&c, "No work pending"));
    }

    Ok(())
}
