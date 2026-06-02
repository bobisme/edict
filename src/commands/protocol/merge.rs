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
    let review_id = if review_enabled {
        find_review_for_workspace(&ctx, workspace).map(|(id, _)| id)
    } else {
        None
    };

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
    if let Some((review_id, review_detail)) = find_review_for_workspace(ctx, workspace) {
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
        guidance
            .diagnostic("Review is enabled but no review exists for this workspace.".to_string());
        guidance.advise(
            "Create a review before merging, or use --force to skip review gate.".to_string(),
        );

        let mut steps = Vec::new();
        let title = bone_id.map_or_else(
            || format!("Work from {workspace}"),
            |id| format!("Work from {id}"),
        );
        steps.push(shell::seal_create_cmd(
            workspace,
            "agent",
            &title,
            &required_reviewers.join(","),
        ));
        guidance.steps(steps);

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
                    guidance
                        .diagnostic("Workspace is stale — run `maw ws sync` first.".to_string());
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
        "Conflict recovery — workspace is preserved (not destroyed). Steps:\n\
         \n\
         1. Inspect conflicts and stale state:\n\
         \n\
         maw ws conflicts {workspace} --format json\n\
         {check_cmd}\n\
         maw ws sync {workspace}\n\
         \n\
         2. For auto-resolvable files (.bones/, .claude/, .agents/):\n\
         \n\
         maw exec {workspace} -- git restore --source refs/heads/main -- .bones/ .claude/ .agents/\n\
         \n\
         3. For code conflicts — resolve, stage, and commit in workspace:\n\
         \n\
         maw exec {workspace} -- git status\n\
         maw exec {workspace} -- git add <resolved-file>\n\
         maw exec {workspace} -- git commit -m 'resolve: merge conflicts in {workspace}'\n\
         \n\
         4. After resolving:\n\
         \n\
         {retry_cmd}              # retry merge\n\
         \n\
         5. To UNDO the merge entirely (recover pre-merge state):\n\
         \n\
         maw ws undo {workspace}                         # reset workspace to its base\n\
         \n\
         6. To recover a destroyed workspace:\n\
         \n\
         maw ws recover {workspace} --to {workspace}-recovered    # recreate it under a new name",
    ));
}

/// Try to find the bone associated with a workspace.
///
/// Checks claims first (workspace claim memo = bone ID), then falls back
/// to checking all held bone claims (for workers with one bone).
fn find_bone_for_workspace(ctx: &ProtocolContext, workspace: &str) -> Option<String> {
    // Method 1: check workspace claims for memo (when rite includes memo in JSON)
    for claim in ctx.claims() {
        if let Some(memo) = &claim.memo {
            for pattern in &claim.patterns {
                if let Some(ws_name) = pattern
                    .strip_prefix("workspace://")
                    .and_then(|rest| rest.split('/').nth(1))
                    && ws_name == workspace
                {
                    return Some(memo.clone());
                }
            }
        }
    }

    // Method 2: if there's exactly one bone claim, use that
    let bone_claims = ctx.held_bone_claims();
    if bone_claims.len() == 1 {
        return Some(bone_claims[0].0.to_string());
    }

    None
}

/// Try to find a review for a workspace.
fn find_review_for_workspace(
    ctx: &ProtocolContext,
    workspace: &str,
) -> Option<(String, super::adapters::ReviewDetail)> {
    let output = std::process::Command::new("maw")
        .args([
            "exec", workspace, "--", "seal", "reviews", "list", "--format", "json",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let reviews_resp = super::adapters::parse_reviews_list(&stdout).ok()?;

    for review_summary in &reviews_resp.reviews {
        if review_summary.status != "merged"
            && let Ok(detail) = ctx.review_status(&review_summary.review_id, workspace)
        {
            return Some((review_summary.review_id.clone(), detail));
        }
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
                .any(|d| d.contains("maw ws undo"))
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
}
