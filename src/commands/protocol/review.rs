//! Protocol review command: check state and output commands to request review.
//!
//! Resolves bone claim, workspace, existing review status, and reviewer list
//! to produce guidance for creating or following up on a code review.

use super::context::ProtocolContext;
use super::executor;
use super::render::{BoneRef, ProtocolGuidance, ProtocolStatus, ReviewRef};
use super::review_gate::{self, ReviewGateStatus};
use super::shell;
use crate::commands::doctor::OutputFormat;
use crate::config::Config;

/// Execute review protocol: check state and output review guidance.
pub fn execute(
    bone_id: &str,
    reviewers_override: Option<&str>,
    review_id_flag: Option<&str>,
    execute: bool,
    agent: &str,
    project: &str,
    config: &Config,
    format: OutputFormat,
) -> anyhow::Result<()> {
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
    let workspace = match ctx.workspace_for_bone(bone_id) {
        Some(ws) => ws.to_string(),
        None => {
            guidance.blocked(format!(
                "no workspace claim found for bone {bone_id}. \
                 Create workspace and stake claim first."
            ));
            print_guidance(&guidance, format)?;
            return Ok(());
        }
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

    // Check for existing review in the workspace
    match ctx.reviews_in_workspace(&workspace) {
        Ok(reviews) if !reviews.is_empty() => {
            // Use the first open review found
            let existing = &reviews[0];
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
        Ok(_) => {
            // No existing review — output creation commands
        }
        Err(e) => {
            // Listing failed — proceed to create a new review
            guidance.diagnostic(format!(
                "Could not list existing reviews ({e}); proceeding with creation."
            ));
        }
    }

    // No review exists: output seal reviews create + rite announce commands
    guidance.status = ProtocolStatus::NeedsReview;

    let reviewers_str = reviewer_names.join(",");
    let title = format!("{bone_id}: {}", bone_info.title);

    let mut steps = Vec::new();
    steps.push(shell::seal_create_cmd(
        &workspace,
        agent,
        &title,
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

    print_guidance(&guidance, format)?;
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
            guidance.advise(
                "Read review feedback, address issues, then re-request review.".to_string(),
            );
        }
        ReviewGateStatus::NeedsReview => {
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
        }
    }

    print_guidance(guidance, format)?;
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
    let names: Vec<String> = if let Some(override_str) = reviewers_override {
        override_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        config
            .review
            .reviewers
            .iter()
            .map(|role| format!("{project}-{role}"))
            .collect()
    };

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
    println!("{}", output);
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
                reviewers: reviewers.into_iter().map(|s| s.to_string()).collect(),
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
