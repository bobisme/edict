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

/// Parameters for the finish protocol command.
pub struct ExecuteParams<'a> {
    pub bone_id: &'a str,
    pub no_merge: bool,
    pub force: bool,
    pub execute: bool,
    pub agent: &'a str,
    pub project: &'a str,
    pub config: &'a Config,
    pub format: OutputFormat,
}

/// Execute the finish protocol command.
///
/// # Errors
///
/// Returns an error if rendering or printing guidance fails, or if executing
/// the finish steps fails when `--execute` is set.
pub fn execute(params: &ExecuteParams) -> anyhow::Result<()> {
    let &ExecuteParams {
        bone_id,
        no_merge,
        force,
        execute,
        agent,
        project,
        config,
        format,
    } = params;

    // Collect state from rite and maw
    let ctx = match ProtocolContext::collect(project, agent) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut guidance = ProtocolGuidance::new("finish");
            guidance.blocked(format!("failed to collect state: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    // Fetch bone info
    let Ok(bone_info) = ctx.bone_status(bone_id) else {
        let mut guidance = ProtocolGuidance::new("finish");
        guidance.blocked(format!(
            "bone {bone_id} not found. Check the ID with: maw exec default -- bn show {bone_id}"
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    };

    let mut guidance = ProtocolGuidance::new("finish");
    guidance.bone = Some(BoneRef {
        id: bone_id.to_string(),
        title: bone_info.title.clone(),
    });
    guidance.set_freshness(120, Some(format!("edict protocol finish {bone_id}")));

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
            "agent '{agent}' does not hold a claim for bone {bone_id}. \
             Check with: rite claims list --agent {agent} --format json"
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    }

    // Resolve workspace from claims
    let workspace = if let Some(ws) = ctx.workspace_for_bone(bone_id) {
        ws.to_string()
    } else {
        guidance.blocked(format!(
            "no workspace claim found for bone {bone_id}. \
             Cannot determine which workspace to merge."
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    };
    guidance.workspace = Some(workspace.clone());
    let mut merge_target = ctx
        .find_workspace(&workspace)
        .and_then(|ws| ws.change_id.clone());

    // Build required reviewers list from config: "{project}-{role}"
    let required_reviewers: Vec<String> = config
        .review
        .reviewers
        .iter()
        .map(|role| format!("{project}-{role}"))
        .collect();

    // Check review gate status
    let review_enabled = config.review.enabled && !required_reviewers.is_empty();

    let should_execute = build_finish_guidance(
        &mut guidance,
        &mut GuidanceCtx {
            ctx: &ctx,
            bone_id,
            bead_title: &bone_info.title,
            project,
            workspace: &workspace,
            merge_target: &mut merge_target,
            required_reviewers: &required_reviewers,
            review_enabled,
            force,
            no_merge,
            execute,
        },
    );

    if should_execute {
        return execute_and_render(&guidance, format);
    }

    print_guidance(&guidance, format)?;
    Ok(())
}

/// Parameters for building the standard finish steps.
struct FinishStepsParams<'a> {
    bone_id: &'a str,
    bead_title: &'a str,
    project: &'a str,
    workspace: &'a str,
    merge_target: Option<&'a str>,
    review_id: Option<&'a str>,
    no_merge: bool,
}

/// Inputs for building the finish guidance decision tree.
#[allow(clippy::struct_excessive_bools, reason = "CLI flag context struct")]
struct GuidanceCtx<'a> {
    ctx: &'a ProtocolContext,
    bone_id: &'a str,
    bead_title: &'a str,
    project: &'a str,
    workspace: &'a str,
    merge_target: &'a mut Option<String>,
    required_reviewers: &'a [String],
    review_enabled: bool,
    force: bool,
    no_merge: bool,
    execute: bool,
}

/// Build the finish guidance based on review gate state.
///
/// Returns `true` when the caller should execute the steps immediately (the
/// `--execute` flag was set and the bone is ready to finish).
fn build_finish_guidance(guidance: &mut ProtocolGuidance, gc: &mut GuidanceCtx) -> bool {
    if gc.review_enabled && !gc.force {
        // Try to find a live review for this bone
        if let Some((review_id, review_detail)) =
            gc.ctx.find_review_for_bone(gc.workspace, gc.bone_id)
        {
            let decision = review_gate::evaluate_review_gate(&review_detail, gc.required_reviewers);
            if gc.merge_target.is_none() {
                gc.merge_target.clone_from(&review_detail.change_id);
            }
            guidance.review = Some(ReviewRef {
                review_id: review_id.clone(),
                status: decision.status_str().to_string(),
            });

            match decision.status {
                ReviewGateStatus::Approved => {
                    // Ready to finish
                    guidance.status = ProtocolStatus::Ready;
                    build_finish_steps(
                        guidance,
                        &FinishStepsParams {
                            bone_id: gc.bone_id,
                            bead_title: gc.bead_title,
                            project: gc.project,
                            workspace: gc.workspace,
                            merge_target: gc.merge_target.as_deref(),
                            review_id: Some(&review_id),
                            no_merge: gc.no_merge,
                        },
                    );

                    // Execute if --execute flag is set
                    if gc.execute {
                        return true;
                    }

                    guidance.advise(format!(
                        "Review {review_id} approved. Run these commands to finish bone {}.",
                        gc.bone_id
                    ));
                }
                ReviewGateStatus::Blocked => {
                    build_blocked_section(
                        guidance,
                        &decision,
                        &review_detail,
                        &review_id,
                        gc.workspace,
                        gc.project,
                        gc.required_reviewers,
                    );
                }
                ReviewGateStatus::NeedsReview => {
                    build_needs_review_section(
                        guidance,
                        &decision,
                        &review_id,
                        gc.workspace,
                        gc.project,
                    );
                }
            }
        } else {
            build_no_review_section(
                guidance,
                gc.bone_id,
                gc.bead_title,
                gc.workspace,
                gc.project,
                gc.required_reviewers,
            );
        }
    } else {
        // Review not enabled, or --force flag used
        guidance.status = ProtocolStatus::Ready;

        if gc.force && gc.review_enabled {
            guidance.diagnostic("WARNING: --force flag used, bypassing review gate.".to_string());
        }

        build_finish_steps(
            guidance,
            &FinishStepsParams {
                bone_id: gc.bone_id,
                bead_title: gc.bead_title,
                project: gc.project,
                workspace: gc.workspace,
                merge_target: gc.merge_target.as_deref(),
                review_id: None,
                no_merge: gc.no_merge,
            },
        );

        // Execute if --execute flag is set
        if gc.execute {
            return true;
        }

        if gc.force && gc.review_enabled {
            guidance.advise(format!(
                "Force-finishing bone {} without review approval. Run these commands to finish.",
                gc.bone_id
            ));
        } else {
            guidance.advise(format!(
                "Review not required. Run these commands to finish bone {}.",
                gc.bone_id
            ));
        }
    }

    false
}

/// Build the standard finish steps: commit workspace, merge, close, announce,
/// release claims.
fn build_finish_steps(guidance: &mut ProtocolGuidance, params: &FinishStepsParams) {
    let FinishStepsParams {
        bone_id,
        bead_title,
        project,
        workspace,
        merge_target,
        review_id,
        no_merge,
    } = *params;

    let mut steps = Vec::new();

    // 1. Stage workspace changes
    steps.push(format!("maw exec {workspace} -- git add -A"));

    // 2. Commit workspace changes
    steps.push(format!(
        "maw exec {} -- git commit -m {}",
        workspace,
        shell::shell_escape(&format!(
            "{bone_id}: {bead_title}\n\nCo-Authored-By: Claude <noreply@anthropic.com>"
        ))
    ));

    // 3. Merge workspace (unless --no-merge)
    if !no_merge {
        // Use a conventional commit message derived from the bone title
        let merge_msg = format!("feat: {bead_title}");
        let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
        steps.push(shell::ws_merge_cmd(workspace, target, &merge_msg));
    }

    // 4. Mark review as merged (if review exists)
    if let Some(rid) = review_id {
        steps.push(format!(
            "maw exec default -- seal reviews mark-merged {rid}"
        ));
    }

    // 5. Close the bone
    steps.push(shell::bn_done_cmd(
        bone_id,
        &format!("Completed in workspace {workspace}"),
    ));

    // 6. Announce completion on rite
    steps.push(shell::rite_send_cmd(
        "agent",
        project,
        &format!("Finished {bone_id}: {bead_title}"),
        "task-done",
    ));

    // 7. Release all claims
    steps.push(shell::claims_release_all_cmd("agent"));

    guidance.steps(steps);
}

/// Build guidance for a review that is blocked by a reviewer.
fn build_blocked_section(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_detail: &super::adapters::ReviewDetail,
    review_id: &str,
    workspace: &str,
    project: &str,
    required_reviewers: &[String],
) {
    // Blocked by reviewer
    guidance.status = ProtocolStatus::Blocked;
    guidance.diagnostic(format!(
        "Review {} is blocked by: {}",
        review_id,
        decision.blocked_by.join(", ")
    ));
    if decision.open_thread_count_hint(review_detail) > 0 {
        guidance.diagnostic(format!(
            "{} open thread(s) need resolution",
            review_detail.open_thread_count
        ));
    }

    // Output commands to check review feedback and re-request
    let mut steps = Vec::new();
    steps.push(shell::seal_show_cmd(workspace, review_id));
    steps.push(shell::seal_request_cmd(
        workspace,
        review_id,
        &required_reviewers.join(","),
        "agent",
    ));
    steps.push(shell::rite_send_cmd(
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
        "Review {review_id} is blocked. Address reviewer feedback, then re-request review."
    ));
}

/// Build guidance for a review that exists but lacks all required approvals.
fn build_needs_review_section(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_id: &str,
    workspace: &str,
    project: &str,
) {
    // Review exists but not all reviewers have voted
    guidance.status = ProtocolStatus::NeedsReview;

    if !decision.missing_approvals.is_empty() {
        guidance.diagnostic(format!(
            "Awaiting votes from: {}",
            decision.missing_approvals.join(", ")
        ));
    }

    let mut steps = Vec::new();
    steps.push(shell::seal_show_cmd(workspace, review_id));
    // Re-request from missing reviewers
    if !decision.missing_approvals.is_empty() {
        steps.push(shell::seal_request_cmd(
            workspace,
            review_id,
            &decision.missing_approvals.join(","),
            "agent",
        ));
        let mentions: Vec<String> = decision
            .missing_approvals
            .iter()
            .map(|r| format!("@{r}"))
            .collect();
        steps.push(shell::rite_send_cmd(
            "agent",
            project,
            &format!("Review pending: {} {}", review_id, mentions.join(" ")),
            "review-request",
        ));
    }
    guidance.steps(steps);
    guidance.advise(format!(
        "Review {review_id} needs approval. Wait for reviewers or re-request."
    ));
}

/// Build guidance for the case where no review exists yet for the bone.
fn build_no_review_section(
    guidance: &mut ProtocolGuidance,
    bone_id: &str,
    bead_title: &str,
    workspace: &str,
    project: &str,
    required_reviewers: &[String],
) {
    // No review found — needs review creation
    guidance.status = ProtocolStatus::NeedsReview;
    guidance.diagnostic(format!("No review found for bone {bone_id}."));

    let mut steps = Vec::new();
    steps.push(shell::seal_create_cmd(
        workspace,
        "agent",
        bone_id,
        bead_title,
        &required_reviewers.join(","),
    ));
    let mentions: Vec<String> = required_reviewers.iter().map(|r| format!("@{r}")).collect();
    steps.push(shell::rite_send_cmd(
        "agent",
        project,
        &format!("Review requested: <review-id> {}", mentions.join(" ")),
        "review-request",
    ));
    guidance.steps(steps);
    guidance.advise(
        "No review exists yet. Create one and request reviewers before finishing.".to_string(),
    );
}

/// Execute finish steps and render the execution report.
fn execute_and_render(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    // Execute the steps
    let report = executor::execute_steps(&guidance.steps)
        .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;

    // Render the execution report
    let output = executor::render_report(&report, format);
    println!("{output}");

    // Exit with non-zero if any step failed
    if !report.remaining.is_empty() || report.results.iter().any(|r| !r.success) {
        std::process::exit(1);
    }

    Ok(())
}

/// Render and print guidance.
fn print_guidance(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    let output =
        render::render(guidance, format).map_err(|e| anyhow::anyhow!("render error: {e}"))?;
    println!("{output}");
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
            &FinishStepsParams {
                bone_id: "bd-abc",
                bead_title: "test feature",
                project: "myproject",
                workspace: "frost-castle",
                merge_target: None,
                review_id: Some("cr-123"),
                no_merge: false,
            },
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
                .any(|s| s.contains("maw ws merge frost-castle --into default --destroy"))
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
                .any(|s| s.contains("seal reviews mark-merged cr-123"))
        );
        // Should have bn done
        assert!(guidance.steps.iter().any(|s| s.contains("bn done")));
        // Should have rite send task-done
        assert!(guidance.steps.iter().any(|s| s.contains("task-done")));
        // Should have claims release
        assert!(guidance.steps.iter().any(|s| s.contains("claims release")));
    }

    #[test]
    fn test_build_finish_steps_no_merge() {
        let mut guidance = ProtocolGuidance::new("finish");

        build_finish_steps(
            &mut guidance,
            &FinishStepsParams {
                bone_id: "bd-abc",
                bead_title: "test feature",
                project: "myproject",
                workspace: "frost-castle",
                merge_target: None,
                review_id: None,
                no_merge: true,
            },
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
            &FinishStepsParams {
                bone_id: "bd-abc",
                bead_title: "it's a test; rm -rf /",
                project: "myproject",
                workspace: "frost-castle",
                merge_target: None,
                review_id: None,
                no_merge: false,
            },
        );

        // The title should be shell-escaped in commands that embed it
        let announce_step = guidance
            .steps
            .iter()
            .find(|s| s.contains("rite send"))
            .unwrap();
        assert!(
            announce_step.contains("'\\''"),
            "single quotes in title should be escaped in rite send"
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
