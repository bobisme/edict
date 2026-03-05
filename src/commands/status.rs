use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Args;
use serde::{Deserialize, Serialize};

use super::doctor::OutputFormat;
use super::protocol::context::ProtocolContext;
use super::protocol::review_gate;
use crate::config::Config;
use crate::subprocess::Tool;

/// Validate that a bone ID matches the expected pattern (e.g., bn-xxxx).
fn is_valid_bone_id(id: &str) -> bool {
    (id.starts_with("bn-") || id.starts_with("bd-"))
        && id.len() <= 20
        && id[3..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// Validate that a workspace name is safe (alphanumeric + hyphens only).
fn is_valid_workspace_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Project name for scoping (defaults to project.name in config)
    #[arg(long)]
    pub project: Option<String>,
    /// Agent name for filtering (defaults to EDICT_AGENT or defaultAgent in config)
    #[arg(long)]
    pub agent: Option<String>,
    /// Output format
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Advice {
    /// Priority level: CRITICAL, HIGH, MEDIUM, LOW, INFO
    pub severity: String,
    /// Human-readable advice message
    pub message: String,
    /// Suggested shell command (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusReport {
    pub ready_bones: ReadyBones,
    pub workspaces: WorkspaceSummary,
    pub inbox: InboxSummary,
    pub agents: AgentsSummary,
    pub claims: ClaimsSummary,
    /// Actionable advice based on cross-tool state
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<Advice>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadyBones {
    pub count: usize,
    pub items: Vec<BoneSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BoneSummary {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub total: usize,
    pub active: usize,
    pub stale: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InboxSummary {
    pub unread: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentsSummary {
    pub running: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimsSummary {
    pub active: usize,
}

impl StatusArgs {
    pub fn execute(&self) -> anyhow::Result<()> {
        let format = self.format.unwrap_or_else(|| {
            if std::io::stdout().is_terminal() {
                OutputFormat::Pretty
            } else {
                OutputFormat::Text
            }
        });

        // Get project and agent from args → env → config → hardcoded fallback
        let config = crate::config::find_config_in_project(&PathBuf::from("."))
            .ok()
            .and_then(|(p, _)| Config::load(&p).ok());
        let project = self
            .project
            .clone()
            .or_else(|| std::env::var("EDICT_PROJECT").ok())
            .or_else(|| config.as_ref().map(|c| c.project.name.clone()))
            .unwrap_or_else(|| "edict".to_string());

        let agent = self
            .agent
            .clone()
            .or_else(|| std::env::var("EDICT_AGENT").ok())
            .or_else(|| config.as_ref().map(|c| c.default_agent()))
            .unwrap_or_else(|| format!("{project}-dev"));

        // Get required reviewers from config (format: ["security"] → ["<project>-security"])
        let required_reviewers: Vec<String> = config
            .as_ref()
            .filter(|c| c.review.enabled)
            .map(|c| {
                c.review
                    .reviewers
                    .iter()
                    .map(|r| format!("{project}-{r}"))
                    .collect()
            })
            .unwrap_or_else(|| vec![format!("{project}-security")]);

        let mut report = StatusReport {
            ready_bones: ReadyBones {
                count: 0,
                items: vec![],
            },
            workspaces: WorkspaceSummary {
                total: 0,
                active: 0,
                stale: 0,
            },
            inbox: InboxSummary { unread: 0 },
            agents: AgentsSummary { running: 0 },
            claims: ClaimsSummary { active: 0 },
            advice: Vec::new(),
        };

        // Try to collect ProtocolContext for advice generation
        let ctx = ProtocolContext::collect(&project, &agent).ok();

        // 1. Ready bones
        if let Ok(output) = Tool::new("bn")
            .arg("next")
            .arg("--format")
            .arg("json")
            .run()
            && let Ok(bones_json) = serde_json::from_str::<serde_json::Value>(&output.stdout)
            && let Some(items) = bones_json.get("items").and_then(|v| v.as_array())
        {
            report.ready_bones.count = items.len();
            for item in items.iter().take(5) {
                if let (Some(id), Some(title)) = (
                    item.get("id").and_then(|v| v.as_str()),
                    item.get("title").and_then(|v| v.as_str()),
                ) {
                    report.ready_bones.items.push(BoneSummary {
                        id: id.to_string(),
                        title: title.to_string(),
                    });
                }
            }
        }

        // 2. Active workspaces
        if let Ok(output) = Tool::new("maw")
            .arg("ws")
            .arg("list")
            .arg("--format")
            .arg("json")
            .run()
            && let Ok(ws_json) = serde_json::from_str::<serde_json::Value>(&output.stdout)
        {
            if let Some(workspaces) = ws_json.get("workspaces").and_then(|v| v.as_array()) {
                report.workspaces.total = workspaces.len();
                for ws in workspaces {
                    if ws
                        .get("is_default")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    report.workspaces.active += 1;
                }
            }
            if let Some(ws_advice) = ws_json.get("advice").and_then(|v| v.as_array()) {
                report.workspaces.stale = ws_advice
                    .iter()
                    .filter(|a| {
                        a.get("message")
                            .and_then(|v| v.as_str())
                            .map(|s| s.contains("stale"))
                            .unwrap_or(false)
                    })
                    .count();
            }
        }

        // 3. Pending inbox
        if let Ok(output) = Tool::new("bus")
            .arg("inbox")
            .arg("--format")
            .arg("json")
            .run()
            && let Ok(inbox_json) = serde_json::from_str::<serde_json::Value>(&output.stdout)
            && let Some(messages) = inbox_json.get("messages").and_then(|v| v.as_array())
        {
            report.inbox.unread = messages.len();
        }

        // 4. Running agents
        if let Ok(output) = Tool::new("botty")
            .arg("list")
            .arg("--format")
            .arg("json")
            .run()
            && let Ok(agents_json) = serde_json::from_str::<serde_json::Value>(&output.stdout)
            && let Some(agents) = agents_json.get("agents").and_then(|v| v.as_array())
        {
            report.agents.running = agents.len();
        }

        // 5. Active claims
        if let Ok(output) = Tool::new("bus")
            .arg("claims")
            .arg("list")
            .arg("--format")
            .arg("json")
            .run()
            && let Ok(claims_json) = serde_json::from_str::<serde_json::Value>(&output.stdout)
            && let Some(claims) = claims_json.get("claims").and_then(|v| v.as_array())
        {
            report.claims.active = claims.len();
        }

        // 6. Generate advice based on cross-tool state
        if let Some(ref context) = ctx {
            self.generate_advice(&mut report, context, &required_reviewers)?;
        }

        match format {
            OutputFormat::Pretty => {
                self.print_pretty(&report);
            }
            OutputFormat::Text => {
                self.print_text(&report);
            }
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }

        Ok(())
    }

    /// Generate actionable advice from cross-tool state analysis.
    fn generate_advice(
        &self,
        report: &mut StatusReport,
        ctx: &ProtocolContext,
        required_reviewers: &[String],
    ) -> anyhow::Result<()> {
        // Priority 1: CRITICAL - orphaned claims (bone closed but claim still active)
        for (bone_id, _pattern) in ctx.held_bone_claims() {
            if !is_valid_bone_id(bone_id) {
                continue; // Skip malformed claim URIs
            }
            if let Ok(bone) = ctx.bone_status(bone_id) {
                if bone.state == "done" || bone.state == "archived" {
                    report.advice.push(Advice {
                        severity: "CRITICAL".to_string(),
                        message: format!(
                            "Orphaned claim: bone {} is closed but claim still active → cleanup required",
                            bone_id
                        ),
                        command: Some(format!("edict protocol cleanup {}", bone_id)),
                    });
                }
            }
        }

        // Priority 2: HIGH - LGTM review with no finish action
        for (bone_id, _pattern) in ctx.held_bone_claims() {
            if !is_valid_bone_id(bone_id) {
                continue;
            }
            if let Some(ws_name) = ctx.workspace_for_bone(bone_id) {
                if let Ok(reviews) = ctx.reviews_in_workspace(ws_name) {
                    for review_summary in reviews {
                        if let Ok(review_detail) =
                            ctx.review_status(&review_summary.review_id, ws_name)
                        {
                            let gate = review_gate::evaluate_review_gate(
                                &review_detail,
                                required_reviewers,
                            );

                            if gate.status == review_gate::ReviewGateStatus::Approved {
                                report.advice.push(Advice {
                                    severity: "HIGH".to_string(),
                                    message: format!(
                                        "Review {} approved (LGTM) → ready to finish bone {}",
                                        review_detail.review_id, bone_id
                                    ),
                                    command: Some(format!("edict protocol finish {}", bone_id)),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Priority 3: HIGH - BLOCK review needing response
        for (bone_id, _pattern) in ctx.held_bone_claims() {
            if !is_valid_bone_id(bone_id) {
                continue;
            }
            if let Some(ws_name) = ctx.workspace_for_bone(bone_id) {
                if let Ok(reviews) = ctx.reviews_in_workspace(ws_name) {
                    for review_summary in reviews {
                        if let Ok(review_detail) =
                            ctx.review_status(&review_summary.review_id, ws_name)
                        {
                            let gate = review_gate::evaluate_review_gate(
                                &review_detail,
                                required_reviewers,
                            );

                            if gate.status == review_gate::ReviewGateStatus::Blocked {
                                let blocked_by = gate.blocked_by.join(", ");
                                report.advice.push(Advice {
                                    severity: "HIGH".to_string(),
                                    message: format!(
                                        "Review {} blocked by {} → address feedback on bone {}",
                                        review_detail.review_id, blocked_by, bone_id
                                    ),
                                    command: Some(format!("bn show {}", bone_id)),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Priority 4: MEDIUM - in-progress bone with no workspace
        for (bone_id, _pattern) in ctx.held_bone_claims() {
            if !is_valid_bone_id(bone_id) {
                continue;
            }
            if let Ok(bone) = ctx.bone_status(bone_id) {
                if bone.state == "doing" && ctx.workspace_for_bone(bone_id).is_none() {
                    report.advice.push(Advice {
                        severity: "MEDIUM".to_string(),
                        message: format!(
                            "In-progress bone {} has no workspace → possible crash recovery needed",
                            bone_id
                        ),
                        command: Some(format!("bn show {}", bone_id)),
                    });
                }
            }
        }

        // Priority 5: MEDIUM - workspace with no bone claim
        for ws in ctx.workspaces() {
            if ws.is_default {
                continue;
            }
            let has_claim = ctx
                .held_workspace_claims()
                .iter()
                .any(|(name, _)| name == &ws.name);

            if !has_claim {
                let command = if is_valid_workspace_name(&ws.name) {
                    Some(format!("maw ws destroy {}", ws.name))
                } else {
                    None // Don't suggest a command with an unsafe name
                };
                report.advice.push(Advice {
                    severity: "MEDIUM".to_string(),
                    message: format!(
                        "Workspace {} has no bone claim → investigate or clean up",
                        ws.name
                    ),
                    command,
                });
            }
        }

        // Priority 6: LOW - ready bones available (informational)
        if report.ready_bones.count > 0 {
            report.advice.push(Advice {
                severity: "LOW".to_string(),
                message: format!(
                    "{} ready bone(s) available → run triage",
                    report.ready_bones.count
                ),
                command: Some("maw exec default -- bn next".to_string()),
            });
        }

        // Priority 7: INFO - agent idle with no work
        if ctx.held_bone_claims().is_empty() && report.ready_bones.count == 0 {
            report.advice.push(Advice {
                severity: "INFO".to_string(),
                message: "No held bones and no ready work → check inbox or create bones from tasks"
                    .to_string(),
                command: Some("bus inbox --agent $AGENT".to_string()),
            });
        }

        Ok(())
    }

    fn print_pretty(&self, report: &StatusReport) {
        println!("=== Botbox Status ===\n");

        println!("Ready Bones: {}", report.ready_bones.count);
        for bone in report.ready_bones.items.iter().take(5) {
            println!("  • {} — {}", bone.id, bone.title);
        }
        if report.ready_bones.count > 5 {
            println!("  ... and {} more", report.ready_bones.count - 5);
        }

        println!("\nWorkspaces:");
        println!(
            "  Total: {}  (Active: {}, Stale: {})",
            report.workspaces.total, report.workspaces.active, report.workspaces.stale
        );

        println!("\nInbox: {} unread", report.inbox.unread);
        println!("Running Agents: {}", report.agents.running);
        println!("Active Claims: {}", report.claims.active);

        if !report.advice.is_empty() {
            println!("\nAdvice:");
            for adv in &report.advice {
                println!("  [{}] {}", adv.severity, adv.message);
                if let Some(ref cmd) = adv.command {
                    println!("      → {}", cmd);
                }
            }
        }
    }

    fn print_text(&self, report: &StatusReport) {
        println!("edict-status");
        println!("ready-bones  count={}", report.ready_bones.count);
        for bone in report.ready_bones.items.iter().take(5) {
            println!("ready-bone  id={}  title={}", bone.id, bone.title);
        }
        println!(
            "workspaces  total={}  active={}  stale={}",
            report.workspaces.total, report.workspaces.active, report.workspaces.stale
        );
        println!("inbox  unread={}", report.inbox.unread);
        println!("agents  running={}", report.agents.running);
        println!("claims  active={}", report.claims.active);

        if !report.advice.is_empty() {
            println!("advice  count={}", report.advice.len());
            for adv in &report.advice {
                println!(
                    "advice-item  severity={}  message={}",
                    adv.severity, adv.message
                );
                if let Some(ref cmd) = adv.command {
                    println!("advice-command  {}", cmd);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advice_structure_is_serializable() {
        let adv = Advice {
            severity: "HIGH".to_string(),
            message: "Test advice".to_string(),
            command: Some("test-command".to_string()),
        };
        let json = serde_json::to_string(&adv).expect("should serialize");
        assert!(json.contains("\"severity\""));
        assert!(json.contains("\"message\""));
        assert!(json.contains("\"command\""));
    }

    #[test]
    fn status_report_with_empty_advice() {
        let report = StatusReport {
            ready_bones: ReadyBones {
                count: 0,
                items: vec![],
            },
            workspaces: WorkspaceSummary {
                total: 0,
                active: 0,
                stale: 0,
            },
            inbox: InboxSummary { unread: 0 },
            agents: AgentsSummary { running: 0 },
            claims: ClaimsSummary { active: 0 },
            advice: vec![],
        };

        let json = serde_json::to_string_pretty(&report).expect("should serialize");
        // Empty advice array should not be included due to skip_serializing_if
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");
        // The advice key may or may not exist if empty due to skip_serializing_if
        // but if present, it should be an empty array
        if let Some(advice) = parsed.get("advice") {
            assert!(advice.is_array());
            assert_eq!(advice.as_array().unwrap().len(), 0);
        }
    }

    #[test]
    fn status_report_with_advice() {
        let report = StatusReport {
            ready_bones: ReadyBones {
                count: 2,
                items: vec![BoneSummary {
                    id: "bd-abc".to_string(),
                    title: "test bone 1".to_string(),
                }],
            },
            workspaces: WorkspaceSummary {
                total: 1,
                active: 1,
                stale: 0,
            },
            inbox: InboxSummary { unread: 1 },
            agents: AgentsSummary { running: 1 },
            claims: ClaimsSummary { active: 1 },
            advice: vec![
                Advice {
                    severity: "HIGH".to_string(),
                    message: "Test high priority".to_string(),
                    command: Some("test-cmd".to_string()),
                },
                Advice {
                    severity: "INFO".to_string(),
                    message: "Test info".to_string(),
                    command: None,
                },
            ],
        };

        let json = serde_json::to_string_pretty(&report).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");

        // Verify advice array structure
        assert!(parsed.get("advice").is_some());
        let advice_array = parsed
            .get("advice")
            .unwrap()
            .as_array()
            .expect("should be array");
        assert_eq!(advice_array.len(), 2);
        assert_eq!(advice_array[0]["severity"], "HIGH");
        assert_eq!(advice_array[1]["severity"], "INFO");
    }

    #[test]
    fn advice_command_is_optional() {
        let adv_with_cmd = Advice {
            severity: "CRITICAL".to_string(),
            message: "Action required".to_string(),
            command: Some("cleanup-command".to_string()),
        };

        let adv_without_cmd = Advice {
            severity: "INFO".to_string(),
            message: "Informational only".to_string(),
            command: None,
        };

        let json_with = serde_json::to_value(&adv_with_cmd).expect("should serialize");
        let json_without = serde_json::to_value(&adv_without_cmd).expect("should serialize");

        assert!(json_with.get("command").is_some());
        // The command field may or may not be serialized when None due to skip_serializing_if
        // but if present should be null or omitted
        match json_without.get("command") {
            Some(cmd) => assert!(cmd.is_null()),
            None => {} // Also acceptable if field is completely omitted
        }
    }
}
