//! Protocol merge command: lead-facing command to check preconditions and
//! output merge steps for a worker's completed workspace.
//!
//! Validates: workspace exists, has changes, associated bone is closed,
//! review is approved (if enabled). Outputs merge steps with conflict
//! recovery guidance.

use std::io::IsTerminal;

use anyhow::Context;
use serde::Deserialize;

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
pub fn execute(
    workspace: &str,
    message: &str,
    force: bool,
    execute: bool,
    agent: &str,
    project: &str,
    config: &Config,
    format: OutputFormat,
) -> anyhow::Result<()> {
    // Reject merging default workspace
    if workspace == "default" {
        let mut guidance = ProtocolGuidance::new("merge");
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
            guidance.blocked(format!("failed to collect state: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    let mut guidance = ProtocolGuidance::new("merge");
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
    let bone_id = find_bone_for_workspace(&ctx, workspace);

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
    match ctx.bone_status(bone_id) {
        Ok(bone_info) => {
            guidance.bone = Some(render::BoneRef {
                id: bone_id.to_string(),
                title: bone_info.title.clone(),
            });

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
        }
        Err(_) => {
            guidance.diagnostic(format!(
                "Could not fetch bone {bone_id} — it may have been deleted. Proceeding with merge."
            ));
        }
    }

    Ok(false)
}

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
        let decision = review_gate::evaluate_review_gate(&review_detail, required_reviewers);
        if merge_target.is_none() {
            *merge_target = review_detail.change_id;
        }
        guidance.review = Some(render::ReviewRef {
            review_id: review_id.clone(),
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
                guidance.advise("Address reviewer feedback, then re-request review.".to_string());

                let steps = vec![shell::seal_show_cmd(workspace, &review_id)];
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
                guidance
                    .advise("Wait for reviewers or re-request review before merging.".to_string());

                let steps = vec![shell::seal_show_cmd(workspace, &review_id)];
                guidance.steps(steps);

                print_guidance(guidance, format)?;
                return Ok(true);
            }
        }
    } else {
        guidance.status = ProtocolStatus::NeedsReview;

        if let Some(id) = bone_id {
            guidance.diagnostic(format!(
                "Review is enabled but no live review exists for bone {id}."
            ));
            guidance.advise("Create a review before merging.".to_string());

            let mut steps = Vec::new();
            steps.push(shell::seal_create_cmd(
                workspace,
                "agent",
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
        return Ok(true);
    }

    Ok(false)
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
                    guidance.diagnostic(
                        "Workspace is stale. `maw ws merge` auto-syncs stale sources before \
                         merging, so this alone does not block the merge — a manual `maw ws sync` \
                         is only needed if the auto-sync itself reported conflicts."
                            .to_string(),
                    );
                }
                if let Some(message) = check.message.as_deref() {
                    guidance.diagnostic(message.to_string());
                }
                add_conflict_recovery_guidance(guidance, workspace, merge_target, message);
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
}

/// Build the merge steps: merge, mark-merged, sync, push.
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
    } = *params;

    let mut steps = Vec::new();

    // 1. Merge workspace with the required commit message
    let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
    steps.push(shell::ws_merge_cmd(workspace, target, message));

    // 2. Mark review as merged (if review exists)
    if let Some(rid) = review_id {
        steps.push(format!(
            "maw exec default -- seal reviews mark-merged {rid}"
        ));
    }

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
        "agent",
        project,
        &announce_msg,
        "task-done",
    ));

    guidance.steps(steps);

    // Add conflict recovery guidance
    add_conflict_recovery_guidance(guidance, workspace, merge_target, message);
}

/// Append comprehensive maw/git conflict recovery guidance as diagnostics.
fn add_conflict_recovery_guidance(
    guidance: &mut ProtocolGuidance,
    workspace: &str,
    merge_target: Option<&str>,
    merge_msg: &str,
) {
    let target = merge_target.map_or(shell::MergeTarget::Default, shell::MergeTarget::Change);
    let retry_cmd = shell::ws_merge_cmd(workspace, target, merge_msg);
    let check_cmd = shell::ws_merge_check_cmd(workspace, target);
    guidance.diagnostic(format!(
        "Conflict recovery — workspace is preserved (not destroyed). Conflicts are data, not \
         failure: merge auto-syncs stale sources, so a bare staleness report is not a blocker. \
         Steps:\n\
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
fn find_bone_for_workspace(ctx: &ProtocolContext, workspace: &str) -> Option<String> {
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
    if bone_claims.len() == 1 {
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
        conflict_guidance.workspace = Some(workspace.to_string());
        conflict_guidance.status = ProtocolStatus::Blocked;
        conflict_guidance.diagnostic(format!(
            "Merge completed with CONFLICTS. Workspace {workspace} is preserved (not destroyed)."
        ));
        add_conflict_recovery_guidance(&mut conflict_guidance, workspace, None, merge_msg);

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
            find_bone_for_workspace(&ctx, "bn-24r").as_deref(),
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
        assert_eq!(find_bone_for_workspace(&ctx, "bn-24r"), None);
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
            find_bone_for_workspace(&ctx, "my-ws"),
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
            find_bone_for_workspace(&ctx, "my-ws").as_deref(),
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
            find_bone_for_workspace(&ctx, "my-ws").as_deref(),
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
        assert_eq!(find_bone_for_workspace(&ctx, "my-ws"), None);
    }
}
