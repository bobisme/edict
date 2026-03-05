//! Protocol finish command: check state and output commands to finish a bone.
//!
//! Validates bone claim ownership, resolves workspace from claims, checks review
//! gate status, and outputs the appropriate shell commands depending on whether
//! the review is approved, blocked, or needs review.

use super::context::ProtocolContext;
use super::executor;
use super::render::{self, BoneRef, ProtocolGuidance, ProtocolStatus, ReviewRef};
use super::review_gate::{self, ReviewGateStatus};
use super::shell;
use crate::commands::doctor::OutputFormat;
use crate::config::Config;

/// Execute the finish protocol command.
pub fn execute(
    bone_id: &str,
    no_merge: bool,
    force: bool,
    execute: bool,
    agent: &str,
    project: &str,
    config: &Config,
    format: OutputFormat,
) -> anyhow::Result<()> {
    // Collect state from bus and maw
    let ctx = match ProtocolContext::collect(project, agent) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut guidance = ProtocolGuidance::new("finish");
            guidance.blocked(format!("failed to collect state: {}", e));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    // Fetch bone info
    let bone_info = match ctx.bone_status(bone_id) {
        Ok(bone) => bone,
        Err(_) => {
            let mut guidance = ProtocolGuidance::new("finish");
            guidance.blocked(format!(
                "bone {} not found. Check the ID with: maw exec default -- bn show {}",
                bone_id, bone_id
            ));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    let mut guidance = ProtocolGuidance::new("finish");
    guidance.bone = Some(BoneRef {
        id: bone_id.to_string(),
        title: bone_info.title.clone(),
    });
    guidance.set_freshness(120, Some(format!("edict protocol finish {}", bone_id)));

    // Check bone is already closed
    if bone_info.state == "done" {
        guidance.blocked("bone is already done".to_string());
        print_guidance(&guidance, format)?;
        return Ok(());
    }

    // Check agent holds bone claim
    let held_bone_claims = ctx.held_bone_claims();
    let holds_claim = held_bone_claims.iter().any(|(id, _)| *id == bone_id);

    if !holds_claim {
        guidance.blocked(format!(
            "agent '{}' does not hold a claim for bone {}. \
             Check with: bus claims list --agent {} --format json",
            agent, bone_id, agent
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    }

    // Resolve workspace from claims
    let workspace = match ctx.workspace_for_bone(bone_id) {
        Some(ws) => ws.to_string(),
        None => {
            guidance.blocked(format!(
                "no workspace claim found for bone {}. \
                 Cannot determine which workspace to merge.",
                bone_id
            ));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };
    guidance.workspace = Some(workspace.clone());

    // Build required reviewers list from config: "{project}-{role}"
    let required_reviewers: Vec<String> = config
        .review
        .reviewers
        .iter()
        .map(|role| format!("{}-{}", project, role))
        .collect();

    // Check review gate status
    let review_enabled = config.review.enabled && !required_reviewers.is_empty();

    if review_enabled && !force {
        // Try to find a review for this workspace
        match find_review_for_workspace(&ctx, &workspace) {
            Some((review_id, review_detail)) => {
                let decision =
                    review_gate::evaluate_review_gate(&review_detail, &required_reviewers);
                guidance.review = Some(ReviewRef {
                    review_id: review_id.clone(),
                    status: decision.status_str().to_string(),
                });

                match decision.status {
                    ReviewGateStatus::Approved => {
                        // Ready to finish
                        guidance.status = ProtocolStatus::Ready;
                        build_finish_steps(
                            &mut guidance,
                            bone_id,
                            &bone_info.title,
                            project,
                            &workspace,
                            Some(&review_id),
                            no_merge,
                        );

                        // Execute if --execute flag is set
                        if execute {
                            return execute_and_render(&guidance, format);
                        }

                        guidance.advise(format!(
                            "Review {} approved. Run these commands to finish bone {}.",
                            review_id, bone_id
                        ));
                    }
                    ReviewGateStatus::Blocked => {
                        // Blocked by reviewer
                        guidance.status = ProtocolStatus::Blocked;
                        guidance.diagnostic(format!(
                            "Review {} is blocked by: {}",
                            review_id,
                            decision.blocked_by.join(", ")
                        ));
                        if decision.open_thread_count_hint(&review_detail) > 0 {
                            guidance.diagnostic(format!(
                                "{} open thread(s) need resolution",
                                review_detail.open_thread_count
                            ));
                        }

                        // Output commands to check review feedback and re-request
                        let mut steps = Vec::new();
                        steps.push(shell::crit_show_cmd(&workspace, &review_id));
                        steps.push(shell::crit_request_cmd(
                            &workspace,
                            &review_id,
                            &required_reviewers.join(","),
                            "agent",
                        ));
                        steps.push(shell::bus_send_cmd(
                            "agent",
                            project,
                            &format!(
                                "Review re-requested: {} @{}",
                                review_id,
                                required_reviewers.join(" @")
                            ),
                            "review-request",
                        ));
                        guidance.steps(steps);
                        guidance.advise(format!(
                            "Review {} is blocked. Address reviewer feedback, then re-request review.",
                            review_id
                        ));
                    }
                    ReviewGateStatus::NeedsReview => {
                        // Review exists but not all reviewers have voted
                        guidance.status = ProtocolStatus::NeedsReview;

                        if !decision.missing_approvals.is_empty() {
                            guidance.diagnostic(format!(
                                "Awaiting votes from: {}",
                                decision.missing_approvals.join(", ")
                            ));
                        }

                        let mut steps = Vec::new();
                        steps.push(shell::crit_show_cmd(&workspace, &review_id));
                        // Re-request from missing reviewers
                        if !decision.missing_approvals.is_empty() {
                            steps.push(shell::crit_request_cmd(
                                &workspace,
                                &review_id,
                                &decision.missing_approvals.join(","),
                                "agent",
                            ));
                            let mentions: Vec<String> = decision
                                .missing_approvals
                                .iter()
                                .map(|r| format!("@{}", r))
                                .collect();
                            steps.push(shell::bus_send_cmd(
                                "agent",
                                project,
                                &format!("Review pending: {} {}", review_id, mentions.join(" ")),
                                "review-request",
                            ));
                        }
                        guidance.steps(steps);
                        guidance.advise(format!(
                            "Review {} needs approval. Wait for reviewers or re-request.",
                            review_id
                        ));
                    }
                }
            }
            None => {
                // No review found — needs review creation
                guidance.status = ProtocolStatus::NeedsReview;
                guidance.diagnostic("No review found for this workspace.".to_string());

                let mut steps = Vec::new();
                steps.push(shell::crit_create_cmd(
                    &workspace,
                    "agent",
                    &bone_info.title,
                    &required_reviewers.join(","),
                ));
                let mentions: Vec<String> = required_reviewers
                    .iter()
                    .map(|r| format!("@{}", r))
                    .collect();
                steps.push(shell::bus_send_cmd(
                    "agent",
                    project,
                    &format!("Review requested: <review-id> {}", mentions.join(" ")),
                    "review-request",
                ));
                guidance.steps(steps);
                guidance.advise(
                    "No review exists yet. Create one and request reviewers before finishing."
                        .to_string(),
                );
            }
        }
    } else {
        // Review not enabled, or --force flag used
        guidance.status = ProtocolStatus::Ready;

        if force && review_enabled {
            guidance.diagnostic("WARNING: --force flag used, bypassing review gate.".to_string());
        }

        build_finish_steps(
            &mut guidance,
            bone_id,
            &bone_info.title,
            project,
            &workspace,
            None,
            no_merge,
        );

        // Execute if --execute flag is set
        if execute {
            return execute_and_render(&guidance, format);
        }

        if force && review_enabled {
            guidance.advise(format!(
                "Force-finishing bone {} without review approval. Run these commands to finish.",
                bone_id
            ));
        } else {
            guidance.advise(format!(
                "Review not required. Run these commands to finish bone {}.",
                bone_id
            ));
        }
    }

    print_guidance(&guidance, format)?;
    Ok(())
}

/// Build the standard finish steps: commit workspace, merge, close, announce,
/// release claims.
fn build_finish_steps(
    guidance: &mut ProtocolGuidance,
    bone_id: &str,
    bead_title: &str,
    project: &str,
    workspace: &str,
    review_id: Option<&str>,
    no_merge: bool,
) {
    let mut steps = Vec::new();

    // 1. Stage workspace changes
    steps.push(format!("maw exec {} -- git add -A", workspace,));

    // 2. Commit workspace changes
    steps.push(format!(
        "maw exec {} -- git commit -m {}",
        workspace,
        shell::shell_escape(&format!(
            "{}: {}\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
            bone_id, bead_title
        ))
    ));

    // 3. Merge workspace (unless --no-merge)
    if !no_merge {
        // Use a conventional commit message derived from the bone title
        let merge_msg = format!("feat: {}", bead_title);
        steps.push(shell::ws_merge_cmd(workspace, &merge_msg));
    }

    // 4. Mark review as merged (if review exists)
    if let Some(rid) = review_id {
        steps.push(format!(
            "maw exec default -- crit reviews mark-merged {}",
            rid
        ));
    }

    // 5. Close the bone
    steps.push(shell::bn_done_cmd(
        bone_id,
        &format!("Completed in workspace {}", workspace),
    ));

    // 6. Announce completion on bus
    steps.push(shell::bus_send_cmd(
        "agent",
        project,
        &format!("Finished {}: {}", bone_id, bead_title),
        "task-done",
    ));

    // 7. Release all claims
    steps.push(shell::claims_release_all_cmd("agent"));

    guidance.steps(steps);
}

/// Try to find a review for a workspace by listing reviews in that workspace.
fn find_review_for_workspace(
    ctx: &ProtocolContext,
    workspace: &str,
) -> Option<(String, super::adapters::ReviewDetail)> {
    // List reviews in the workspace via crit reviews list
    let output = std::process::Command::new("maw")
        .args([
            "exec", workspace, "--", "crit", "reviews", "list", "--format", "json",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let reviews_resp = super::adapters::parse_reviews_list(&stdout).ok()?;

    // Find the first open/reviewed review (not merged)
    for review_summary in &reviews_resp.reviews {
        if review_summary.status != "merged" {
            // Fetch full review details
            if let Ok(detail) = ctx.review_status(&review_summary.review_id, workspace) {
                return Some((review_summary.review_id.clone(), detail));
            }
        }
    }

    None
}

/// Execute finish steps and render the execution report.
fn execute_and_render(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    // Execute the steps
    let report = executor::execute_steps(&guidance.steps)
        .map_err(|e| anyhow::anyhow!("execution failed: {}", e))?;

    // Render the execution report
    let output = executor::render_report(&report, format);
    println!("{}", output);

    // Exit with non-zero if any step failed
    if !report.remaining.is_empty() || report.results.iter().any(|r| !r.success) {
        std::process::exit(1);
    }

    Ok(())
}

/// Render and print guidance.
fn print_guidance(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    let output =
        render::render(guidance, format).map_err(|e| anyhow::anyhow!("render error: {}", e))?;
    println!("{}", output);
    Ok(())
}

// Helper trait extension for ReviewGateDecision
trait ReviewGateDecisionExt {
    fn open_thread_count_hint(&self, review: &super::adapters::ReviewDetail) -> usize;
}

impl ReviewGateDecisionExt for review_gate::ReviewGateDecision {
    fn open_thread_count_hint(&self, review: &super::adapters::ReviewDetail) -> usize {
        review.open_thread_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::protocol::render::{BoneRef, ProtocolGuidance};

    #[test]
    fn test_build_finish_steps_with_merge() {
        let mut guidance = ProtocolGuidance::new("finish");
        guidance.bone = Some(BoneRef {
            id: "bd-abc".to_string(),
            title: "test feature".to_string(),
        });
        guidance.workspace = Some("frost-castle".to_string());

        build_finish_steps(
            &mut guidance,
            "bd-abc",
            "test feature",
            "myproject",
            "frost-castle",
            Some("cr-123"),
            false,
        );

        assert!(guidance.steps.len() >= 6);
        // Should have git add + commit
        assert!(guidance.steps.iter().any(|s| s.contains("git add -A")));
        assert!(guidance.steps.iter().any(|s| s.contains("git commit -m")));
        // Should have ws merge with --message
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("maw ws merge frost-castle --destroy"))
        );
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("--message") && s.contains("test feature"))
        );
        // Should have mark-merged
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("crit reviews mark-merged cr-123"))
        );
        // Should have bn done
        assert!(guidance.steps.iter().any(|s| s.contains("bn done")));
        // Should have bus send task-done
        assert!(guidance.steps.iter().any(|s| s.contains("task-done")));
        // Should have claims release
        assert!(guidance.steps.iter().any(|s| s.contains("claims release")));
    }

    #[test]
    fn test_build_finish_steps_no_merge() {
        let mut guidance = ProtocolGuidance::new("finish");

        build_finish_steps(
            &mut guidance,
            "bd-abc",
            "test feature",
            "myproject",
            "frost-castle",
            None,
            true, // no_merge
        );

        // Should NOT have ws merge
        assert!(!guidance.steps.iter().any(|s| s.contains("maw ws merge")));
        // Should NOT have mark-merged (no review_id)
        assert!(!guidance.steps.iter().any(|s| s.contains("mark-merged")));
        // Should still have close, announce, release
        assert!(guidance.steps.iter().any(|s| s.contains("bn done")));
        assert!(guidance.steps.iter().any(|s| s.contains("task-done")));
        assert!(guidance.steps.iter().any(|s| s.contains("claims release")));
    }

    #[test]
    fn test_build_finish_steps_shell_safety() {
        let mut guidance = ProtocolGuidance::new("finish");

        // Title with shell metacharacters
        build_finish_steps(
            &mut guidance,
            "bd-abc",
            "it's a test; rm -rf /",
            "myproject",
            "frost-castle",
            None,
            false,
        );

        // The title should be shell-escaped in commands that embed it
        let announce_step = guidance
            .steps
            .iter()
            .find(|s| s.contains("bus send"))
            .unwrap();
        assert!(
            announce_step.contains("'\\''"),
            "single quotes in title should be escaped in bus send"
        );
        let commit_step = guidance
            .steps
            .iter()
            .find(|s| s.contains("git commit -m"))
            .unwrap();
        assert!(
            commit_step.contains("'\\''"),
            "single quotes in title should be escaped in git commit"
        );
    }
}
