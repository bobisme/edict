//! Protocol cleanup command: check for held resources and suggest cleanup.
//!
//! Reads agent's active claims (from rite) and stale workspaces (from maw)
//! to produce cleanup guidance. Skips release commands for active bone claims.
//!
//! Exit policy: always exits 0 with status in stdout (clean or has-resources).
//! Operational failures (rite/maw unavailable) propagate as anyhow errors → exit 1.

use super::context::ProtocolContext;
use super::executor;
use super::exit_policy;
use super::render::{ProtocolGuidance, ProtocolStatus};
use super::shell;
use crate::commands::doctor::OutputFormat;

/// Execute cleanup protocol: check for held resources and output cleanup guidance.
///
/// Returns Ok(()) with guidance on stdout (exit 0) for all status outcomes.
/// ProtocolContext::collect errors propagate as anyhow::Error → exit 1.
///
/// When `execute` is true and status is HasResources, runs the cleanup steps
/// via the executor instead of outputting them as guidance.
pub fn execute(
    execute: bool,
    agent: &str,
    project: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    // Collect state from rite and maw
    let ctx = ProtocolContext::collect(project, agent)?;

    // Build guidance
    let mut guidance = ProtocolGuidance::new("cleanup");
    guidance.bone = None;
    guidance.workspace = None;
    guidance.review = None;

    // Analyze active claims
    let bone_claims = ctx.held_bone_claims();
    let workspace_claims = ctx.held_workspace_claims();

    // If no resources held, we're clean
    if bone_claims.is_empty() && workspace_claims.is_empty() {
        guidance.status = ProtocolStatus::Ready;
        guidance.advise("No cleanup needed.".to_string());
        // If execute is true but we're already clean, just report status (no execution needed)
        return render_cleanup(&guidance, format, execute);
    }

    // We have resources held
    guidance.status = ProtocolStatus::HasResources;

    // Build cleanup steps
    let mut steps = Vec::new();

    // Step 1: Post agent idle message
    steps.push(shell::rite_send_cmd(
        "agent",
        project,
        "Agent idle",
        "agent-idle",
    ));

    // Step 2: Clear statuses
    steps.push(shell::rite_statuses_clear_cmd("agent"));

    // Step 3: Release claims (but warn if bone claims are active)
    if !bone_claims.is_empty() {
        // Add diagnostic warning
        let bone_list = bone_claims
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        guidance.diagnostic(format!(
            "WARNING: Active bone claim(s) held: {}. Releasing these marks them as unowned in doing state.",
            bone_list
        ));
    }
    steps.push(shell::claims_release_all_cmd("agent"));

    guidance.steps(steps);

    // Build summary for advice
    let summary = format!(
        "Agent {} has {} bone claim(s) and {} workspace claim(s). \
         Run these commands to clean up and mark as idle.",
        agent,
        bone_claims.len(),
        workspace_claims.len()
    );
    guidance.advise(summary);

    render_cleanup(&guidance, format, execute)
}

/// Render cleanup guidance in the requested format.
///
/// For JSON format, delegates to the standard render path (exit_policy::render_guidance).
/// For text/pretty formats, uses cleanup-specific rendering optimized for
/// the cleanup use case (tab-delimited status, claim counts, etc.).
///
/// When execute is true and status is HasResources, runs the steps via the executor.
/// If execute is true but status is Ready (clean), just reports clean status.
///
/// All formats exit 0 — status is communicated via stdout content.
fn render_cleanup(
    guidance: &ProtocolGuidance,
    format: OutputFormat,
    execute: bool,
) -> anyhow::Result<()> {
    // If execute flag is set and we have resources to clean up, run the executor
    if execute && matches!(guidance.status, ProtocolStatus::HasResources) {
        let report = executor::execute_steps(&guidance.steps)?;
        let output = executor::render_report(&report, format);
        println!("{}", output);
        return Ok(());
    }

    // Otherwise, render guidance as usual (including when execute=true but status=Ready)
    match format {
        OutputFormat::Text => {
            // Text format: machine-readable, token-efficient
            let status_text = match guidance.status {
                ProtocolStatus::Ready => "clean",
                ProtocolStatus::HasResources => "has-resources",
                _ => "unknown",
            };
            println!("status\t{}", status_text);

            // Count claims if has-resources
            if matches!(guidance.status, ProtocolStatus::HasResources) {
                let claim_count = guidance
                    .diagnostics
                    .iter()
                    .find(|d| d.contains("Active bone claim"))
                    .map(|_| guidance.diagnostics.len())
                    .unwrap_or(0);
                println!("claims\t{} active", claim_count);
                println!();
                println!("Run these commands to clean up:");
                for step in &guidance.steps {
                    println!("  {}", step);
                }
            } else {
                println!("claims\t0 active");
                println!();
                println!("No cleanup needed.");
            }
            Ok(())
        }
        OutputFormat::Pretty => {
            // Pretty format: human-readable with formatting
            let status_text = match guidance.status {
                ProtocolStatus::Ready => "✓ clean",
                ProtocolStatus::HasResources => "⚠ has-resources",
                _ => "? unknown",
            };
            println!("Status: {}", status_text);

            if matches!(guidance.status, ProtocolStatus::HasResources) {
                println!();
                println!("Run these commands to clean up:");
                for step in &guidance.steps {
                    println!("  {}", step);
                }

                if !guidance.diagnostics.is_empty() {
                    println!();
                    println!("Warnings:");
                    for diagnostic in &guidance.diagnostics {
                        println!("  ⚠ {}", diagnostic);
                    }
                }
            } else {
                println!("No cleanup needed.");
            }

            if let Some(advice) = &guidance.advice {
                println!();
                println!("Notes: {}", advice);
            }
            Ok(())
        }
        OutputFormat::Json => {
            // JSON format: use standard render path for consistency
            exit_policy::render_guidance(guidance, format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleanup_status_clean() {
        // When no resources held, status should be Ready
        let mut guidance = ProtocolGuidance::new("cleanup");
        guidance.status = ProtocolStatus::Ready;
        guidance.advise("No cleanup needed.".to_string());

        assert_eq!(format!("{:?}", guidance.status), "Ready");
        assert!(guidance.steps.is_empty());
    }

    #[test]
    fn test_cleanup_status_has_resources() {
        // When resources held, status should be HasResources
        let mut guidance = ProtocolGuidance::new("cleanup");
        guidance.status = ProtocolStatus::HasResources;
        guidance.steps(vec![
            "rite send --agent test-agent test-project \"Agent idle\" -L agent-idle".to_string(),
            "rite statuses clear --agent test-agent".to_string(),
            "rite claims release --agent test-agent --all".to_string(),
        ]);

        assert_eq!(format!("{:?}", guidance.status), "HasResources");
        assert_eq!(guidance.steps.len(), 3);
        assert!(guidance.steps.iter().any(|s| s.contains("rite send")));
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("rite statuses clear"))
        );
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("rite claims release"))
        );
    }

    #[test]
    fn test_cleanup_warning_for_active_bones() {
        // When active bone claims exist, should add warning diagnostic
        let mut guidance = ProtocolGuidance::new("cleanup");
        guidance.diagnostic(
            "WARNING: Active bone claim(s) held: bd-3cqv. \
             Releasing these marks them as unowned in doing state."
                .to_string(),
        );

        assert!(guidance.diagnostics.iter().any(|d| d.contains("WARNING")));
        assert!(guidance.diagnostics.iter().any(|d| d.contains("bd-3cqv")));
    }
}
