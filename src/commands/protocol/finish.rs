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
    pub layout: crate::layout::Layout,
}

/// Execute the finish protocol command.
///
/// # Errors
///
/// Returns an error if rendering or printing guidance fails, or if executing
/// the finish steps fails when `--execute` is set.
#[allow(
    clippy::too_many_lines,
    reason = "sequential finish-protocol state machine; sub-steps already extracted into helpers"
)]
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
        layout,
    } = params;

    // Collect state from rite and maw
    let ctx = match ProtocolContext::collect(project, agent) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut guidance = ProtocolGuidance::new("finish");
            guidance.set_layout(layout);
            guidance.blocked(format!("failed to collect state: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    // Fetch bone info
    let Ok(bone_info) = ctx.bone_status(bone_id) else {
        let mut guidance = ProtocolGuidance::new("finish");
        guidance.set_layout(layout);
        guidance.blocked(format!(
            "bone {bone_id} not found. Check the ID with: maw exec default -- bn show {bone_id}"
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    };

    let mut guidance = ProtocolGuidance::new("finish");
    guidance.set_layout(layout);
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

    // A risk:critical bone needs a human approval that no protocol state records.
    // Fail closed rather than emit merge steps an agent would run on a security LGTM.
    if merge_needs_human_approval(&bone_info.labels, no_merge) {
        guidance.blocked(format!(
            "bone {bone_id} is risk:critical and requires human approval before merge. \
             protocol finish does not emit merge steps for it. After an authorized approver \
             approves, the approver or the lead merges manually (see finish.md), or run \
             edict protocol finish {bone_id} --no-merge to close the bone without merging."
        ));
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
            agent,
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
    agent: &'a str,
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
    /// The resolved agent name the printed commands act as.
    agent: &'a str,
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
            return build_review_found_guidance(guidance, gc, &review_id, &review_detail);
        }
        build_no_review_section(
            guidance,
            gc.bone_id,
            gc.bead_title,
            gc.workspace,
            gc.project,
            gc.required_reviewers,
            gc.agent,
        );
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
                agent: gc.agent,
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

/// Build the finish guidance for a bone with a live review.
///
/// Returns `true` when the caller should execute the steps immediately.
fn build_review_found_guidance(
    guidance: &mut ProtocolGuidance,
    gc: &mut GuidanceCtx,
    review_id: &str,
    review_detail: &super::adapters::ReviewDetail,
) -> bool {
    let votes = review_gate::evaluate_review_gate(review_detail, gc.required_reviewers);
    // The finish steps merge the workspace, so the approval must cover
    // its HEAD, not just some earlier commit (bn-2cyu). Only an
    // approved review needs the (subprocess) freshness check.
    let (decision, stale_context) = if votes.status == ReviewGateStatus::Approved {
        let (summary, freshness) =
            super::merge::check_approval_freshness(gc.ctx, gc.workspace, review_id);
        if let Some(note) = &freshness.note {
            guidance.diagnostic(note.clone());
        }
        (
            review_gate::bind_to_commit(votes, freshness.freshness),
            Some((summary, freshness)),
        )
    } else {
        (votes, None)
    };
    if gc.merge_target.is_none() {
        gc.merge_target.clone_from(&review_detail.change_id);
    }
    guidance.review = Some(ReviewRef {
        review_id: review_id.to_string(),
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
                    review_id: Some(review_id),
                    no_merge: gc.no_merge,
                    agent: gc.agent,
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
                review_detail,
                review_id,
                gc.workspace,
                gc.project,
                gc.required_reviewers,
                gc.agent,
            );
        }
        ReviewGateStatus::NeedsReview if decision.stale_approval => {
            let (summary, freshness) = stale_context.unwrap_or_else(|| {
                (
                    None,
                    review_gate::FreshnessCheck {
                        freshness: review_gate::CommitFreshness::Unknown,
                        uncovered_paths: Vec::new(),
                        note: None,
                    },
                )
            });
            build_stale_approval_section(
                guidance,
                review_id,
                gc.workspace,
                gc.required_reviewers,
                gc.agent,
                summary.as_ref(),
                &freshness,
            );
        }
        ReviewGateStatus::NeedsReview => {
            build_needs_review_section(
                guidance,
                &decision,
                review_id,
                gc.workspace,
                gc.project,
                gc.agent,
            );
        }
    }
    false
}

/// Whether finishing this bone would merge code that needs a human approval.
///
/// `risk:critical` requires an authorized human to approve before merge. That
/// approval is a Rite message, not Seal state, so no gate here can verify it.
/// With `--no-merge` nothing merges, so the lead's merge path owns the check.
fn merge_needs_human_approval(labels: &[String], no_merge: bool) -> bool {
    !no_merge && labels.iter().any(|l| l == "risk:critical")
}

/// Build the standard finish steps: commit or record, merge, close, announce,
/// release claims.
///
/// With a review, the code was committed before the review was created, and a
/// blanket `git add -A` now would merge files no reviewer saw. The only commit
/// after the LGTM is the review log itself, recorded just before the merge.
fn build_finish_steps(guidance: &mut ProtocolGuidance, params: &FinishStepsParams) {
    let FinishStepsParams {
        bone_id,
        bead_title,
        project,
        workspace,
        merge_target,
        review_id,
        no_merge,
        agent,
    } = *params;

    let mut steps = Vec::new();

    if let Some(rid) = review_id {
        // 1. Record the review in the workspace before the merge destroys it.
        //    With --no-merge the lead records it in `edict protocol merge`:
        //    a merged review no longer passes that merge gate.
        if !no_merge {
            steps.extend(shell::seal_record_cmds(workspace, rid));
        }
    } else {
        // 1. Stage and commit workspace changes
        steps.push(format!("maw exec {workspace} -- git add -A"));
        steps.push(format!(
            "maw exec {} -- git commit -m {}",
            workspace,
            shell::shell_escape(&format!(
                "{bone_id}: {bead_title}\n\nCo-Authored-By: Claude <noreply@anthropic.com>"
            ))
        ));
    }

    // 2. Merge workspace (unless --no-merge)
    if !no_merge {
        // Use a conventional commit message derived from the bone title
        let merge_msg = format!("feat: {bead_title}");
        let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
        steps.push(review_id.map_or_else(
            || shell::ws_merge_cmd(workspace, target, &merge_msg),
            |rid| shell::ws_merge_reviewed_cmd(workspace, rid, target, &merge_msg),
        ));
    }

    // 3. Close the bone
    steps.push(shell::bn_done_cmd(
        bone_id,
        &format!("Completed in workspace {workspace}"),
    ));

    // 4. Announce completion on rite
    steps.push(shell::rite_send_cmd(
        agent,
        project,
        &format!("Finished {bone_id}: {bead_title}"),
        "task-done",
    ));

    // 5. Release all claims
    steps.push(shell::claims_release_all_cmd(agent));

    guidance.steps(steps);
}

/// Build guidance for a review that is blocked by a reviewer.
#[allow(
    clippy::too_many_arguments,
    reason = "flat inputs keep the section builder testable without a ProtocolContext"
)]
fn build_blocked_section(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_detail: &super::adapters::ReviewDetail,
    review_id: &str,
    workspace: &str,
    project: &str,
    required_reviewers: &[String],
    agent: &str,
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

    // Output commands to check review feedback, retarget to the fixed
    // commits, and re-request.
    let mut steps = Vec::new();
    steps.push(shell::seal_show_cmd(workspace, review_id));
    steps.push(shell::seal_retarget_cmd(workspace, review_id, agent));
    steps.push(shell::seal_request_cmd(
        workspace,
        review_id,
        &required_reviewers.join(","),
        agent,
    ));
    steps.push(shell::rite_send_cmd(
        agent,
        project,
        &format!("Review re-requested: {review_id}; dispatch its assigned reviewers explicitly"),
        "review-request",
    ));
    guidance.steps(steps);
    guidance.advise(format!(
        "Review {review_id} is blocked. Address reviewer feedback, retarget the review to the fixed commits, then re-request the same review and explicitly dispatch its assigned reviewers. For security, follow .agents/edict/security-review.md."
    ));
}

/// Build guidance for a review that exists but lacks all required approvals.
fn build_needs_review_section(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_id: &str,
    workspace: &str,
    project: &str,
    agent: &str,
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
            agent,
        ));
        steps.push(shell::rite_send_cmd(
            agent,
            project,
            &format!("Review pending: {review_id}; dispatch missing reviewers explicitly"),
            "review-request",
        ));
    }
    guidance.steps(steps);
    guidance.advise(format!(
        "Review {review_id} needs approval. Explicitly dispatch the missing reviewers; for security, follow .agents/edict/security-review.md."
    ));
}

/// Build guidance for an approval that no longer covers the workspace HEAD.
///
/// Same diagnostic, advice and recovery steps as `protocol merge`: retarget
/// the review to the current commit, re-request, and wait for a fresh LGTM.
fn build_stale_approval_section(
    guidance: &mut ProtocolGuidance,
    review_id: &str,
    workspace: &str,
    required_reviewers: &[String],
    agent: &str,
    summary: Option<&super::adapters::ReviewDiffSummary>,
    freshness: &review_gate::FreshnessCheck,
) {
    guidance.status = ProtocolStatus::NeedsReview;
    guidance.diagnostic(super::merge::stale_approval_diagnostic(
        review_id, workspace, summary, freshness,
    ));
    guidance.advise(super::merge::STALE_APPROVAL_ADVICE.to_string());
    guidance.steps(super::merge::stale_approval_steps(
        workspace,
        review_id,
        required_reviewers,
        agent,
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
    agent: &str,
) {
    // No review found — needs review creation
    guidance.status = ProtocolStatus::NeedsReview;
    guidance.diagnostic(format!("No review found for bone {bone_id}."));

    let steps = vec![
        shell::seal_create_cmd(
            workspace,
            agent,
            bone_id,
            bead_title,
            &required_reviewers.join(","),
        ),
        shell::rite_send_cmd(
            agent,
            project,
            "Review requested: <review-id>; dispatch assigned reviewers explicitly",
            "review-request",
        ),
    ];
    guidance.steps(steps);
    guidance.advise(
        "No review exists yet. Create one, then explicitly dispatch its assigned reviewers before finishing. For security, follow .agents/edict/security-review.md.".to_string(),
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
                agent: "edict-dev",
            },
        );

        // Nothing but the review log is committed after the LGTM
        assert!(!guidance.steps.iter().any(|s| s.contains("git add -A")));
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
        // The review is marked merged and committed in the workspace, before
        // the merge destroys it. Nothing runs in default.
        let pos = |needle: &str| {
            guidance
                .steps
                .iter()
                .position(|s| s.contains(needle))
                .unwrap_or_else(|| panic!("no step contains {needle:?}: {:#?}", guidance.steps))
        };
        let clean = pos(
            "git status --porcelain --untracked-files=all -- . ':(exclude).seal/reviews/cr-123'",
        );
        let mark = pos("maw exec frost-castle -- seal reviews mark-merged cr-123");
        assert!(clean < mark);
        let commit = pos("git commit -m 'chore: seal review cr-123' -- .seal/reviews/cr-123");
        let merge = pos("maw ws merge frost-castle");
        assert!(mark < commit && commit < merge);
        assert!(
            !guidance
                .steps
                .iter()
                .any(|s| s.contains("maw exec default -- seal"))
        );
        // Should have bn done
        assert!(guidance.steps.iter().any(|s| s.contains("bn done")));
        // Should have rite send task-done
        assert!(guidance.steps.iter().any(|s| s.contains("task-done")));
        // Should have claims release
        assert!(guidance.steps.iter().any(|s| s.contains("claims release")));
    }

    /// With --no-merge the lead merges. A merged review fails the lead's merge
    /// gate, so the worker must leave the review open for the lead to record.
    #[test]
    fn test_build_finish_steps_no_merge_leaves_review_to_lead() {
        let mut guidance = ProtocolGuidance::new("finish");

        build_finish_steps(
            &mut guidance,
            &FinishStepsParams {
                bone_id: "bd-abc",
                bead_title: "test feature",
                project: "myproject",
                workspace: "frost-castle",
                merge_target: None,
                review_id: Some("cr-123"),
                no_merge: true,
                agent: "edict-dev",
            },
        );

        assert!(!guidance.steps.iter().any(|s| s.contains("mark-merged")));
        assert!(!guidance.steps.iter().any(|s| s.contains("git commit")));
        assert!(!guidance.steps.iter().any(|s| s.contains("maw ws merge")));
        assert!(guidance.steps.iter().any(|s| s.contains("bn done")));
    }

    #[test]
    fn risk_critical_blocks_merging_finish() {
        let critical = vec!["risk:critical".to_string()];
        assert!(merge_needs_human_approval(&critical, false));
        assert!(!merge_needs_human_approval(&critical, true));
        assert!(!merge_needs_human_approval(
            &["risk:high".to_string()],
            false
        ));
        assert!(!merge_needs_human_approval(&[], false));
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
                agent: "edict-dev",
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
                agent: "edict-dev",
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

    /// A blocked review at finish time must be retargeted to the fixed
    /// commits before it is re-requested (bn-w912): `seal reviews request`
    /// alone leaves the review's target commit pinned at the pre-fix anchor.
    #[test]
    fn build_blocked_section_retargets_before_re_requesting() {
        let mut guidance = ProtocolGuidance::new("finish");
        let decision = review_gate::ReviewGateDecision {
            status: ReviewGateStatus::Blocked,
            missing_approvals: vec![],
            newer_block_after_lgtm: vec![],
            total_required: 1,
            approved_by: vec![],
            blocked_by: vec!["edict-security".to_string()],
            stale_approval: false,
        };
        let review_detail = super::super::adapters::ReviewDetail {
            review_id: "cr-123".into(),
            title: None,
            status: "open".into(),
            status_changed_at: None,
            status_changed_by: None,
            change_id: None,
            votes: vec![],
            open_thread_count: 1,
        };

        build_blocked_section(
            &mut guidance,
            &decision,
            &review_detail,
            "cr-123",
            "frost-castle",
            "myproject",
            &["edict-security".to_string()],
            "edict-dev",
        );

        let retarget_pos = guidance
            .steps
            .iter()
            .position(|s| s.contains("seal reviews retarget cr-123"))
            .expect("retarget step present");
        let request_pos = guidance
            .steps
            .iter()
            .position(|s| s.contains("seal reviews request cr-123"))
            .expect("request step present");
        assert!(
            retarget_pos < request_pos,
            "retarget must come before re-request: {:?}",
            guidance.steps
        );
    }

    fn assert_real_agent(steps: &[String]) {
        assert!(
            steps.iter().all(|s| !s.contains("--agent agent")),
            "placeholder agent leaked: {steps:?}"
        );
    }

    /// bn-25cq: every printed command acts as the real agent, never `agent`.
    #[test]
    fn finish_output_uses_the_real_agent_name() {
        for (review_id, no_merge) in [(Some("cr-123"), false), (None, false), (None, true)] {
            let mut guidance = ProtocolGuidance::new("finish");
            build_finish_steps(
                &mut guidance,
                &FinishStepsParams {
                    bone_id: "bd-abc",
                    bead_title: "test feature",
                    project: "myproject",
                    workspace: "frost-castle",
                    merge_target: None,
                    review_id,
                    no_merge,
                    agent: "edict-dev",
                },
            );
            assert_real_agent(&guidance.steps);
            assert!(
                guidance
                    .steps
                    .iter()
                    .any(|s| s.starts_with("rite send --agent edict-dev"))
            );
            assert!(
                guidance
                    .steps
                    .iter()
                    .any(|s| s.starts_with("rite claims release --agent edict-dev"))
            );
        }

        let decision = review_gate::ReviewGateDecision {
            status: ReviewGateStatus::NeedsReview,
            approved_by: vec![],
            blocked_by: vec!["edict-security".into()],
            missing_approvals: vec!["edict-security".into()],
            newer_block_after_lgtm: vec![],
            total_required: 1,
            stale_approval: false,
        };
        let review_detail = super::super::adapters::ReviewDetail {
            review_id: "cr-123".into(),
            title: None,
            status: "open".into(),
            status_changed_at: None,
            status_changed_by: None,
            change_id: None,
            votes: vec![],
            open_thread_count: 0,
        };
        let reviewers = ["edict-security".to_string()];

        let mut g = ProtocolGuidance::new("finish");
        build_blocked_section(
            &mut g,
            &decision,
            &review_detail,
            "cr-123",
            "frost-castle",
            "myproject",
            &reviewers,
            "edict-dev",
        );
        assert_real_agent(&g.steps);

        let mut g = ProtocolGuidance::new("finish");
        build_needs_review_section(
            &mut g,
            &decision,
            "cr-123",
            "frost-castle",
            "myproject",
            "edict-dev",
        );
        assert_real_agent(&g.steps);
        assert!(g.steps.iter().any(|s| s.contains("--agent edict-dev")));

        let mut g = ProtocolGuidance::new("finish");
        build_no_review_section(
            &mut g,
            "bd-abc",
            "t",
            "frost-castle",
            "myproject",
            &reviewers,
            "edict-dev",
        );
        assert_real_agent(&g.steps);
        assert!(g.steps.iter().any(|s| s.contains("--agent edict-dev")));
    }

    /// bn-2cyu: an approval edict found stale (seal said fresh) emits the
    /// retarget/re-request recovery, names the uncovered path, and no merge.
    #[test]
    fn stale_approval_section_retargets_and_names_uncovered_paths() {
        let summary = super::super::adapters::ReviewDiffSummary {
            target_commit: Some("c658bbf939ff".into()),
            approval_stale: Some(false),
            approved_commit: Some("c658bbf939ff".into()),
            uncovered_commits: Some(0),
        };
        let freshness = review_gate::FreshnessCheck {
            freshness: review_gate::CommitFreshness::Stale,
            uncovered_paths: vec!["src/main.rs".into()],
            note: None,
        };
        let mut g = ProtocolGuidance::new("finish");
        build_stale_approval_section(
            &mut g,
            "cr-123",
            "frost-castle",
            &["edict-security".to_string()],
            "edict-dev",
            Some(&summary),
            &freshness,
        );
        assert_eq!(g.status, ProtocolStatus::NeedsReview);
        assert!(
            g.steps
                .iter()
                .any(|s| s.contains("seal reviews retarget cr-123"))
        );
        assert!(
            g.steps
                .iter()
                .all(|s| !s.contains("ws merge") && !s.contains("mark-merged"))
        );
        assert_real_agent(&g.steps);
        let diag = g.diagnostics.join("\n");
        assert!(diag.contains("src/main.rs"), "{diag}");
        assert!(diag.contains("approval_stale:false"), "{diag}");
    }
}
