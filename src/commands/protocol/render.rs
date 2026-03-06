//! Shell-safe command renderer and protocol guidance types.
//!
//! Renders protocol guidance with shell-safe commands, validation, and format support.

use crate::commands::doctor::OutputFormat;
use crate::commands::protocol::executor::ExecutionReport;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

// --- Core Types ---

/// A rendered protocol guidance output.
///
/// Provides a snapshot of agent state (bones, workspaces, reviews) with
/// next steps as shell commands agents can execute.
///
/// Freshness Semantics:
/// - `snapshot_at`: UTC timestamp when this guidance was generated
/// - `valid_for_sec`: How long this guidance remains fresh (in seconds)
/// - `revalidate_cmd`: If present, run this command to refresh guidance
///
/// Agents receiving stale guidance (snapshot_at + valid_for_sec < now) should
/// re-run the revalidate_cmd to get fresh state before executing steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolGuidance {
    /// Schema version for machine parsing
    pub schema: &'static str,
    /// Command type: "start", "finish", "review", "cleanup", "resume"
    pub command: &'static str,
    /// Status indicating readiness or blocker
    pub status: ProtocolStatus,
    /// UTC ISO 8601 snapshot timestamp
    pub snapshot_at: String,
    /// Validity duration in seconds (how long this guidance is fresh)
    pub valid_for_sec: u32,
    /// Command to re-fetch fresh guidance if stale (e.g., "edict protocol start")
    pub revalidate_cmd: Option<String>,
    /// Bone context (if applicable)
    pub bone: Option<BoneRef>,
    /// Workspace name (if applicable)
    pub workspace: Option<String>,
    /// Review context (if applicable)
    pub review: Option<ReviewRef>,
    /// Rendered shell commands (ready to copy-paste)
    pub steps: Vec<String>,
    /// Diagnostic messages if blocked or errored
    pub diagnostics: Vec<String>,
    /// Human-readable summary
    pub advice: Option<String>,
    /// Whether commands were executed (--execute mode)
    #[serde(default)]
    pub executed: bool,
    /// Execution report (if --execute was used)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_report: Option<ExecutionReport>,
}

impl ProtocolGuidance {
    /// Create a new guidance with ready status.
    /// Default freshness: 300 seconds (5 minutes)
    pub fn new(command: &'static str) -> Self {
        Self {
            schema: "protocol-guidance.v1",
            command,
            status: ProtocolStatus::Ready,
            snapshot_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            valid_for_sec: 300, // 5 minutes default
            revalidate_cmd: None,
            bone: None,
            workspace: None,
            review: None,
            steps: Vec::new(),
            diagnostics: Vec::new(),
            advice: None,
            executed: false,
            execution_report: None,
        }
    }

    /// Set the validity duration and optional revalidate command.
    /// Use this to control how long guidance remains fresh.
    pub fn set_freshness(&mut self, valid_for_sec: u32, revalidate_cmd: Option<String>) {
        self.valid_for_sec = valid_for_sec;
        self.revalidate_cmd = revalidate_cmd;
    }

    /// Add a step command.
    pub fn step(&mut self, cmd: String) {
        self.steps.push(cmd);
    }

    /// Add multiple steps.
    pub fn steps(&mut self, cmds: Vec<String>) {
        self.steps.extend(cmds);
    }

    /// Add a diagnostic message (e.g., reason for blocked status).
    pub fn diagnostic(&mut self, msg: String) {
        self.diagnostics.push(msg);
    }

    /// Set status and add corresponding diagnostics.
    pub fn blocked(&mut self, reason: String) {
        self.status = ProtocolStatus::Blocked;
        self.diagnostic(reason);
    }

    /// Set advice message (human-readable summary).
    pub fn advise(&mut self, msg: String) {
        self.advice = Some(msg);
    }
}

/// Protocol status indicating readiness, blockers, or next action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProtocolStatus {
    Ready,        // Commands are ready to run
    Blocked,      // Cannot proceed; diagnostics explain why
    Resumable,    // Work in progress; resume from previous state
    NeedsReview,  // Awaiting review approval
    HasResources, // Workspace/claims held
    Clean,        // No held resources
    HasWork,      // Ready bones available
    Fresh,        // Starting fresh (no prior state)
}

/// Bone reference in protocol output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneRef {
    pub id: String,
    pub title: String,
}

/// Review reference in protocol output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRef {
    pub review_id: String,
    pub status: String,
}

// --- Validation (from shell module) ---

use super::shell::{
    ValidationError, validate_bone_id, validate_review_id, validate_workspace_name,
};

/// Validate all dynamic values in a guidance before rendering.
pub fn validate_guidance(guidance: &ProtocolGuidance) -> Result<(), ValidationError> {
    if let Some(ref bone) = guidance.bone {
        validate_bone_id(&bone.id)?;
    }
    if let Some(ref ws) = guidance.workspace {
        validate_workspace_name(ws)?;
    }
    if let Some(ref review) = guidance.review {
        validate_review_id(&review.review_id)?;
    }
    Ok(())
}

// --- Rendering ---

/// Render guidance as human/agent-readable text.
///
/// Format:
/// ```text
/// Command: start
/// Status: Ready
/// Bone: bd-3t1d (protocol: shell-safe command renderer)
/// Workspace: brave-tiger
///
/// Steps:
/// 1. bus send --agent $AGENT edict 'Working...' -L task-claim
/// 2. maw ws create --random
///
/// Advice: Create workspace and stake claims before starting implementation.
/// ```
pub fn render_text(guidance: &ProtocolGuidance) -> String {
    let mut out = String::new();

    // Header
    writeln!(&mut out, "Command: {}", guidance.command).unwrap();
    writeln!(&mut out, "Status: {}", format_status(guidance.status)).unwrap();
    writeln!(
        &mut out,
        "Snapshot: {} (valid for {}s)",
        guidance.snapshot_at, guidance.valid_for_sec
    )
    .unwrap();

    if let Some(ref bone) = guidance.bone {
        writeln!(&mut out, "Bone: {} ({})", bone.id, bone.title).unwrap();
    }
    if let Some(ref ws) = guidance.workspace {
        writeln!(&mut out, "Workspace: {}", ws).unwrap();
    }
    if let Some(ref review) = guidance.review {
        writeln!(&mut out, "Review: {} ({})", review.review_id, review.status).unwrap();
    }

    if let Some(ref cmd) = guidance.revalidate_cmd {
        writeln!(&mut out, "Revalidate: {}", cmd).unwrap();
    }

    if !guidance.diagnostics.is_empty() {
        writeln!(&mut out).unwrap();
        writeln!(&mut out, "Diagnostics:").unwrap();
        for (i, diag) in guidance.diagnostics.iter().enumerate() {
            writeln!(&mut out, "  {}. {}", i + 1, diag).unwrap();
        }
    }

    // Show execution results if --execute was used
    if guidance.executed {
        if let Some(ref report) = guidance.execution_report {
            writeln!(&mut out).unwrap();
            writeln!(&mut out, "Execution:").unwrap();
            let exec_output = super::executor::render_report(report, OutputFormat::Text);
            for line in exec_output.lines() {
                writeln!(&mut out, "  {}", line).unwrap();
            }
        }
    } else if !guidance.steps.is_empty() {
        // Show steps only if not executed
        writeln!(&mut out).unwrap();
        writeln!(&mut out, "Steps:").unwrap();
        for (i, step) in guidance.steps.iter().enumerate() {
            writeln!(&mut out, "  {}. {}", i + 1, step).unwrap();
        }
    }

    if let Some(ref advice) = guidance.advice {
        writeln!(&mut out).unwrap();
        writeln!(&mut out, "Advice: {}", advice).unwrap();
    }

    out
}

/// Render guidance as JSON with schema version and structured data.
pub fn render_json(guidance: &ProtocolGuidance) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string_pretty(guidance)?;
    Ok(json)
}

/// Render guidance as colored TTY output (for humans).
///
/// Uses ANSI color codes for status, headers, and command highlighting.
pub fn render_pretty(guidance: &ProtocolGuidance) -> String {
    let mut out = String::new();

    // Color codes
    let reset = "\x1b[0m";
    let bold = "\x1b[1m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
    let red = "\x1b[31m";

    // Status color
    let status_color = match guidance.status {
        ProtocolStatus::Ready | ProtocolStatus::Clean | ProtocolStatus::Fresh => green,
        ProtocolStatus::Blocked | ProtocolStatus::HasWork => red,
        _ => yellow,
    };

    // Header
    writeln!(&mut out, "{}Command:{} {}", bold, reset, guidance.command).unwrap();
    writeln!(
        &mut out,
        "{}Status:{} {}{}{}\n",
        bold,
        reset,
        status_color,
        format_status(guidance.status),
        reset
    )
    .unwrap();

    writeln!(
        &mut out,
        "{}Snapshot:{} {} (valid for {}s)",
        bold, reset, guidance.snapshot_at, guidance.valid_for_sec
    )
    .unwrap();

    if let Some(ref bone) = guidance.bone {
        writeln!(
            &mut out,
            "{}Bone:{} {} ({})",
            bold, reset, bone.id, bone.title
        )
        .unwrap();
    }
    if let Some(ref ws) = guidance.workspace {
        writeln!(&mut out, "{}Workspace:{} {}", bold, reset, ws).unwrap();
    }
    if let Some(ref review) = guidance.review {
        writeln!(
            &mut out,
            "{}Review:{} {} ({})",
            bold, reset, review.review_id, review.status
        )
        .unwrap();
    }

    if let Some(ref cmd) = guidance.revalidate_cmd {
        writeln!(&mut out, "{}Revalidate:{} {}", bold, reset, cmd).unwrap();
    }

    if !guidance.diagnostics.is_empty() {
        writeln!(&mut out, "\n{}Diagnostics:{}", bold, reset).unwrap();
        for diag in &guidance.diagnostics {
            writeln!(&mut out, "  {}{}{}", red, diag, reset).unwrap();
        }
    }

    // Show execution results if --execute was used
    if guidance.executed {
        if let Some(ref report) = guidance.execution_report {
            writeln!(&mut out, "\n{}Execution:{}", bold, reset).unwrap();
            let exec_output = super::executor::render_report(report, OutputFormat::Pretty);
            for line in exec_output.lines() {
                writeln!(&mut out, "  {}", line).unwrap();
            }
        }
    } else if !guidance.steps.is_empty() {
        // Show steps only if not executed
        writeln!(&mut out, "\n{}Steps:{}", bold, reset).unwrap();
        for (i, step) in guidance.steps.iter().enumerate() {
            writeln!(&mut out, "  {}. {}", i + 1, step).unwrap();
        }
    }

    if let Some(ref advice) = guidance.advice {
        writeln!(&mut out, "\n{}Advice:{} {}", bold, reset, advice).unwrap();
    }

    out
}

/// Format status as human-readable string.
fn format_status(status: ProtocolStatus) -> &'static str {
    match status {
        ProtocolStatus::Ready => "Ready",
        ProtocolStatus::Blocked => "Blocked",
        ProtocolStatus::Resumable => "Resumable",
        ProtocolStatus::NeedsReview => "Needs Review",
        ProtocolStatus::HasResources => "Has Resources",
        ProtocolStatus::Clean => "Clean",
        ProtocolStatus::HasWork => "Has Work",
        ProtocolStatus::Fresh => "Fresh",
    }
}

/// Render guidance using the specified format.
pub fn render(guidance: &ProtocolGuidance, format: OutputFormat) -> Result<String, String> {
    // Validate before rendering
    validate_guidance(guidance).map_err(|e| e.to_string())?;

    Ok(match format {
        OutputFormat::Json => render_json(guidance).map_err(|e| e.to_string())?,
        OutputFormat::Pretty => render_pretty(guidance),
        OutputFormat::Text => render_text(guidance),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::protocol::executor::StepResult;

    // --- ProtocolGuidance builder tests ---

    #[test]
    fn guidance_new_start() {
        let g = ProtocolGuidance::new("start");
        assert_eq!(g.command, "start");
        assert_eq!(g.status, ProtocolStatus::Ready);
        assert_eq!(g.steps.len(), 0);
    }

    #[test]
    fn guidance_add_step() {
        let mut g = ProtocolGuidance::new("start");
        g.step("echo hello".to_string());
        assert_eq!(g.steps.len(), 1);
        assert_eq!(g.steps[0], "echo hello");
    }

    #[test]
    fn guidance_add_multiple_steps() {
        let mut g = ProtocolGuidance::new("finish");
        g.steps(vec![
            "cmd1".to_string(),
            "cmd2".to_string(),
            "cmd3".to_string(),
        ]);
        assert_eq!(g.steps.len(), 3);
    }

    #[test]
    fn guidance_blocked() {
        let mut g = ProtocolGuidance::new("start");
        g.blocked("workspace not found".to_string());
        assert_eq!(g.status, ProtocolStatus::Blocked);
        assert_eq!(g.diagnostics.len(), 1);
        assert!(g.diagnostics[0].contains("workspace"));
    }

    #[test]
    fn guidance_with_bone_and_workspace() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "protocol: shell-safe command renderer".to_string(),
        });
        g.workspace = Some("brave-tiger".to_string());
        assert!(g.bone.is_some());
        assert!(g.workspace.is_some());
    }

    #[test]
    fn guidance_with_review() {
        let mut g = ProtocolGuidance::new("review");
        g.review = Some(ReviewRef {
            review_id: "cr-abc1".to_string(),
            status: "open".to_string(),
        });
        assert!(g.review.is_some());
    }

    #[test]
    fn guidance_with_advice() {
        let mut g = ProtocolGuidance::new("start");
        g.advise("Create workspace and stake claims first.".to_string());
        assert!(g.advice.is_some());
        assert!(g.advice.as_ref().unwrap().contains("workspace"));
    }

    // --- Validation tests ---

    #[test]
    fn validate_guidance_valid() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "test".to_string(),
        });
        g.workspace = Some("brave-tiger".to_string());
        assert!(validate_guidance(&g).is_ok());
    }

    #[test]
    fn validate_guidance_invalid_bead_id() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "has spaces; rm -rf".to_string(),
            title: "test".to_string(),
        });
        assert!(validate_guidance(&g).is_err());
    }

    #[test]
    fn validate_guidance_invalid_workspace_name() {
        let mut g = ProtocolGuidance::new("start");
        g.workspace = Some("invalid name".to_string());
        assert!(validate_guidance(&g).is_err());
    }

    // --- Rendering tests ---

    #[test]
    fn render_text_minimal() {
        let g = ProtocolGuidance::new("start");
        let text = render_text(&g);
        assert!(text.contains("Command: start"));
        assert!(text.contains("Status: Ready"));
    }

    #[test]
    fn render_text_with_bead_and_steps() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "Test feature".to_string(),
        });
        g.step("echo step 1".to_string());
        g.step("echo step 2".to_string());

        let text = render_text(&g);
        assert!(text.contains("Bone: bd-abc (Test feature)"));
        assert!(text.contains("Steps:"));
        assert!(text.contains("echo step 1"));
        assert!(text.contains("echo step 2"));
    }

    #[test]
    fn render_text_with_diagnostics() {
        let mut g = ProtocolGuidance::new("finish");
        g.blocked("review not approved".to_string());
        g.diagnostic("waiting for LGTM".to_string());

        let text = render_text(&g);
        assert!(text.contains("Status: Blocked"));
        assert!(text.contains("Diagnostics:"));
        assert!(text.contains("review not approved"));
        assert!(text.contains("waiting for LGTM"));
    }

    #[test]
    fn render_text_with_advice() {
        let mut g = ProtocolGuidance::new("cleanup");
        g.advise("Run the cleanup steps to release held resources.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Advice:"));
        assert!(text.contains("cleanup steps"));
    }

    #[test]
    fn render_json_valid() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-xyz".to_string(),
            title: "Feature".to_string(),
        });
        g.step("echo test".to_string());

        let json = render_json(&g).unwrap();
        assert!(json.contains("schema"));
        assert!(json.contains("protocol-guidance.v1"));
        assert!(json.contains("\"command\": \"start\"") || json.contains("\"command\":\"start\""));
        assert!(json.contains("bd-xyz"));
        assert!(json.contains("steps"));
        assert!(json.contains("echo test"));
    }

    #[test]
    fn render_pretty_has_colors() {
        let g = ProtocolGuidance::new("start");
        let pretty = render_pretty(&g);
        // Should have ANSI color codes
        assert!(pretty.contains("\x1b["));
    }

    #[test]
    fn render_pretty_ready_status_is_green() {
        let g = ProtocolGuidance::new("start");
        let pretty = render_pretty(&g);
        // Green code appears before Ready
        assert!(pretty.contains("\x1b[32m")); // green
    }

    #[test]
    fn render_pretty_blocked_status_is_red() {
        let mut g = ProtocolGuidance::new("start");
        g.blocked("error".to_string());
        let pretty = render_pretty(&g);
        // Red code appears in output
        assert!(pretty.contains("\x1b[31m")); // red
    }

    // --- Integration tests (golden-style) ---

    #[test]
    fn golden_start_workflow() {
        // Typical start workflow
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "protocol: shell-safe command renderer".to_string(),
        });
        g.workspace = Some("brave-tiger".to_string());
        g.steps(vec![
            "maw exec default -- bn do bd-3t1d".to_string(),
            "bus claims stake --agent crimson-storm 'bone://edict/bd-3t1d' -m 'bd-3t1d'"
                .to_string(),
            "maw ws create --random".to_string(),
            "bus claims stake --agent crimson-storm 'workspace://edict/brave-tiger' -m 'bd-3t1d'"
                .to_string(),
        ]);
        g.advise("Workspace created. Implement render.rs with ProtocolGuidance, ProtocolStatus, and rendering functions.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Command: start"));
        assert!(text.contains("brave-tiger"));
        assert!(text.contains("3. maw ws create --random"));
        assert!(text.contains("Advice:"));
    }

    #[test]
    fn golden_blocked_workflow() {
        // Blocked due to claim conflict
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "protocol: shell-safe command renderer".to_string(),
        });
        g.blocked("bone already claimed by another agent".to_string());
        g.diagnostic("Check: bus claims list --format json".to_string());

        let text = render_text(&g);
        assert!(text.contains("Status: Blocked"));
        assert!(text.contains("already claimed"));
        assert!(text.contains("bus claims list"));
    }

    #[test]
    fn golden_review_workflow() {
        // Review requested and pending approval
        let mut g = ProtocolGuidance::new("review");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "protocol: shell-safe command renderer".to_string(),
        });
        g.workspace = Some("brave-tiger".to_string());
        g.review = Some(ReviewRef {
            review_id: "cr-123".to_string(),
            status: "open".to_string(),
        });
        g.status = ProtocolStatus::NeedsReview;
        g.steps(vec![
            "maw exec brave-tiger -- seal reviews request cr-123 --reviewers edict-security --agent crimson-storm".to_string(),
            "bus send --agent crimson-storm edict 'Review requested: cr-123 @edict-security' -L review-request".to_string(),
        ]);
        g.advise("Review is open. Awaiting approval from edict-security.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Status: Needs Review"));
        assert!(text.contains("cr-123"));
        assert!(text.contains("edict-security"));
    }

    #[test]
    fn golden_cleanup_workflow() {
        // Release all held claims
        let mut g = ProtocolGuidance::new("cleanup");
        g.status = ProtocolStatus::Clean;
        g.steps(vec![
            "bus claims list --agent crimson-storm --mine --format json".to_string(),
            "bus claims release --agent crimson-storm --all".to_string(),
        ]);
        g.advise("All held resources released.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Command: cleanup"));
        assert!(text.contains("bus claims release") && text.contains("--all"));
        assert!(text.contains("Clean"));
    }

    #[test]
    fn status_serialization() {
        // Statuses should serialize to PascalCase
        let json = serde_json::to_string(&ProtocolStatus::Ready).unwrap();
        assert_eq!(json, "\"Ready\"");

        let json = serde_json::to_string(&ProtocolStatus::NeedsReview).unwrap();
        assert_eq!(json, "\"NeedsReview\"");

        let json = serde_json::to_string(&ProtocolStatus::HasWork).unwrap();
        assert_eq!(json, "\"HasWork\"");
    }

    #[test]
    fn guidance_json_roundtrip() {
        let mut original = ProtocolGuidance::new("start");
        original.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "test".to_string(),
        });
        original.steps = vec!["echo hello".to_string()];

        // Serialize and verify JSON contains expected fields
        let json = render_json(&original).unwrap();
        assert!(json.contains("command"));
        assert!(json.contains("start"));
        assert!(json.contains("bd-abc"));
        assert!(json.contains("echo hello"));

        // Verify it's valid JSON that can be parsed
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["command"].as_str(), Some("start"));
        assert_eq!(parsed["status"].as_str(), Some("Ready"));
    }

    #[test]
    fn snapshot_at_is_rfc3339() {
        let g = ProtocolGuidance::new("start");
        // Should be ISO 8601 / RFC 3339 format
        assert!(g.snapshot_at.contains("T"));
        assert!(g.snapshot_at.contains("Z") || g.snapshot_at.contains("+"));
    }

    // --- Freshness Semantics Tests ---

    #[test]
    fn guidance_default_freshness() {
        let g = ProtocolGuidance::new("start");
        assert_eq!(g.valid_for_sec, 300); // 5 minutes default
        assert!(g.revalidate_cmd.is_none());
    }

    #[test]
    fn guidance_set_freshness() {
        let mut g = ProtocolGuidance::new("start");
        g.set_freshness(600, Some("edict protocol start".to_string()));
        assert_eq!(g.valid_for_sec, 600);
        assert_eq!(g.revalidate_cmd, Some("edict protocol start".to_string()));
    }

    #[test]
    fn render_text_includes_freshness() {
        let mut g = ProtocolGuidance::new("start");
        g.set_freshness(300, Some("edict protocol start".to_string()));
        let text = render_text(&g);
        assert!(text.contains("Snapshot:"));
        assert!(text.contains("valid for 300s"));
        assert!(text.contains("Revalidate: edict protocol start"));
    }

    #[test]
    fn render_json_includes_freshness() {
        let mut g = ProtocolGuidance::new("start");
        g.set_freshness(600, Some("edict protocol start".to_string()));
        let json = render_json(&g).unwrap();
        assert!(json.contains("valid_for_sec"));
        assert!(json.contains("600"));
        assert!(json.contains("revalidate_cmd"));
    }

    #[test]
    fn guidance_stale_window_logic() {
        // This test demonstrates how clients should detect stale guidance.
        let mut g = ProtocolGuidance::new("start");
        g.set_freshness(1, Some("edict protocol start".to_string())); // 1 second fresh

        let guidance_json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&guidance_json).unwrap();

        let snapshot_str = parsed["snapshot_at"].as_str().unwrap();
        let valid_for_sec = parsed["valid_for_sec"].as_u64().unwrap();
        let revalidate_cmd = parsed["revalidate_cmd"].as_str();

        assert!(!snapshot_str.is_empty());
        assert_eq!(valid_for_sec, 1);
        assert!(revalidate_cmd.is_some());
    }

    // --- Golden Schema Tests: Contract Stability ---

    #[test]
    fn golden_schema_version_is_stable() {
        let g = ProtocolGuidance::new("start");
        assert_eq!(g.schema, "protocol-guidance.v1");
    }

    #[test]
    fn golden_status_variants_are_complete() {
        let _statuses = vec![
            ProtocolStatus::Ready,
            ProtocolStatus::Blocked,
            ProtocolStatus::Resumable,
            ProtocolStatus::NeedsReview,
            ProtocolStatus::HasResources,
            ProtocolStatus::Clean,
            ProtocolStatus::HasWork,
            ProtocolStatus::Fresh,
        ];
        assert_eq!(_statuses.len(), 8);
    }

    #[test]
    fn golden_guidance_json_structure() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "test".to_string(),
        });
        g.workspace = Some("test-ws".to_string());
        g.step("echo test".to_string());
        g.diagnostic("info".to_string());

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("schema").is_some(), "schema field missing");
        assert!(parsed.get("command").is_some(), "command field missing");
        assert!(parsed.get("status").is_some(), "status field missing");
        assert!(
            parsed.get("snapshot_at").is_some(),
            "snapshot_at field missing"
        );
        assert!(
            parsed.get("valid_for_sec").is_some(),
            "valid_for_sec field missing"
        );
        assert!(parsed.get("steps").is_some(), "steps field missing");
        assert!(
            parsed.get("diagnostics").is_some(),
            "diagnostics field missing"
        );
    }

    #[test]
    fn golden_minimal_guidance_json() {
        let g = ProtocolGuidance::new("cleanup");
        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["schema"].as_str(), Some("protocol-guidance.v1"));
        assert_eq!(parsed["command"].as_str(), Some("cleanup"));
        assert_eq!(parsed["status"].as_str(), Some("Ready"));
        assert!(parsed["snapshot_at"].is_string());
        assert_eq!(parsed["valid_for_sec"].as_u64(), Some(300));
        assert!(parsed["steps"].is_array());
        assert!(parsed["diagnostics"].is_array());
    }

    #[test]
    fn golden_full_guidance_json() {
        let mut g = ProtocolGuidance::new("review");
        g.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "Feature X".to_string(),
        });
        g.workspace = Some("worker-1".to_string());
        g.review = Some(ReviewRef {
            review_id: "cr-123".to_string(),
            status: "open".to_string(),
        });
        g.set_freshness(600, Some("edict protocol review".to_string()));
        g.step("maw exec worker-1 -- seal reviews request cr-123 --reviewers edict-security --agent crimson-storm".to_string());
        g.diagnostic("awaiting review approval".to_string());
        g.advise("Review is pending.".to_string());

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed["bone"]["id"].as_str().is_some());
        assert!(parsed["workspace"].as_str().is_some());
        assert!(parsed["review"]["review_id"].as_str().is_some());
        assert_eq!(parsed["valid_for_sec"].as_u64(), Some(600));
        assert!(parsed["revalidate_cmd"].as_str().is_some());
        assert!(!parsed["steps"].as_array().unwrap().is_empty());
        assert!(!parsed["diagnostics"].as_array().unwrap().is_empty());
        assert!(parsed["advice"].is_string());
    }

    #[test]
    fn golden_text_render_includes_all_fields() {
        let mut g = ProtocolGuidance::new("start");
        g.bone = Some(BoneRef {
            id: "bd-3t1d".to_string(),
            title: "protocol: renderer".to_string(),
        });
        g.workspace = Some("work-1".to_string());
        g.set_freshness(300, Some("edict protocol start".to_string()));
        g.step("maw ws create work-1".to_string());
        g.advise("Start implementation".to_string());

        let text = render_text(&g);

        assert!(text.contains("Command:"), "Command field missing");
        assert!(text.contains("Status:"), "Status field missing");
        assert!(text.contains("Snapshot:"), "Snapshot field missing");
        assert!(text.contains("Bone:"), "Bone field missing");
        assert!(text.contains("Workspace:"), "Workspace field missing");
        assert!(text.contains("Revalidate:"), "Revalidate field missing");
        assert!(text.contains("Steps:"), "Steps field missing");
        assert!(text.contains("Advice:"), "Advice field missing");
    }

    #[test]
    fn golden_compatibility_additive_only() {
        let g = ProtocolGuidance::new("start");

        let _schema = g.schema;
        let _command = g.command;
        let _status = g.status;
        let _snapshot_at = g.snapshot_at;
        let _valid_for_sec = g.valid_for_sec;
        let _steps = g.steps;
        let _diagnostics = g.diagnostics;

        assert!(g.bone.is_none());
        assert!(g.workspace.is_none());
        assert!(g.review.is_none());
        assert!(g.revalidate_cmd.is_none());
        assert!(g.advice.is_none());
    }

    // --- Status Rendering Tests: All Variants ---

    #[test]
    fn render_text_status_resumable() {
        let mut g = ProtocolGuidance::new("resume");
        g.status = ProtocolStatus::Resumable;
        g.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "In progress task".to_string(),
        });
        g.advise("Resume from previous work state.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Status: Resumable"));
        assert!(text.contains("bd-abc"));
        assert!(text.contains("resume"));
    }

    #[test]
    fn render_json_status_resumable() {
        let mut g = ProtocolGuidance::new("resume");
        g.status = ProtocolStatus::Resumable;
        g.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "In progress".to_string(),
        });

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"].as_str(), Some("Resumable"));
        assert_eq!(parsed["command"].as_str(), Some("resume"));
    }

    #[test]
    fn render_text_status_has_resources() {
        let mut g = ProtocolGuidance::new("cleanup");
        g.status = ProtocolStatus::HasResources;
        g.steps(vec!["bus claims list --agent $AGENT --mine".to_string()]);

        let text = render_text(&g);
        assert!(text.contains("Status: Has Resources"));
        assert!(text.contains("bus claims list"));
    }

    #[test]
    fn render_json_status_has_resources() {
        let mut g = ProtocolGuidance::new("cleanup");
        g.status = ProtocolStatus::HasResources;

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"].as_str(), Some("HasResources"));
    }

    #[test]
    fn render_text_status_has_work() {
        let mut g = ProtocolGuidance::new("start");
        g.status = ProtocolStatus::HasWork;
        g.steps(vec!["maw exec default -- bn next".to_string()]);

        let text = render_text(&g);
        assert!(text.contains("Status: Has Work"));
    }

    #[test]
    fn render_json_status_has_work() {
        let mut g = ProtocolGuidance::new("start");
        g.status = ProtocolStatus::HasWork;

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"].as_str(), Some("HasWork"));
    }

    #[test]
    fn render_text_status_fresh() {
        let mut g = ProtocolGuidance::new("start");
        g.status = ProtocolStatus::Fresh;
        g.advise("Starting fresh with no prior state.".to_string());

        let text = render_text(&g);
        assert!(text.contains("Status: Fresh"));
        assert!(text.contains("Fresh"));
    }

    #[test]
    fn render_json_status_fresh() {
        let mut g = ProtocolGuidance::new("start");
        g.status = ProtocolStatus::Fresh;

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"].as_str(), Some("Fresh"));
    }

    // --- Execution Report Rendering Tests ---

    #[test]
    fn render_text_with_execution_report_success() {
        let mut g = ProtocolGuidance::new("start");
        g.executed = true;
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "echo test".to_string(),
                success: true,
                stdout: "test".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        });

        let text = render_text(&g);
        assert!(text.contains("Execution:"));
        assert!(text.contains("ok"));
    }

    #[test]
    fn render_json_with_execution_report() {
        let mut g = ProtocolGuidance::new("start");
        g.executed = true;
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "echo hello".to_string(),
                success: true,
                stdout: "hello".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        });

        let json = render_json(&g).unwrap();
        assert!(json.contains("executed"));
        assert!(json.contains("true"));
        assert!(json.contains("execution_report"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn render_pretty_with_execution_report() {
        let mut g = ProtocolGuidance::new("start");
        g.executed = true;
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "echo test".to_string(),
                success: true,
                stdout: "test".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        });

        let pretty = render_pretty(&g);
        assert!(pretty.contains("Execution:"));
    }

    #[test]
    fn render_text_execution_report_with_failure() {
        let mut g = ProtocolGuidance::new("finish");
        g.executed = true;
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "maw ws merge --destroy".to_string(),
                success: false,
                stdout: String::new(),
                stderr: "error: workspace not found".to_string(),
            }],
            remaining: vec!["next step".to_string()],
        });

        let text = render_text(&g);
        assert!(text.contains("Execution:"));
        assert!(text.contains("FAILED"));
    }

    #[test]
    fn render_json_execution_report_with_remaining_steps() {
        let mut g = ProtocolGuidance::new("finish");
        g.executed = true;
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "step1".to_string(),
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            }],
            remaining: vec!["step2".to_string(), "step3".to_string()],
        });

        let json = render_json(&g).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["execution_report"].is_object());
        let report = &parsed["execution_report"];
        assert!(report["results"].is_array());
        assert!(report["remaining"].is_array());
        assert_eq!(report["remaining"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn render_text_without_execution_report_shows_steps() {
        let mut g = ProtocolGuidance::new("start");
        g.executed = false;
        g.step("echo hello".to_string());
        g.step("echo world".to_string());

        let text = render_text(&g);
        // When not executed, should show steps, not execution report
        assert!(text.contains("Steps:"));
        assert!(text.contains("echo hello"));
        assert!(!text.contains("Execution:"));
    }

    #[test]
    fn render_pretty_executed_true_skips_steps_section() {
        let mut g = ProtocolGuidance::new("start");
        g.executed = true;
        g.step("echo hello".to_string());
        g.execution_report = Some(ExecutionReport {
            results: vec![StepResult {
                command: "echo hello".to_string(),
                success: true,
                stdout: "hello".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        });

        let pretty = render_pretty(&g);
        // When executed, should show execution report instead of steps section
        assert!(pretty.contains("Execution:"));
    }
}
