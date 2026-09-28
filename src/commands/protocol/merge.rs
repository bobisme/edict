//! Protocol merge command: lead-facing command to check preconditions and
//! output merge steps for a worker's completed workspace.
//!
//! Validates: workspace exists, has changes, associated bone is closed,
//! review is approved (if enabled). Outputs merge steps with conflict
//! recovery guidance.

use std::io::IsTerminal;

use anyhow::Context;
use serde::Deserialize;

use super::adapters::{self, ReviewDetail};
use super::context::ProtocolContext;
use super::render::{self, ProtocolGuidance, ProtocolStatus};
use super::review_gate::{self, ReviewGateStatus};
use super::shell;
use crate::commands::doctor::OutputFormat;
use crate::config::Config;

/// Resolve the commit message: use the provided value, open an editor on TTY, or fail.
///
/// - If `provided` is `Some`, returns it as-is.
/// - If stdin is not a TTY, returns an error asking for `--message`.
/// - If stdin is a TTY, opens `$EDITOR` → `$VISUAL` → `vi` with a template, reads the result.
///
/// # Errors
///
/// Returns an error when no message is provided in non-interactive mode, the
/// editor cannot be launched or exits non-zero, or the resulting message is empty.
pub fn resolve_message(provided: Option<&str>) -> anyhow::Result<String> {
    if let Some(msg) = provided {
        return Ok(msg.to_string());
    }

    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "--message is required in non-interactive mode.\n\
             Example: edict protocol merge <workspace> --message \"feat: description\""
        );
    }

    // TTY: open editor with a template
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let tmp_path = std::env::temp_dir().join(format!("edict-merge-msg-{}.txt", std::process::id()));
    std::fs::write(
        &tmp_path,
        "# Enter commit message for merge (lines starting with '#' are ignored).\n\
         # Use conventional commit prefix: feat:, fix:, chore:, docs:, etc.\n\
         # Example: feat: add user authentication\n\n",
    )
    .context("failed to create temporary message file")?;

    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .with_context(|| format!("failed to open editor '{editor}'"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::bail!("editor '{editor}' exited with non-zero status — aborting");
    }

    let content =
        std::fs::read_to_string(&tmp_path).context("failed to read message from editor")?;
    let _ = std::fs::remove_file(&tmp_path);

    let msg: String = content
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if msg.is_empty() {
        anyhow::bail!("commit message is empty — aborting merge");
    }

    Ok(msg)
}

/// Parsed output from `maw ws merge <ws> --check --format json`.
#[derive(Debug, Clone, Deserialize)]
struct MergeCheckResult {
    #[serde(default)]
    ready: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conflicts: Vec<serde_json::Value>,
    #[serde(default)]
    has_conflicts: bool,
    #[serde(default)]
    stale: bool,
    #[serde(default)]
    message: Option<String>,
}

impl MergeCheckResult {
    fn is_ready(&self) -> bool {
        self.ready.unwrap_or_else(|| {
            self.status.as_ref().map_or_else(
                || !self.has_conflicts && self.conflicts.is_empty() && !self.stale,
                |status| matches!(status.as_str(), "clean" | "ready" | "ok") && !self.has_conflicts,
            )
        })
    }

    fn conflict_labels(&self) -> Vec<String> {
        self.conflicts
            .iter()
            .map(|conflict| {
                conflict
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| {
                        conflict
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .or_else(|| {
                        conflict
                            .get("file")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| conflict.to_string())
            })
            .collect()
    }
}

/// Execute the merge protocol command.
///
/// # Errors
///
/// Returns an error when guidance fails to render or, in execute mode, when
/// the merge steps fail to run.
#[allow(
    clippy::too_many_arguments,
    reason = "CLI command entry point: each arg is a distinct user-facing option"
)]
#[allow(
    clippy::too_many_lines,
    reason = "sequential merge-protocol state machine; sub-steps already extracted into helpers"
)]
pub fn execute(
    workspace: &str,
    message: &str,
    force: bool,
    execute: bool,
    agent: &str,
    project: &str,
    config: &Config,
    format: OutputFormat,
    layout: crate::layout::Layout,
) -> anyhow::Result<()> {
    // Reject merging default workspace
    if workspace == "default" {
        let mut guidance = ProtocolGuidance::new("merge");
        guidance.set_layout(layout);
        guidance.blocked(
            "cannot merge the default workspace. \
             Default is the merge TARGET — other workspaces merge INTO it."
                .to_string(),
        );
        print_guidance(&guidance, format)?;
        return Ok(());
    }

    // Collect state from rite and maw
    let ctx = match ProtocolContext::collect(project, agent) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut guidance = ProtocolGuidance::new("merge");
            guidance.set_layout(layout);
            guidance.blocked(format!("failed to collect state: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    let mut guidance = ProtocolGuidance::new("merge");
    guidance.set_layout(layout);
    guidance.workspace = Some(workspace.to_string());
    guidance.set_freshness(120, Some(format!("edict protocol merge {workspace}")));
    let mut merge_target = ctx
        .find_workspace(workspace)
        .and_then(|ws| ws.change_id.clone());

    // Check workspace exists
    let ws_exists = ctx.workspaces().iter().any(|ws| ws.name == workspace);
    if !ws_exists {
        guidance.blocked(format!(
            "workspace '{workspace}' not found. Check with: maw ws list"
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    }

    // Try to find the associated bone from workspace claims
    // --force skips the review gate, so the bone must be bound to this exact
    // workspace: its labels are the only risk:critical check left.
    let bone_id = find_bone_for_workspace(&ctx, workspace, force);

    if check_bone_gate(&mut guidance, &ctx, bone_id.as_deref(), force, format)? {
        return Ok(());
    }

    // Check review gate (if enabled)
    let required_reviewers: Vec<String> = config
        .review
        .reviewers
        .iter()
        .map(|role| format!("{project}-{role}"))
        .collect();
    let review_enabled = config.review.enabled && !required_reviewers.is_empty();

    if review_enabled
        && !force
        && check_review_gate(
            &mut guidance,
            &ctx,
            workspace,
            bone_id.as_deref(),
            &required_reviewers,
            &mut merge_target,
            format,
        )?
    {
        return Ok(());
    }

    if check_conflict_gate(
        &mut guidance,
        workspace,
        merge_target.as_deref(),
        message,
        format,
    )? {
        return Ok(());
    }

    // All preconditions met — build merge steps
    guidance.status = ProtocolStatus::Ready;
    let review_id = review_enabled
        .then(|| find_review_id(&ctx, workspace, bone_id.as_deref()))
        .flatten();

    build_merge_steps(
        &mut guidance,
        &MergeStepsParams {
            workspace,
            project,
            message,
            merge_target: merge_target.as_deref(),
            bone_id: bone_id.as_deref(),
            review_id: review_id.as_deref(),
            push_main: config.push_main,
            agent: ctx.agent(),
        },
    );

    // Execute if --execute flag is set
    if execute {
        return execute_and_render(&guidance, workspace, message, format);
    }

    if force {
        guidance.advise(format!(
            "Force-merging workspace {workspace} (review/bone checks bypassed). \
             Run these commands to merge."
        ));
    } else {
        guidance.advise(format!(
            "All preconditions met. Run these commands to merge workspace {workspace}."
        ));
    }

    print_guidance(&guidance, format)?;
    Ok(())
}

/// Check the bone-status gate. Returns `Ok(true)` when the caller should stop
/// (guidance was printed), `Ok(false)` to continue.
fn check_bone_gate(
    guidance: &mut ProtocolGuidance,
    ctx: &ProtocolContext,
    bone_id: Option<&str>,
    force: bool,
    format: OutputFormat,
) -> anyhow::Result<bool> {
    let Some(bone_id) = bone_id else {
        // --force skips the review gate, and only the bone's labels show
        // whether the work is risk:critical. Without a bone, --force would emit
        // merge steps for critical work that no human approved.
        if force {
            block_force_without_labels(
                guidance,
                "No bone is linked to this workspace through a bone claim held by this agent",
            );
            print_guidance(guidance, format)?;
            return Ok(true);
        }
        guidance.diagnostic(
            "No associated bone found for this workspace. Proceeding without bone check."
                .to_string(),
        );
        return Ok(false);
    };

    guidance.bone = Some(render::BoneRef {
        id: bone_id.to_string(),
        title: String::new(), // filled below if bone found
    });

    // Check bone status
    if let Ok(bone_info) = ctx.bone_status(bone_id) {
        guidance.bone = Some(render::BoneRef {
            id: bone_id.to_string(),
            title: bone_info.title.clone(),
        });

        // risk:critical needs a human approval that no protocol state records,
        // so the protocol never emits its merge steps. --force does not lift this.
        if bone_info.labels.iter().any(|l| l == "risk:critical") {
            guidance.status = ProtocolStatus::Blocked;
            guidance.diagnostic(format!(
                "Bone {bone_id} is risk:critical and requires human approval before merge. \
                 edict protocol merge does not emit merge steps for it."
            ));
            guidance.advise(
                "Verify the authorized human approval in rite history, record it on the bone, \
                 then merge manually (see merge-check.md: record the review, then maw ws merge)."
                    .to_string(),
            );
            print_guidance(guidance, format)?;
            return Ok(true);
        }

        if bone_info.state != "done" && !force {
            guidance.status = ProtocolStatus::Blocked;
            guidance.diagnostic(format!(
                "Bone {} is '{}', expected 'done'. Worker may still be working.",
                bone_id, bone_info.state
            ));
            guidance.advise(format!(
                "Wait for worker to finish bone {bone_id}, or use --force to merge anyway."
            ));

            let mut steps = Vec::new();
            steps.push(format!("maw exec default -- bn show {bone_id}"));
            guidance.steps(steps);

            print_guidance(guidance, format)?;
            return Ok(true);
        }
    } else {
        // Fail closed: every other unresolvable input in this module (no
        // bone, unknown status, uncorroborated identity) blocks. This is
        // safe to bypass with --force like the sibling state-mismatch
        // check above, but must not silently pass by default just because
        // `bn show` failed — that would let a bones-tool outage or a wrong
        // bone ID open the merge gate.
        if !force {
            guidance.status = ProtocolStatus::Blocked;
            guidance.diagnostic(format!(
                "Could not fetch bone {bone_id} — refusing to assume it is done. It may have been deleted, or `bn show` may be failing."
            ));
            guidance.advise(format!(
                "Investigate why `bn show {bone_id}` failed, or use --force to merge anyway."
            ));

            let mut steps = Vec::new();
            steps.push(format!("maw exec default -- bn show {bone_id}"));
            guidance.steps(steps);

            print_guidance(guidance, format)?;
            return Ok(true);
        }
        block_force_without_labels(guidance, &format!("Could not fetch bone {bone_id}"));
        print_guidance(guidance, format)?;
        return Ok(true);
    }

    Ok(false)
}

/// Block a `--force` merge whose bone labels could not be loaded.
///
/// `--force` lifts the done-state and review checks, never the risk:critical
/// human-approval gate. That gate reads the bone's labels, so a forced merge
/// needs an identified bone.
fn block_force_without_labels(guidance: &mut ProtocolGuidance, reason: &str) {
    guidance.status = ProtocolStatus::Blocked;
    guidance.diagnostic(format!(
        "{reason}, so its labels are unknown. --force cannot rule out risk:critical, \
         which needs human approval, so it does not emit merge steps."
    ));
    guidance.advise(
        "Stake the bone claim as this agent so protocol merge can load the bone, or verify \
         the bone is not risk:critical and merge manually (see merge-check.md)."
            .to_string(),
    );
}

/// Check whether the review's approval covers the workspace HEAD.
///
/// Reads `seal diff <id> --format json` and the workspace HEAD, and when seal
/// says the approval is current but its `approved_commit` is not HEAD, lists
/// what changed in between (`git diff --name-only`) to catch a post-LGTM
/// commit seal missed (seal bn-2ypz). See [`review_gate::approval_freshness`].
pub(super) fn check_approval_freshness(
    ctx: &ProtocolContext,
    workspace: &str,
    review_id: &str,
) -> (
    Option<adapters::ReviewDiffSummary>,
    review_gate::FreshnessCheck,
) {
    let diff_summary = ctx.review_diff_summary(review_id, workspace).ok();
    let head = ctx.workspace_head_commit(workspace).ok();
    let check = review_gate::approval_freshness(
        diff_summary.as_ref(),
        head.as_deref(),
        review_id,
        |from, to| ctx.changed_paths_between(workspace, from, to).ok(),
    );
    (diff_summary, check)
}

/// The diagnostic for an approval that no longer covers the workspace HEAD.
///
/// Names what is not covered: the paths edict found changed after the
/// approved commit, or else the commits seal counted.
pub(super) fn stale_approval_diagnostic(
    review_id: &str,
    workspace: &str,
    diff_summary: Option<&adapters::ReviewDiffSummary>,
    check: &review_gate::FreshnessCheck,
) -> String {
    let approved_short = |c: &str| c.chars().take(12).collect::<String>();
    let seal_detects = check.uncovered_paths.is_empty();
    let scope = if seal_detects {
        diff_summary.map_or_else(String::new, |d| {
            match (d.approved_commit.as_deref(), d.uncovered_commits) {
                (Some(commit), Some(n)) => {
                    format!(
                        " Approved at {}, {n} commit(s) not covered.",
                        approved_short(commit)
                    )
                }
                _ => String::new(),
            }
        })
    } else {
        let at = diff_summary
            .and_then(|d| d.approved_commit.as_deref())
            .map_or_else(String::new, |c| format!(" at {}", approved_short(c)));
        format!(
            " Approved{at}; changed since, outside .seal/reviews/{review_id}/: {}.",
            check.uncovered_paths.join(", ")
        )
    };
    let seal_note = if seal_detects {
        "`seal reviews mark-merged` refuses this until the approval covers the current code."
    } else {
        "seal reported approval_stale:false and its `mark-merged` would accept this, but \
         the approval does not cover these changes. Do not merge until it does."
    };
    format!(
        "Review {review_id} was approved for an earlier commit — new commits have \
         landed on workspace {workspace} since (or the commit couldn't be \
         confirmed).{scope} {seal_note}"
    )
}

/// Advice for an approval that no longer covers the workspace HEAD.
pub(super) const STALE_APPROVAL_ADVICE: &str = "Retarget the review to this commit, then \
     re-request review and wait for a fresh LGTM. `seal reviews request` alone leaves the \
     review's target commit pinned at the old, already-approved anchor — only `seal reviews \
     retarget` moves it and requires fresh votes. Only pass --allow-stale-approval when the \
     new commits are provably outside what was reviewed.";

/// Check the review gate. Only called when review is enabled and not forced.
/// Returns `Ok(true)` when the caller should stop (guidance was printed),
/// `Ok(false)` to continue. May update `merge_target` from the review detail.
fn check_review_gate(
    guidance: &mut ProtocolGuidance,
    ctx: &ProtocolContext,
    workspace: &str,
    bone_id: Option<&str>,
    required_reviewers: &[String],
    merge_target: &mut Option<String>,
    format: OutputFormat,
) -> anyhow::Result<bool> {
    if let Some((review_id, review_detail)) =
        bone_id.and_then(|id| ctx.find_review_for_bone(workspace, id))
    {
        if merge_target.is_none() {
            merge_target.clone_from(&review_detail.change_id);
        }
        return evaluate_found_review(
            guidance,
            ctx,
            workspace,
            &review_id,
            &review_detail,
            required_reviewers,
            format,
        );
    }
    handle_missing_review(
        guidance,
        workspace,
        bone_id,
        required_reviewers,
        ctx.agent(),
        format,
    )
}

/// Evaluate a review that was found for this bone, including the
/// review→commit binding, and apply guidance. Returns `Ok(true)` when the
/// caller should stop (guidance was printed), `Ok(false)` to continue.
fn evaluate_found_review(
    guidance: &mut ProtocolGuidance,
    ctx: &ProtocolContext,
    workspace: &str,
    review_id: &str,
    review_detail: &ReviewDetail,
    required_reviewers: &[String],
    format: OutputFormat,
) -> anyhow::Result<bool> {
    // Votes alone only prove *some* version of this work was approved,
    // not that it's the version about to be merged (review→bone binding
    // without review→commit binding). Compare the commit the review's
    // diff was last computed against to the workspace's actual current
    // HEAD; either subprocess failing, or the two disagreeing, fails
    // closed via `bind_to_commit` below rather than trusting stale votes.
    let (diff_summary, freshness) = check_approval_freshness(ctx, workspace, review_id);
    if let Some(note) = &freshness.note {
        guidance.diagnostic(note.clone());
    }
    let decision = review_gate::bind_to_commit(
        review_gate::evaluate_review_gate(review_detail, required_reviewers),
        freshness.freshness,
    );
    guidance.review = Some(render::ReviewRef {
        review_id: review_id.to_string(),
        status: decision.status_str().to_string(),
    });

    match decision.status {
        ReviewGateStatus::Approved => {
            // Good — review approved, proceed to merge
        }
        ReviewGateStatus::Blocked => {
            guidance.status = ProtocolStatus::Blocked;
            guidance.diagnostic(format!(
                "Review {} is blocked by: {}. Resolve feedback before merging.",
                review_id,
                decision.blocked_by.join(", ")
            ));
            guidance.advise(
                "Address reviewer feedback, retarget the review to the fixed commits, then \
                 re-request review."
                    .to_string(),
            );

            let steps = vec![shell::seal_show_cmd(workspace, review_id)];
            guidance.steps(steps);

            print_guidance(guidance, format)?;
            return Ok(true);
        }
        ReviewGateStatus::NeedsReview if decision.stale_approval => {
            guidance.status = ProtocolStatus::NeedsReview;
            guidance.diagnostic(stale_approval_diagnostic(
                review_id,
                workspace,
                diff_summary.as_ref(),
                &freshness,
            ));
            guidance.advise(STALE_APPROVAL_ADVICE.to_string());

            let steps = stale_approval_steps(workspace, review_id, required_reviewers, ctx.agent());
            guidance.steps(steps);

            print_guidance(guidance, format)?;
            return Ok(true);
        }
        ReviewGateStatus::NeedsReview => {
            guidance.status = ProtocolStatus::NeedsReview;
            guidance.diagnostic(format!(
                "Review {} still awaiting votes from: {}",
                review_id,
                decision.missing_approvals.join(", ")
            ));
            guidance.advise("Wait for reviewers or re-request review before merging.".to_string());

            let steps = vec![shell::seal_show_cmd(workspace, review_id)];
            guidance.steps(steps);

            print_guidance(guidance, format)?;
            return Ok(true);
        }
    }

    Ok(false)
}

/// Steps to recover a review whose approval no longer covers the workspace
/// HEAD: retarget it to the current commit first, then re-request.
///
/// `seal reviews request` alone leaves the review's target commit pinned at
/// the old, already-approved anchor (bn-w912) — only `seal reviews retarget`
/// moves it and requires fresh votes.
pub(super) fn stale_approval_steps(
    workspace: &str,
    review_id: &str,
    required_reviewers: &[String],
    agent: &str,
) -> Vec<String> {
    vec![
        shell::seal_retarget_cmd(workspace, review_id, agent),
        shell::seal_request_cmd(workspace, review_id, &required_reviewers.join(","), agent),
        shell::seal_show_cmd(workspace, review_id),
    ]
}

/// No live review exists for this bone. Returns `Ok(true)` when the caller
/// should stop (guidance was printed), `Ok(false)` to continue.
fn handle_missing_review(
    guidance: &mut ProtocolGuidance,
    workspace: &str,
    bone_id: Option<&str>,
    required_reviewers: &[String],
    agent: &str,
    format: OutputFormat,
) -> anyhow::Result<bool> {
    guidance.status = ProtocolStatus::NeedsReview;

    if let Some(id) = bone_id {
        guidance.diagnostic(format!(
            "Review is enabled but no live review exists for bone {id}."
        ));
        guidance.advise("Create a review before merging.".to_string());

        let mut steps = Vec::new();
        steps.push(shell::seal_create_cmd(
            workspace,
            agent,
            id,
            &format!("work from {workspace}"),
            &required_reviewers.join(","),
        ));
        guidance.steps(steps);
    } else {
        // Without a bone ID there is no title the gate could match, so a review
        // created here would be invisible to it — approved and still reported as
        // missing. Offering that step would leave `--force` (skip the gate) as the
        // only way forward, so say what is actually wrong instead.
        guidance.diagnostic(format!(
            "Review is enabled but the bone behind workspace {workspace} could not be \
                 identified, so no review can be bound to it."
        ));
        guidance.advise(
            "Stake the workspace claim for the bone \
                 (rite claims stake \"workspace://<project>/<ws>\" -m \"<bone-id>\"), \
                 then run `edict protocol review <bone-id>` to open a review against it."
                .to_string(),
        );
    }

    print_guidance(guidance, format)?;
    Ok(true)
}

/// Run the pre-flight conflict check and apply guidance. Returns `Ok(true)`
/// when the caller should stop (guidance was printed), `Ok(false)` to continue.
fn check_conflict_gate(
    guidance: &mut ProtocolGuidance,
    workspace: &str,
    merge_target: Option<&str>,
    message: &str,
    format: OutputFormat,
) -> anyhow::Result<bool> {
    match run_merge_check(workspace, merge_target) {
        Ok(check) => {
            if !check.is_ready() {
                guidance.status = ProtocolStatus::Blocked;
                let conflict_labels = check.conflict_labels();
                if !conflict_labels.is_empty() {
                    guidance.diagnostic(format!(
                        "Merge would produce conflicts in {} file(s): {}",
                        conflict_labels.len(),
                        conflict_labels.join(", ")
                    ));
                }
                if check.stale {
                    guidance.diagnostic(format!(
                        "Workspace {workspace} is stale. `maw ws merge` refuses a stale source \
                         instead of syncing it — run `maw ws sync {workspace}` first, then retry \
                         the merge."
                    ));
                }
                if let Some(message) = check.message.as_deref() {
                    guidance.diagnostic(message.to_string());
                }
                let review_id = guidance.review.as_ref().map(|r| r.review_id.clone());
                add_conflict_recovery_guidance(
                    guidance,
                    workspace,
                    merge_target,
                    message,
                    review_id.as_deref().map(|rid| (rid, false)),
                );
                print_guidance(guidance, format)?;
                return Ok(true);
            }
        }
        Err(e) => {
            // --check failed (maybe old maw version). Warn but proceed.
            guidance.diagnostic(format!(
                "Pre-flight check failed ({e}). Proceeding without conflict detection."
            ));
        }
    }

    Ok(false)
}

/// Run `maw ws merge <ws> --into <target> --check --format json` before merging.
fn run_merge_check(
    workspace: &str,
    merge_target: Option<&str>,
) -> Result<MergeCheckResult, String> {
    let target = merge_target.unwrap_or("default");
    let output = std::process::Command::new("maw")
        .args([
            "ws", "merge", workspace, "--into", target, "--check", "--format", "json",
        ])
        .output()
        .map_err(|e| format!("failed to run maw ws merge --check: {e}"))?;

    let stdout = String::from_utf8(output.stdout).map_err(|e| format!("invalid UTF-8: {e}"))?;

    // Parse JSON even on non-zero exit (--check exits non-zero on conflicts)
    serde_json::from_str(&stdout).map_err(|e| format!("failed to parse --check output: {e}"))
}

/// Parameters for [`build_merge_steps`].
struct MergeStepsParams<'a> {
    workspace: &'a str,
    project: &'a str,
    message: &'a str,
    merge_target: Option<&'a str>,
    bone_id: Option<&'a str>,
    review_id: Option<&'a str>,
    push_main: bool,
    /// The resolved agent name the printed commands act as.
    agent: &'a str,
}

/// Build the merge steps: record the review, merge, push, announce.
/// Also includes conflict recovery guidance as diagnostics.
fn build_merge_steps(guidance: &mut ProtocolGuidance, params: &MergeStepsParams) {
    let MergeStepsParams {
        workspace,
        project,
        message,
        merge_target,
        bone_id,
        review_id,
        push_main,
        agent,
    } = *params;

    let mut steps = Vec::new();

    // 1. Record the review in the workspace. The merge destroys the
    //    workspace, so this is the last point where the log can be committed.
    if let Some(rid) = review_id {
        steps.extend(shell::seal_record_cmds(workspace, rid));
    }

    // 2. Merge workspace with the required commit message
    let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
    steps.push(review_id.map_or_else(
        || shell::ws_merge_cmd(workspace, target, message),
        |rid| shell::ws_merge_reviewed_cmd(workspace, rid, target, message),
    ));

    // 3. Push (if enabled)
    if push_main {
        steps.push("maw push".to_string());
    }

    // 4. Announce merge
    let announce_msg = bone_id.map_or_else(
        || format!("Merged workspace {workspace}"),
        |bid| format!("Merged workspace {workspace} ({bid})"),
    );
    steps.push(shell::rite_send_cmd(
        agent,
        project,
        &announce_msg,
        "task-done",
    ));

    guidance.steps(steps);

    // Add conflict recovery guidance
    add_conflict_recovery_guidance(
        guidance,
        workspace,
        merge_target,
        message,
        review_id.map(|rid| (rid, true)),
    );
}

/// Append comprehensive maw/git conflict recovery guidance as diagnostics.
///
/// `review` is the review id and whether its record steps already ran. Before
/// they run, a raw retry would skip them and lose the log, so the retry is the
/// protocol itself. After, the retry carries the clean check.
fn add_conflict_recovery_guidance(
    guidance: &mut ProtocolGuidance,
    workspace: &str,
    merge_target: Option<&str>,
    merge_msg: &str,
    review: Option<(&str, bool)>,
) {
    let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
    let retry_cmd = match review {
        Some((rid, true)) => shell::ws_merge_reviewed_cmd(workspace, rid, target, merge_msg),
        Some((_, false)) => format!(
            "edict protocol merge {workspace} --message {}",
            shell::shell_escape(merge_msg)
        ),
        None => shell::ws_merge_cmd(workspace, target, merge_msg),
    };
    let check_cmd = shell::ws_merge_check_cmd(workspace, target);
    guidance.diagnostic(format!(
        "Conflict recovery — workspace is preserved (not destroyed). Conflicts are data, not \
         failure. A stale source is different: `maw ws merge` refuses it outright rather than \
         syncing it, so if `--check` reports staleness, run `maw ws sync {workspace}` first, \
         then retry the merge. Steps:\n\
         \n\
         1. Inspect conflicts:\n\
         \n\
         maw ws conflicts {workspace} --format json\n\
         {check_cmd}\n\
         maw ws resolve {workspace} --list\n\
         \n\
         2. For auto-resolvable files (.bones/, .claude/, .agents/):\n\
         \n\
         maw exec {workspace} -- git restore --source refs/heads/main -- .bones/ .claude/ .agents/\n\
         \n\
         Once the review is recorded, restore only .bones/. A change to .claude/ or .agents/ \
         after the approval needs a fresh review.\n\
         \n\
         3. Resolve remaining conflicts — prefer `maw ws resolve` over hand-editing markers:\n\
         \n\
         maw ws resolve {workspace} --keep epoch|{workspace}|both|union   # whole-workspace\n\
         maw ws resolve {workspace} --keep <path>=<name>                 # per-file\n\
         \n\
         Manual fallback: edit markers by hand, then stage in the workspace:\n\
         \n\
         maw exec {workspace} -- git status\n\
         maw exec {workspace} -- git add <resolved-file>\n\
         \n\
         4. After resolving:\n\
         \n\
         {retry_cmd}              # retry merge\n\
         \n\
         If this merge had a review, it is already marked merged. Retry directly only when the \
         resolution changed nothing outside .seal/ and .bones/. Any other resolution is \
         unreviewed code: commit it, create a fresh review for the bone, and rerun \
         `edict protocol merge` after its LGTM.\n\
         \n\
         (or resolve inline at merge time with --resolve-all={workspace} / --resolve cf-id=<name>)\n\
         \n\
         5. If the merge ATTEMPT ITSELF got stuck (killed/OOM'd/panicked/Ctrl-C'd mid-merge, not \
         a normal recorded conflict), clear the orphaned merge-state:\n\
         \n\
         maw ws merge --abort\n\
         \n\
         To undo a COMPLETED merge instead (recover pre-merge state), use the repo-level undo — \
         NOT `maw ws undo {workspace}`, which discards the workspace's entire delta including the \
         work being merged:\n\
         \n\
         maw undo                                        # undo the last completed merge\n\
         \n\
         6. To recover a destroyed workspace:\n\
         \n\
         maw ws recover {workspace} --to {workspace}-recovered    # recreate it under a new name",
    ));
}

/// ID of the live review gating `bone_id`, if the bone and its review are known.
fn find_review_id(ctx: &ProtocolContext, workspace: &str, bone_id: Option<&str>) -> Option<String> {
    let bone_id = bone_id?;
    ctx.find_review_for_bone(workspace, bone_id)
        .map(|(review_id, _)| review_id)
}

/// Whether this agent actually holds the bone claim for `bone_id`.
///
/// This is the corroboration every path below needs. A workspace name and a claim
/// memo are both strings the caller chooses, so on their own they let the caller
/// nominate which bone gates its merge — point either at someone else's approved
/// bone and the merge inherits that bone's LGTM. rite grants bone claims
/// exclusively, so requiring one turns "the caller says this is bone X" into
/// "rite agrees this caller owns bone X", which the caller cannot forge.
fn holds_bone_claim(ctx: &ProtocolContext, bone_id: &str) -> bool {
    ctx.held_bone_claims()
        .iter()
        .any(|(bone, _)| *bone == bone_id)
}

/// Try to find the bone associated with a workspace.
///
/// Checks the workspace claim's memo, then all held bone claims (for workers with
/// one bone), then the workspace name itself — the dev-loop names workspaces after
/// the bone they serve. Every method that takes its answer from a caller-supplied
/// string is corroborated by [`holds_bone_claim`].
///
/// Returning `None` leaves the review gate with nothing to match against, which
/// keeps the merge blocked as `NeedsReview` rather than letting it through.
///
/// `exact` drops Method 2. A sole bone claim says which bone the caller holds,
/// not which workspace it belongs to, so under `--force` it would check an
/// unrelated bone's labels for this workspace.
fn find_bone_for_workspace(ctx: &ProtocolContext, workspace: &str, exact: bool) -> Option<String> {
    // Method 1: the workspace claim's memo names the bone it was staked for.
    //
    // Two filters, both load-bearing. `claim.agent == ctx.agent()` because
    // `rite claims list` returns EVERY agent's claims and ignores --agent, so
    // without it another agent's memo could name the bone that gates our merge
    // (held_bone_claims and context.rs::workspace_for_bone both filter on agent
    // for this reason). `holds_bone_claim` because the memo is free text the
    // staker chose: stake workspace://<proj>/<my-ws> with the memo set to a
    // victim's approved bone and, unfiltered, this returns that bone and its LGTM
    // gates unreviewed code in my-ws.
    //
    // Today `Claim.memo` never populates — rite emits the field as "message" and
    // `Claim` has no serde alias for it — so this path is currently dead. That is
    // a deserialization accident, not an access control: the day someone adds the
    // alias, these checks are what stands between the memo and the gate.
    for claim in ctx.claims() {
        if claim.agent != ctx.agent() {
            continue;
        }
        if let Some(memo) = &claim.memo {
            for pattern in &claim.patterns {
                if let Some(ws_name) = pattern
                    .strip_prefix("workspace://")
                    .and_then(|rest| rest.split('/').nth(1))
                    && ws_name == workspace
                    && holds_bone_claim(ctx, memo)
                {
                    return Some(memo.clone());
                }
            }
        }
    }

    // Method 2: if there's exactly one bone claim, use that. Already corroborated —
    // held_bone_claims() only returns claims this agent holds.
    let bone_claims = ctx.held_bone_claims();
    if !exact && bone_claims.len() == 1 {
        return Some(bone_claims[0].0.to_string());
    }

    // Method 3: workspaces are created as `maw ws create <bone-id>`, so the name is a
    // strong hint — but only a hint. Taking it at face value would let the bone that
    // gates this merge be chosen by an unverified, agent-supplied STRING: name a
    // workspace after someone else's approved bone and it inherits that approval.
    // Require the caller to actually hold the bone's claim, which rite grants
    // exclusively, so the name is corroborated by something it cannot forge.
    if shell::validate_bone_id(workspace).is_ok() && holds_bone_claim(ctx, workspace) {
        return Some(workspace.to_string());
    }

    None
}

/// Execute merge steps and render the execution report.
///
/// Runs `--check` pre-flight before executing. Falls back to WARNING pattern
/// detection if --check is unavailable.
fn execute_and_render(
    guidance: &ProtocolGuidance,
    workspace: &str,
    merge_msg: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    use super::executor;

    let report = executor::execute_steps(&guidance.steps)
        .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;

    // Fallback conflict detection via WARNING pattern (safety net)
    let merge_had_conflicts = report.results.iter().any(|r| {
        r.stdout.contains("WARNING: Merge has conflicts")
            || r.stdout.contains("conflict(s) remaining")
    });

    if merge_had_conflicts {
        let mut conflict_guidance = ProtocolGuidance::new("merge");
        conflict_guidance.set_layout(guidance.layout);
        conflict_guidance.workspace = Some(workspace.to_string());
        conflict_guidance.status = ProtocolStatus::Blocked;
        conflict_guidance.diagnostic(format!(
            "Merge completed with CONFLICTS. Workspace {workspace} is preserved (not destroyed)."
        ));
        let review_id = guidance.review.as_ref().map(|r| r.review_id.as_str());
        add_conflict_recovery_guidance(
            &mut conflict_guidance,
            workspace,
            None,
            merge_msg,
            review_id.map(|rid| (rid, true)),
        );

        let output = render::render(&conflict_guidance, format)
            .map_err(|e| anyhow::anyhow!("render error: {e}"))?;
        println!("{output}");
        std::process::exit(1);
    }

    let output = executor::render_report(&report, format);
    println!("{output}");

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A stale approval (new commits landed since the LGTM) must retarget the
    /// review to the current commit before re-requesting (bn-w912):
    /// `seal reviews request` alone leaves the review's target commit pinned
    /// at the old, already-approved anchor.
    #[test]
    fn stale_approval_steps_retargets_before_re_requesting() {
        let steps = stale_approval_steps(
            "frost-castle",
            "cr-123",
            &["edict-security".to_string()],
            "crimson-storm",
        );

        let retarget_pos = steps
            .iter()
            .position(|s| s.contains("seal reviews retarget cr-123"))
            .expect("retarget step present");
        let request_pos = steps
            .iter()
            .position(|s| s.contains("seal reviews request cr-123"))
            .expect("request step present");
        assert!(
            retarget_pos < request_pos,
            "retarget must come before re-request: {steps:?}"
        );
    }

    /// `maw ws merge` refuses a stale source outright (maw 1.0.0-pre.16
    /// `merge.rs` ~L1555: "Workspace '<ws>' is stale … To fix: maw ws sync
    /// <ws>"), it does not auto-sync it. The conflict-recovery diagnostic must
    /// say so and point at `maw ws sync`, not claim staleness is harmless.
    #[test]
    fn conflict_recovery_guidance_tells_the_truth_about_stale_sources() {
        let mut guidance = ProtocolGuidance::new("merge");
        add_conflict_recovery_guidance(&mut guidance, "frost-castle", None, "feat: x", None);

        let diag = guidance.diagnostics.join("\n");
        assert!(
            diag.contains("maw ws sync frost-castle"),
            "must point at `maw ws sync` for a stale source: {diag}"
        );
        assert!(
            !diag.contains("auto-syncs stale sources"),
            "must not repeat the false auto-sync claim: {diag}"
        );
    }

    #[test]
    fn test_build_merge_steps_basic() {
        let mut guidance = ProtocolGuidance::new("merge");
        guidance.workspace = Some("frost-castle".to_string());

        build_merge_steps(
            &mut guidance,
            &MergeStepsParams {
                workspace: "frost-castle",
                project: "myproject",
                message: "feat: add login flow",
                merge_target: None,
                bone_id: Some("bd-abc"),
                review_id: Some("cr-123"),
                push_main: true,
                agent: "edict-dev",
            },
        );

        // Should have merge, mark-merged, sync, push, announce
        assert!(guidance.steps.len() >= 4);
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("maw ws merge frost-castle --into default --destroy"))
        );
        // Should include the required --message
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("--message") && s.contains("feat: add login flow"))
        );
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("seal reviews mark-merged cr-123"))
        );
        // br sync removed — bones is event-sourced
        assert!(guidance.steps.iter().any(|s| s.contains("maw push")));
        assert!(guidance.steps.iter().any(|s| s.contains("task-done")));

        // Should include conflict recovery guidance
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("maw ws conflicts"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("maw ws resolve"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("maw ws merge --abort"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("maw undo") && d.contains("undo a COMPLETED merge"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("maw ws recover"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("Conflict recovery"))
        );
    }

    /// The merge destroys the workspace, so every review step must run in the
    /// workspace before it. Run after, `mark-merged` hits a missing workspace
    /// and the review log never reaches trunk.
    #[test]
    fn test_build_merge_steps_records_review_before_merge() {
        let mut guidance = ProtocolGuidance::new("merge");
        build_merge_steps(
            &mut guidance,
            &MergeStepsParams {
                workspace: "frost-castle",
                project: "myproject",
                message: "feat: add login flow",
                merge_target: None,
                bone_id: Some("bd-abc"),
                review_id: Some("cr-123"),
                push_main: false,
                agent: "edict-dev",
            },
        );

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
        let add = pos("maw exec frost-castle -- git add .seal/reviews/cr-123");
        let commit = pos("git commit -m 'chore: seal review cr-123' -- .seal/reviews/cr-123");
        let merge = pos("maw ws merge frost-castle");
        assert!(mark < add && add < commit && commit < merge);
        // The merge step repeats the clean check in the same shell command.
        assert!(guidance.steps[merge].starts_with("{ out=$(maw exec frost-castle"));
        assert!(guidance.steps[merge].contains("&& maw ws merge frost-castle"));
        assert!(
            !guidance
                .steps
                .iter()
                .any(|s| s.contains("maw exec default -- seal"))
        );
    }

    #[test]
    fn test_build_merge_steps_no_push() {
        let mut guidance = ProtocolGuidance::new("merge");

        build_merge_steps(
            &mut guidance,
            &MergeStepsParams {
                workspace: "frost-castle",
                project: "myproject",
                message: "chore: update deps",
                merge_target: None,
                bone_id: None,
                review_id: None,
                push_main: false, // push_main = false
                agent: "edict-dev",
            },
        );

        // Should NOT have push
        assert!(!guidance.steps.iter().any(|s| s.contains("maw push")));
        // Should NOT have mark-merged (no review_id)
        assert!(!guidance.steps.iter().any(|s| s.contains("mark-merged")));
        // Should still have merge, sync, announce
        assert!(guidance.steps.iter().any(|s| s.contains("maw ws merge")));
        // br sync removed — bones is event-sourced
    }

    #[test]
    fn test_merge_check_result_parsing_ready() {
        let json = r#"{"status": "clean", "workspaces": ["frost-castle"], "has_conflicts": false, "conflicts": [], "message": "safe to merge"}"#;
        let result: MergeCheckResult = serde_json::from_str(json).unwrap();
        assert!(result.is_ready());
        assert!(result.conflicts.is_empty());
        assert!(!result.stale);
    }

    #[test]
    fn test_merge_check_result_parsing_conflicts() {
        let json = r#"{"status": "blocked", "has_conflicts": true, "conflicts": ["src/main.rs", "src/lib.rs"], "message": "conflicts detected"}"#;
        let result: MergeCheckResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_ready());
        assert_eq!(result.conflicts.len(), 2);
        assert_eq!(result.conflict_labels()[0], "src/main.rs");
    }

    #[test]
    fn test_merge_check_result_parsing_stale() {
        let json = r#"{"status": "blocked", "has_conflicts": false, "conflicts": [], "stale": true, "message": "workspace is stale"}"#;
        let result: MergeCheckResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_ready());
        assert!(result.stale);
    }

    #[test]
    fn test_merge_check_result_extra_fields_tolerated() {
        let json =
            r#"{"status": "clean", "has_conflicts": false, "conflicts": [], "new_field": 42}"#;
        let result: MergeCheckResult = serde_json::from_str(json).unwrap();
        assert!(result.is_ready());
    }

    #[test]
    fn test_build_merge_steps_announce_includes_bone() {
        let mut guidance = ProtocolGuidance::new("merge");

        build_merge_steps(
            &mut guidance,
            &MergeStepsParams {
                workspace: "frost-castle",
                project: "myproject",
                message: "feat: announce test",
                merge_target: Some("ch-123"),
                bone_id: Some("bd-abc"),
                review_id: None,
                push_main: false,
                agent: "edict-dev",
            },
        );

        let announce = guidance
            .steps
            .iter()
            .find(|s| s.contains("rite send"))
            .unwrap();
        assert!(announce.contains("bd-abc"));
        assert!(guidance.steps.iter().any(|s| s.contains("--into ch-123")));
    }

    /// Build a context whose claims come from raw rite JSON.
    fn ctx_with_claims(agent: &str, claims_json: &str) -> ProtocolContext {
        let claims = super::super::adapters::parse_claims(claims_json)
            .expect("claims fixture parses")
            .claims;
        ProtocolContext::for_test(agent, claims, Vec::new())
    }

    /// The bone that gates a merge decides WHOSE approval is consulted, so it may
    /// never be taken from a string the caller picked. Both the workspace name and
    /// the claim memo are caller-chosen; each must be corroborated by a bone claim,
    /// which rite grants exclusively.
    #[test]
    fn workspace_name_needs_a_held_bone_claim() {
        // Agent holds the bone whose name the workspace carries: trusted.
        let ctx = ctx_with_claims(
            "crimson-storm",
            r#"{"claims": [
                {"agent": "crimson-storm", "patterns": ["bone://edict/bn-24r"], "active": true}
            ]}"#,
        );
        assert_eq!(
            find_bone_for_workspace(&ctx, "bn-24r", false).as_deref(),
            Some("bn-24r")
        );

        // The attack: name a workspace after someone ELSE's approved bone. Without a
        // claim on it, the name is just a string, and inheriting that bone's LGTM
        // would merge unreviewed code. Fail closed: no bone -> gate blocks.
        let ctx = ctx_with_claims(
            "crimson-storm",
            r#"{"claims": [
                {"agent": "green-vertex", "patterns": ["bone://edict/bn-24r"], "active": true}
            ]}"#,
        );
        assert_eq!(find_bone_for_workspace(&ctx, "bn-24r", false), None);
    }

    /// Method 1 (memo) runs BEFORE the workspace-name path, so it needs the same
    /// corroboration — otherwise hardening only the later path is unreachable code.
    /// `Claim.memo` does not currently deserialize (rite emits the field as
    /// "message"), so these fixtures set it explicitly: the checks must hold on the
    /// day that is fixed, not depend on the bug for their safety.
    #[test]
    fn claim_memo_needs_a_held_bone_claim() {
        // Memo names a bone this agent does not hold: refuse it.
        let mut claims = super::super::adapters::parse_claims(
            r#"{"claims": [
                {"agent": "crimson-storm", "patterns": ["workspace://edict/my-ws"], "active": true},
                {"agent": "green-vertex", "patterns": ["bone://edict/bn-victim"], "active": true}
            ]}"#,
        )
        .unwrap()
        .claims;
        claims[0].memo = Some("bn-victim".to_string());
        let ctx = ProtocolContext::for_test("crimson-storm", claims, Vec::new());
        assert_eq!(
            find_bone_for_workspace(&ctx, "my-ws", false),
            None,
            "a claim memo must not nominate a bone the caller does not hold"
        );

        // Memo names a bone this agent does hold: trusted.
        let mut claims = super::super::adapters::parse_claims(
            r#"{"claims": [
                {"agent": "crimson-storm", "patterns": ["workspace://edict/my-ws"], "active": true},
                {"agent": "crimson-storm", "patterns": ["bone://edict/bn-mine"], "active": true}
            ]}"#,
        )
        .unwrap()
        .claims;
        claims[0].memo = Some("bn-mine".to_string());
        let ctx = ProtocolContext::for_test("crimson-storm", claims, Vec::new());
        assert_eq!(
            find_bone_for_workspace(&ctx, "my-ws", false).as_deref(),
            Some("bn-mine")
        );
    }

    /// Under --force only an exact workspace-to-bone binding counts. A sole bone
    /// claim does not say which workspace it belongs to, so it must not supply
    /// the labels that the risk:critical check reads.
    #[test]
    fn exact_lookup_ignores_a_sole_unrelated_bone_claim() {
        let claims = super::super::adapters::parse_claims(
            r#"{"claims": [
                {"agent": "crimson-storm", "patterns": ["bone://edict/bn-mine"], "active": true}
            ]}"#,
        )
        .unwrap()
        .claims;
        let ctx = ProtocolContext::for_test("crimson-storm", claims, Vec::new());
        assert_eq!(
            find_bone_for_workspace(&ctx, "other-ws", false).as_deref(),
            Some("bn-mine")
        );
        assert_eq!(find_bone_for_workspace(&ctx, "other-ws", true), None);
        // A workspace named after the held bone is an exact binding.
        assert_eq!(
            find_bone_for_workspace(&ctx, "bn-mine", true).as_deref(),
            Some("bn-mine")
        );
    }

    /// `rite claims list` returns every agent's claims and ignores --agent, so an
    /// unfiltered scan would let another agent's memo choose the gating bone.
    #[test]
    fn another_agents_claim_memo_is_ignored() {
        let mut claims = super::super::adapters::parse_claims(
            r#"{"claims": [
                {"agent": "green-vertex", "patterns": ["workspace://edict/my-ws"], "active": true},
                {"agent": "crimson-storm", "patterns": ["bone://edict/bn-mine"], "active": true}
            ]}"#,
        )
        .unwrap()
        .claims;
        // Another agent staked a claim on our workspace, memo pointing at our bone.
        claims[0].memo = Some("bn-mine".to_string());
        let ctx = ProtocolContext::for_test("crimson-storm", claims, Vec::new());

        // Method 1 must skip it. Method 2 then resolves the bone from OUR single
        // held bone claim, which is corroborated — so the answer is right, but it
        // came from a source the caller cannot forge.
        assert_eq!(
            find_bone_for_workspace(&ctx, "my-ws", false).as_deref(),
            Some("bn-mine")
        );

        // With no bone claim of our own, nothing corroborates the foreign memo.
        let mut claims = super::super::adapters::parse_claims(
            r#"{"claims": [
                {"agent": "green-vertex", "patterns": ["workspace://edict/my-ws"], "active": true}
            ]}"#,
        )
        .unwrap()
        .claims;
        claims[0].memo = Some("bn-victim".to_string());
        let ctx = ProtocolContext::for_test("crimson-storm", claims, Vec::new());
        assert_eq!(find_bone_for_workspace(&ctx, "my-ws", false), None);
    }

    /// `bone_status` fails fast on a malformed ID (no subprocess needed), which
    /// is enough to exercise `check_bone_gate`'s Err arm deterministically. The
    /// same malformed ID also fails `print_guidance`'s own bone-id validation
    /// (an unrelated shell-safety check, elsewhere in this module), so the
    /// call's `Result` isn't meaningful here — what matters is that `guidance`
    /// was already flipped to `Blocked` before that render attempt, proving
    /// the gate decided to block rather than silently proceeding.
    #[test]
    fn check_bone_gate_fails_closed_when_bone_status_unresolvable() {
        let ctx = ProtocolContext::for_test("crimson-storm", Vec::new(), Vec::new());
        let mut guidance = ProtocolGuidance::new("merge");
        let _ = check_bone_gate(
            &mut guidance,
            &ctx,
            Some("not_a_valid_id"),
            false,
            OutputFormat::Json,
        );
        assert_eq!(guidance.status, ProtocolStatus::Blocked);
    }

    /// --force skips the review gate, so only the bone's labels can reveal
    /// risk:critical. A bone whose labels cannot be loaded must block even
    /// under --force, or critical work merges without human approval.
    #[test]
    fn check_bone_gate_force_blocks_unresolvable_bone() {
        let ctx = ProtocolContext::for_test("crimson-storm", Vec::new(), Vec::new());
        let mut guidance = ProtocolGuidance::new("merge");
        // The renderer rejects the malformed id, so check the guidance state.
        let _ = check_bone_gate(
            &mut guidance,
            &ctx,
            Some("not_a_valid_id"),
            true,
            OutputFormat::Json,
        );
        assert_eq!(guidance.status, ProtocolStatus::Blocked);
    }

    #[test]
    fn check_bone_gate_force_blocks_missing_bone() {
        let ctx = ProtocolContext::for_test("crimson-storm", Vec::new(), Vec::new());
        let mut guidance = ProtocolGuidance::new("merge");
        let stop = check_bone_gate(&mut guidance, &ctx, None, true, OutputFormat::Json).unwrap();
        assert!(stop);
        assert_eq!(guidance.status, ProtocolStatus::Blocked);
        assert!(guidance.steps.iter().all(|s| !s.contains("maw ws merge")));
    }

    #[test]
    fn check_bone_gate_missing_bone_without_force_defers_to_review_gate() {
        let ctx = ProtocolContext::for_test("crimson-storm", Vec::new(), Vec::new());
        let mut guidance = ProtocolGuidance::new("merge");
        let stop = check_bone_gate(&mut guidance, &ctx, None, false, OutputFormat::Json).unwrap();
        assert!(!stop);
    }

    /// bn-25cq: merge's printed commands act as the real agent.
    #[test]
    fn merge_output_uses_the_real_agent_name() {
        let mut guidance = ProtocolGuidance::new("merge");
        build_merge_steps(
            &mut guidance,
            &MergeStepsParams {
                workspace: "frost-castle",
                project: "myproject",
                message: "feat: x",
                merge_target: None,
                bone_id: Some("bd-abc"),
                review_id: Some("cr-123"),
                push_main: false,
                agent: "edict-dev",
            },
        );
        assert!(guidance.steps.iter().all(|s| !s.contains("--agent agent")));
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.starts_with("rite send --agent edict-dev"))
        );

        let mut guidance = ProtocolGuidance::new("merge");
        handle_missing_review(
            &mut guidance,
            "frost-castle",
            Some("bd-abc"),
            &["myproject-security".to_string()],
            "edict-dev",
            OutputFormat::Json,
        )
        .unwrap();
        assert!(guidance.steps.iter().all(|s| !s.contains("--agent agent")));
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("--agent edict-dev"))
        );
    }

    /// bn-2cyu: the stale diagnostic names what seal missed, and keeps seal's
    /// own counts when seal itself reported the approval stale.
    #[test]
    fn stale_diagnostic_names_uncovered_paths_or_seal_counts() {
        let summary = adapters::ReviewDiffSummary {
            target_commit: Some("c658bbf939ffae9f".into()),
            approval_stale: Some(false),
            approved_commit: Some("c658bbf939ffae9f".into()),
            uncovered_commits: Some(0),
        };
        let edict_found = review_gate::FreshnessCheck {
            freshness: review_gate::CommitFreshness::Stale,
            uncovered_paths: vec!["src/main.rs".into(), "src/shout.rs".into()],
            note: None,
        };
        let d = stale_approval_diagnostic("cr-1", "ws1", Some(&summary), &edict_found);
        assert!(d.contains("src/main.rs, src/shout.rs"), "{d}");
        assert!(d.contains("c658bbf939ff"), "{d}");
        assert!(!d.contains("0 commit(s)"), "{d}");
        assert!(d.contains("approval_stale:false"), "{d}");

        let seal_found = review_gate::FreshnessCheck {
            freshness: review_gate::CommitFreshness::Stale,
            uncovered_paths: Vec::new(),
            note: None,
        };
        let stale = adapters::ReviewDiffSummary {
            approval_stale: Some(true),
            uncovered_commits: Some(2),
            ..summary
        };
        let d = stale_approval_diagnostic("cr-1", "ws1", Some(&stale), &seal_found);
        assert!(d.contains("2 commit(s) not covered"), "{d}");
        assert!(d.contains("refuses this"), "{d}");
    }
}
