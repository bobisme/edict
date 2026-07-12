//! Protocol review command: check state and output commands to request review.
//!
//! Resolves bone claim, workspace, existing review status, and reviewer list
//! to produce guidance for creating or following up on a code review.

use super::context::ProtocolContext;
use super::executor;
use super::render::{BoneRef, ProtocolGuidance, ProtocolStatus, ReviewRef};
use super::review_gate::{self, ReviewGateStatus};
use super::review_select;
use super::shell;
use crate::commands::doctor::OutputFormat;
use crate::config::Config;

/// Parameters for [`execute`].
pub struct ReviewParams<'a> {
    pub bone_id: &'a str,
    pub reviewers_override: Option<&'a str>,
    pub review_id_flag: Option<&'a str>,
    pub execute: bool,
    pub agent: &'a str,
    pub project: &'a str,
    pub config: &'a Config,
    pub format: OutputFormat,
}

/// Execute review protocol: check state and output review guidance.
///
/// # Errors
///
/// Returns `Err` if collecting protocol context, resolving reviewers, or
/// rendering/executing the review steps fails.
#[allow(
    clippy::too_many_lines,
    reason = "sequential review-protocol state machine; sub-steps already extracted into helpers"
)]
pub fn execute(params: &ReviewParams) -> anyhow::Result<()> {
    let &ReviewParams {
        bone_id,
        reviewers_override,
        review_id_flag,
        execute,
        agent,
        project,
        config,
        format,
    } = params;
    // Early input validation before any subprocess calls
    if let Err(e) = shell::validate_bone_id(bone_id) {
        anyhow::bail!("invalid bone ID: {e}");
    }

    let ctx = ProtocolContext::collect(project, agent)?;

    let mut guidance = ProtocolGuidance::new("review");
    guidance.set_freshness(300, Some(format!("edict protocol review {bone_id}")));

    // Fetch bone info
    let bone_info = match ctx.bone_status(bone_id) {
        Ok(bone) => bone,
        Err(e) => {
            guidance.blocked(format!("bone {bone_id} not found: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
    };

    guidance.bone = Some(BoneRef {
        id: bone_id.to_string(),
        title: bone_info.title.clone(),
    });

    // Check agent holds bone claim
    let bone_claims = ctx.held_bone_claims();
    let holds_claim = bone_claims.iter().any(|(id, _)| *id == bone_id);
    if !holds_claim {
        guidance.blocked(format!(
            "agent {agent} does not hold claim for bone {bone_id}. \
             Stake a claim first with: {}",
            shell::claims_stake_cmd("agent", &format!("bone://{project}/{bone_id}"), bone_id,)
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
             Create workspace and stake claim first."
        ));
        print_guidance(&guidance, format)?;
        return Ok(());
    };

    // Validate workspace name before it flows into subprocess calls
    if let Err(e) = shell::validate_workspace_name(&workspace) {
        guidance.blocked(format!("invalid workspace name from claims: {e}"));
        print_guidance(&guidance, format)?;
        return Ok(());
    }
    guidance.workspace = Some(workspace.clone());

    // Resolve and validate reviewer names
    let reviewer_names = resolve_reviewers(reviewers_override, config, project)?;

    // If --review-id was provided, check that existing review
    if let Some(rid) = review_id_flag {
        if let Err(e) = shell::validate_review_id(rid) {
            guidance.blocked(format!("invalid review ID: {e}"));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
        return handle_existing_review(
            &ctx,
            &mut guidance,
            rid,
            &workspace,
            &reviewer_names,
            bone_id,
            project,
            agent,
            execute,
            format,
        );
    }

    // Look for a live review belonging to THIS bone. `seal reviews list` is
    // repo-global, so an unscoped pick lands on whichever review sorts first —
    // often a merged one for an unrelated bone, which would sail through the
    // review gate and let unreviewed work merge.
    match ctx.reviews_for_bone(&workspace, bone_id) {
        Ok(reviews) => {
            // Several live reviews for one bone keeps the gate shut until every one of
            // them is signed off (select_for_bone prefers the unapproved candidate).
            // That is the safe direction, but it looks exactly like a reviewer who
            // never showed up, so name the duplicates rather than let the jam be silent.
            if reviews.len() > 1 {
                let ids: Vec<&str> = reviews.iter().map(|r| r.review_id.as_str()).collect();
                guidance.diagnostic(format!(
                    "bone {bone_id} has {} live reviews ({}); the gate stays closed until \
                     each is approved or abandoned.",
                    reviews.len(),
                    ids.join(", ")
                ));
            }
            if let Some(existing) = review_select::select_for_bone(&reviews, bone_id) {
                return handle_existing_review(
                    &ctx,
                    &mut guidance,
                    &existing.review_id,
                    &workspace,
                    &reviewer_names,
                    bone_id,
                    project,
                    agent,
                    execute,
                    format,
                );
            }
            // No live review for this bone — fall through and create one.
        }
        Err(e) => {
            // Listing failed — proceed to create a new review
            guidance.diagnostic(format!(
                "Could not list existing reviews ({e}); proceeding with creation."
            ));
        }
    }

    // No review exists: output seal reviews create + rite announce commands
    build_new_review_guidance(
        &mut guidance,
        &workspace,
        &reviewer_names,
        bone_id,
        &bone_info.title,
        project,
        agent,
        execute,
    )?;

    print_guidance(&guidance, format)?;
    Ok(())
}

/// Build guidance for creating a new review (no existing review found).
#[allow(clippy::too_many_arguments)]
fn build_new_review_guidance(
    guidance: &mut ProtocolGuidance,
    workspace: &str,
    reviewer_names: &[String],
    bone_id: &str,
    bone_title: &str,
    project: &str,
    agent: &str,
    execute: bool,
) -> anyhow::Result<()> {
    guidance.status = ProtocolStatus::NeedsReview;

    let reviewers_str = reviewer_names.join(",");

    let mut steps = Vec::new();
    steps.push(shell::seal_create_cmd(
        workspace,
        agent,
        bone_id,
        bone_title,
        &reviewers_str,
    ));

    // Announce on rite with @mentions for each reviewer
    let mentions: Vec<String> = reviewer_names.iter().map(|r| format!("@{r}")).collect();
    let announce_msg = format!("Review requested: {bone_id} {}", mentions.join(" "));
    steps.push(shell::rite_send_cmd(
        agent,
        project,
        &announce_msg,
        "review-request",
    ));

    if execute {
        // Execute the steps
        let report = executor::execute_steps(&steps)?;
        guidance.executed = true;
        guidance.execution_report = Some(report);
    } else {
        // Just output guidance
        for step in steps {
            guidance.step(step);
        }
    }

    guidance.advise(format!(
        "Create review and announce. Reviewers: {}",
        reviewer_names.join(", ")
    ));

    Ok(())
}

/// Handle an existing review: check its status and output appropriate commands.
#[allow(clippy::too_many_arguments)]
fn handle_existing_review(
    ctx: &ProtocolContext,
    guidance: &mut ProtocolGuidance,
    review_id: &str,
    workspace: &str,
    reviewer_names: &[String],
    bone_id: &str,
    project: &str,
    agent: &str,
    execute: bool,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let review_detail = match ctx.review_status(review_id, workspace) {
        Ok(r) => r,
        Err(e) => {
            guidance.blocked(format!("could not fetch review {review_id}: {e}"));
            print_guidance(guidance, format)?;
            return Ok(());
        }
    };

    // Only a live review can gate work. A merged or abandoned one is closed out --
    // its LGTM votes say nothing about the work sitting in this workspace now --
    // and a status we do not recognize is refused rather than trusted, so seal
    // growing a new state cannot silently re-open the gate.
    if !review_select::is_live_status(&review_detail.status) {
        let status = review_detail.status.trim();
        let status_phrase = if status.is_empty() {
            "of unknown status"
        } else {
            status
        };
        guidance.blocked(format!(
            "review {review_id} is {status_phrase} and cannot gate {bone_id}. \
             Create a fresh review for this bone: {}",
            shell::seal_create_cmd(
                workspace,
                agent,
                bone_id,
                "<title>",
                &reviewer_names.join(","),
            )
        ));
        print_guidance(guidance, format)?;
        return Ok(());
    }

    // Explicit --review-id can still name another bone's review; warn loudly.
    if !review_select::title_matches_bone(review_detail.title.as_deref(), bone_id) {
        guidance.diagnostic(format!(
            "review {review_id} (\"{}\") does not name bone {bone_id} in its title; \
             verify it actually covers this work.",
            review_detail.title.as_deref().unwrap_or("<untitled>"),
        ));
    }

    guidance.review = Some(ReviewRef {
        review_id: review_id.to_string(),
        status: review_detail.status.clone(),
    });

    // Evaluate review gate
    let decision = review_gate::evaluate_review_gate(&review_detail, reviewer_names);

    match decision.status {
        ReviewGateStatus::Approved => {
            // LGTM — advise to proceed to finish (nothing to execute)
            guidance.status = ProtocolStatus::Ready;
            guidance.advise(format!(
                "Review {} approved by {}. Proceed to finish: edict protocol finish {}",
                review_id,
                decision.approved_by.join(", "),
                bone_id,
            ));
        }
        ReviewGateStatus::Blocked => {
            build_blocked_guidance(
                guidance,
                &decision,
                &review_detail,
                review_id,
                workspace,
                reviewer_names,
                project,
                agent,
                execute,
            )?;
        }
        ReviewGateStatus::NeedsReview => {
            build_needs_review_guidance(
                guidance, &decision, review_id, workspace, project, agent, execute,
            )?;
        }
    }

    print_guidance(guidance, format)?;
    Ok(())
}

/// Build guidance for a blocked review: read feedback + re-request commands.
#[allow(clippy::too_many_arguments)]
fn build_blocked_guidance(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_detail: &super::adapters::ReviewDetail,
    review_id: &str,
    workspace: &str,
    reviewer_names: &[String],
    project: &str,
    agent: &str,
    execute: bool,
) -> anyhow::Result<()> {
    // Blocked — output seal review (read feedback) + re-request commands
    guidance.status = ProtocolStatus::Blocked;

    let mut steps = Vec::new();

    // Step 1: Read review feedback
    steps.push(shell::seal_show_cmd(workspace, review_id));

    // Step 2: After addressing feedback, re-request review
    let reviewers_str = reviewer_names.join(",");
    steps.push(shell::seal_request_cmd(
        workspace,
        review_id,
        &reviewers_str,
        agent,
    ));

    // Step 3: Announce re-request on rite
    let mentions: Vec<String> = decision
        .blocked_by
        .iter()
        .map(|r| format!("@{r}"))
        .collect();
    let announce_msg = format!(
        "Review updated: {review_id} — addressed feedback, re-requesting {}",
        mentions.join(" ")
    );
    steps.push(shell::rite_send_cmd(
        agent,
        project,
        &announce_msg,
        "review-request",
    ));

    if execute {
        // Execute the steps
        let report = executor::execute_steps(&steps)?;
        guidance.executed = true;
        guidance.execution_report = Some(report);
    } else {
        // Just output guidance
        for step in steps {
            guidance.step(step);
        }
    }

    guidance.diagnostic(format!(
        "Blocked by: {}. Open threads: {}",
        decision.blocked_by.join(", "),
        review_detail.open_thread_count,
    ));
    guidance.advise("Read review feedback, address issues, then re-request review.".to_string());

    Ok(())
}

/// Build guidance for a review still awaiting required approvals.
fn build_needs_review_guidance(
    guidance: &mut ProtocolGuidance,
    decision: &review_gate::ReviewGateDecision,
    review_id: &str,
    workspace: &str,
    project: &str,
    agent: &str,
    execute: bool,
) -> anyhow::Result<()> {
    // Still waiting for reviews
    guidance.status = ProtocolStatus::NeedsReview;

    if !decision.missing_approvals.is_empty() {
        let mut steps = Vec::new();

        // Re-request from missing reviewers
        let missing_str = decision.missing_approvals.join(",");
        steps.push(shell::seal_request_cmd(
            workspace,
            review_id,
            &missing_str,
            agent,
        ));

        let mentions: Vec<String> = decision
            .missing_approvals
            .iter()
            .map(|r| format!("@{r}"))
            .collect();
        let announce_msg = format!("Review requested: {review_id} {}", mentions.join(" "));
        steps.push(shell::rite_send_cmd(
            agent,
            project,
            &announce_msg,
            "review-request",
        ));

        if execute {
            // Execute the steps
            let report = executor::execute_steps(&steps)?;
            guidance.executed = true;
            guidance.execution_report = Some(report);
        } else {
            // Just output guidance
            for step in steps {
                guidance.step(step);
            }
        }
    }

    guidance.advise(format!(
        "Awaiting review from: {}. {} of {} required reviewers have voted.",
        decision.missing_approvals.join(", "),
        decision.approved_by.len(),
        decision.total_required,
    ));

    Ok(())
}

/// Resolve reviewer names from --reviewers flag or config.
///
/// Reviewers in config are stored as role names (e.g., "security").
/// These are mapped to `<project>-<role>` (e.g., "edict-security").
/// The --reviewers flag overrides with literal reviewer names.
/// All reviewer names are validated against identifier rules.
fn resolve_reviewers(
    reviewers_override: Option<&str>,
    config: &Config,
    project: &str,
) -> anyhow::Result<Vec<String>> {
    let names: Vec<String> = reviewers_override.map_or_else(
        || {
            config
                .review
                .reviewers
                .iter()
                .map(|role| format!("{project}-{role}"))
                .collect()
        },
        |override_str| {
            override_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        },
    );

    // Validate all reviewer names
    for name in &names {
        shell::validate_identifier("reviewer name", name)
            .map_err(|e| anyhow::anyhow!("invalid reviewer: {e}"))?;
    }

    Ok(names)
}

/// Render guidance to stdout.
fn print_guidance(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    let output = super::render::render(guidance, format)
        .map_err(|e| anyhow::anyhow!("render error: {e}"))?;
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(reviewers: Vec<&str>) -> Config {
        Config {
            version: "1.0.0".into(),
            project: crate::config::ProjectConfig {
                name: "edict".into(),
                project_type: vec![],
                languages: vec![],
                default_agent: Some("edict-dev".into()),
                channel: Some("edict".into()),
                install_command: None,
                release_instructions: None,
                check_command: None,
                critical_approvers: None,
            },
            tools: Default::default(),
            review: crate::config::ReviewConfig {
                enabled: true,
                reviewers: reviewers
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            },
            push_main: false,
            agents: Default::default(),
            models: Default::default(),
            env: Default::default(),
        }
    }

    #[test]
    fn resolve_reviewers_from_config() {
        let config = make_config(vec!["security", "perf"]);
        let names = resolve_reviewers(None, &config, "edict").unwrap();
        assert_eq!(names, vec!["edict-security", "edict-perf"]);
    }

    #[test]
    fn resolve_reviewers_override() {
        let config = make_config(vec!["security"]);
        let names = resolve_reviewers(Some("custom-reviewer,another"), &config, "edict").unwrap();
        assert_eq!(names, vec!["custom-reviewer", "another"]);
    }

    #[test]
    fn resolve_reviewers_override_trims_whitespace() {
        let config = make_config(vec![]);
        let names = resolve_reviewers(Some(" a , b , c "), &config, "proj").unwrap();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn resolve_reviewers_empty_config() {
        let config = make_config(vec![]);
        let names = resolve_reviewers(None, &config, "edict").unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn resolve_reviewers_rejects_invalid_names() {
        let config = make_config(vec![]);
        let result = resolve_reviewers(Some("valid,bad name with spaces"), &config, "proj");
        assert!(result.is_err());
    }
}
