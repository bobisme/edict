//! Protocol resume command: check for in-progress work from a previous session.
//!
//! Queries the agent's held claims and correlates with bone status and review
//! state to produce per-bone guidance: continue working, address review feedback,
//! ready to finish, or start fresh.

use super::context::ProtocolContext;
use super::render::{self, BoneRef, ProtocolGuidance, ProtocolStatus, ReviewRef};
use super::review_gate::{self, ReviewGateStatus};
use super::shell;
use crate::commands::doctor::OutputFormat;
use crate::config::Config;

/// Per-bone resume assessment.
struct BoneResume {
    bone_id: String,
    title: String,
    #[allow(dead_code)]
    state: String,
    workspace: Option<String>,
    review: Option<ReviewState>,
}

/// Review state for a held bone.
struct ReviewState {
    review_id: String,
    gate: ReviewGateStatus,
    open_threads: usize,
}

/// Execute the resume protocol command.
pub fn execute(
    agent: &str,
    project: &str,
    config: &Config,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let ctx = ProtocolContext::collect(project, agent)?;

    let bone_claims = ctx.held_bone_claims();

    if bone_claims.is_empty() {
        return render_fresh(agent, format);
    }

    // Assess each held bone
    let mut assessments = Vec::new();
    for (bone_id, _pattern) in &bone_claims {
        let assessment = assess_bone(&ctx, bone_id, config);
        assessments.push(assessment);
    }

    render_resume(&assessments, agent, project, format)
}

/// Assess a single held bone's state.
fn assess_bone(ctx: &ProtocolContext, bone_id: &str, config: &Config) -> BoneResume {
    let (title, state) = match ctx.bone_status(bone_id) {
        Ok(bone) => (bone.title.clone(), bone.state.clone()),
        Err(_) => (String::new(), "unknown".to_string()),
    };

    let workspace = ctx.workspace_for_bone(bone_id).map(|s| s.to_string());

    // Check for reviews in the workspace
    let review = workspace.as_deref().and_then(|ws| {
        let reviews = ctx.reviews_in_workspace(ws).ok()?;
        let review_summary = reviews.into_iter().next()?;
        let detail = ctx.review_status(&review_summary.review_id, ws).ok()?;

        let required_reviewers: Vec<String> = config
            .review
            .reviewers
            .iter()
            .map(|r| format!("{}-{}", config.project.name, r))
            .collect();

        let gate = review_gate::evaluate_review_gate(&detail, &required_reviewers);

        Some(ReviewState {
            review_id: review_summary.review_id,
            gate: gate.status,
            open_threads: detail.open_thread_count,
        })
    });

    BoneResume {
        bone_id: bone_id.to_string(),
        title,
        state,
        workspace,
        review,
    }
}

/// Render guidance when no held claims exist (fresh start).
fn render_fresh(_agent: &str, format: OutputFormat) -> anyhow::Result<()> {
    let mut guidance = ProtocolGuidance::new("resume");
    guidance.status = ProtocolStatus::Fresh;
    guidance.set_freshness(300, Some("edict protocol resume".to_string()));

    guidance.step("maw exec default -- bn next".to_string());

    guidance.advise(
        "No in-progress work found. Run `maw exec default -- bn next` to find available bones."
            .to_string(),
    );

    let output =
        render::render(&guidance, format).map_err(|e| anyhow::anyhow!("render error: {}", e))?;
    println!("{}", output);
    Ok(())
}

/// Render per-bone resume guidance.
fn render_resume(
    assessments: &[BoneResume],
    agent: &str,
    project: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let mut guidance = ProtocolGuidance::new("resume");
    guidance.status = ProtocolStatus::Resumable;
    guidance.set_freshness(300, Some("edict protocol resume".to_string()));

    // If single bone, set it as the primary bone context
    if assessments.len() == 1 {
        let a = &assessments[0];
        guidance.bone = Some(BoneRef {
            id: a.bone_id.clone(),
            title: a.title.clone(),
        });
        if let Some(ref ws) = a.workspace {
            guidance.workspace = Some(ws.clone());
        }
        if let Some(ref review) = a.review {
            guidance.review = Some(ReviewRef {
                review_id: review.review_id.clone(),
                status: match review.gate {
                    ReviewGateStatus::Approved => "approved".to_string(),
                    ReviewGateStatus::Blocked => "blocked".to_string(),
                    ReviewGateStatus::NeedsReview => "needs-review".to_string(),
                },
            });
        }
    }

    for a in assessments {
        // Add a diagnostic header per bone (when multiple)
        if assessments.len() > 1 {
            guidance.diagnostic(format!("--- {} ({}) ---", a.bone_id, a.title));
        }

        build_bone_guidance(&mut guidance, a, agent, project);
    }

    // Summary advice
    let bone_count = assessments.len();
    if bone_count == 1 {
        let a = &assessments[0];
        match (&a.review, a.workspace.as_deref()) {
            (Some(review), _) if review.gate == ReviewGateStatus::Approved => {
                guidance.advise(format!(
                    "Review {} is approved. Ready to finish bone {}.",
                    review.review_id, a.bone_id
                ));
            }
            (Some(review), _) if review.gate == ReviewGateStatus::Blocked => {
                guidance.advise(format!(
                    "Review {} has blocking feedback ({} open thread(s)). Address feedback and request re-review.",
                    review.review_id, review.open_threads
                ));
            }
            (Some(review), _) => {
                guidance.advise(format!(
                    "Review {} is pending. Wait for reviewer feedback or check review status.",
                    review.review_id
                ));
            }
            (None, Some(_ws)) => {
                guidance.advise(format!(
                    "Bone {} is in progress with workspace. Continue implementation.",
                    a.bone_id
                ));
            }
            (None, None) => {
                guidance.advise(format!(
                    "Bone {} is claimed but has no workspace. Create one to continue.",
                    a.bone_id
                ));
            }
        }
    } else {
        guidance.advise(format!(
            "Agent {} has {} in-progress bone(s). Review each and continue or finish as appropriate.",
            agent, bone_count
        ));
    }

    let output =
        render::render(&guidance, format).map_err(|e| anyhow::anyhow!("render error: {}", e))?;
    println!("{}", output);
    Ok(())
}

/// Build guidance steps for a single bone assessment.
fn build_bone_guidance(
    guidance: &mut ProtocolGuidance,
    assessment: &BoneResume,
    _agent: &str,
    project: &str,
) {
    let bead_id = &assessment.bone_id;
    let ws = assessment.workspace.as_deref();

    match (&assessment.review, ws) {
        // Review approved → ready to finish
        (Some(review), Some(ws_name)) if review.gate == ReviewGateStatus::Approved => {
            guidance.step(format!(
                "# {} — review {} approved, ready to finish",
                bead_id, review.review_id
            ));
            guidance.step(shell::seal_show_cmd(ws_name, &review.review_id));
            guidance.step(format!(
                "edict protocol finish {} --project {}",
                bead_id, project
            ));
        }

        // Review blocked → address feedback
        (Some(review), Some(ws_name)) if review.gate == ReviewGateStatus::Blocked => {
            guidance.step(format!(
                "# {} — review {} blocked, address feedback",
                bead_id, review.review_id
            ));
            guidance.step(shell::seal_show_cmd(ws_name, &review.review_id));
            guidance.step(format!(
                "# Fix issues in ws/{ws_name}/, then re-request review:"
            ));
            guidance.step(shell::seal_request_cmd(
                ws_name,
                &review.review_id,
                &format!("{project}-security"),
                "agent",
            ));
        }

        // Review pending → wait or check
        (Some(review), Some(ws_name)) => {
            guidance.step(format!(
                "# {} — review {} pending",
                bead_id, review.review_id
            ));
            guidance.step(shell::seal_show_cmd(ws_name, &review.review_id));
        }

        // No review, has workspace → continue working
        (None, Some(ws_name)) => {
            guidance.step(format!(
                "# {} — continue implementation in {}",
                bead_id, ws_name
            ));
            guidance.step(format!("maw exec default -- bn show {}", bead_id));
            guidance.step(format!(
                "# Work in ws/{ws_name}/, then request review when ready"
            ));
        }

        // No review, no workspace → needs workspace
        (None, None) => {
            guidance.step(format!("# {} — claimed but no workspace", bead_id));
            guidance.step(shell::ws_create_cmd());
            guidance.step(format!("# Stake workspace claim after creation:"));
            guidance.step(shell::claims_stake_cmd(
                "agent",
                &format!("workspace://{project}/$WS"),
                bead_id,
            ));
        }

        // Review exists but no workspace (shouldn't normally happen)
        (Some(review), None) => {
            guidance.step(format!(
                "# {} — review {} exists but workspace missing",
                bead_id, review.review_id
            ));
            guidance.diagnostic(format!(
                "Bone {} has review {} but no associated workspace claim. Check claims with: bus claims list --agent $agent --format json",
                bead_id, review.review_id
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_guidance() {
        // Build what render_fresh would produce
        let mut guidance = ProtocolGuidance::new("resume");
        guidance.status = ProtocolStatus::Fresh;
        guidance.step("maw exec default -- bn next".to_string());
        guidance.advise("No in-progress work found.".to_string());

        assert_eq!(guidance.command, "resume");
        assert_eq!(guidance.status, ProtocolStatus::Fresh);
        assert_eq!(guidance.steps.len(), 1);
        assert!(guidance.steps[0].contains("bn next"));
    }

    #[test]
    fn test_bone_resume_continue_working() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "Fix login bug".to_string(),
            state: "doing".to_string(),
            workspace: Some("frost-castle".to_string()),
            review: None,
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("continue implementation"))
        );
        assert!(guidance.steps.iter().any(|s| s.contains("frost-castle")));
        assert!(guidance.steps.iter().any(|s| s.contains("bn show bd-abc")));
    }

    #[test]
    fn test_bone_resume_review_approved() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "Add feature".to_string(),
            state: "doing".to_string(),
            workspace: Some("frost-castle".to_string()),
            review: Some(ReviewState {
                review_id: "cr-xyz".to_string(),
                gate: ReviewGateStatus::Approved,
                open_threads: 0,
            }),
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(guidance.steps.iter().any(|s| s.contains("approved")));
        assert!(guidance.steps.iter().any(|s| s.contains("protocol finish")));
    }

    #[test]
    fn test_bone_resume_review_blocked() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "Add feature".to_string(),
            state: "doing".to_string(),
            workspace: Some("frost-castle".to_string()),
            review: Some(ReviewState {
                review_id: "cr-xyz".to_string(),
                gate: ReviewGateStatus::Blocked,
                open_threads: 2,
            }),
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(guidance.steps.iter().any(|s| s.contains("blocked")));
        assert!(guidance.steps.iter().any(|s| s.contains("seal review")));
        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("seal reviews request"))
        );
    }

    #[test]
    fn test_bone_resume_review_pending() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "Add feature".to_string(),
            state: "doing".to_string(),
            workspace: Some("frost-castle".to_string()),
            review: Some(ReviewState {
                review_id: "cr-xyz".to_string(),
                gate: ReviewGateStatus::NeedsReview,
                open_threads: 0,
            }),
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(guidance.steps.iter().any(|s| s.contains("pending")));
        assert!(guidance.steps.iter().any(|s| s.contains("seal review")));
    }

    #[test]
    fn test_bone_resume_no_workspace() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "New task".to_string(),
            state: "doing".to_string(),
            workspace: None,
            review: None,
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(guidance.steps.iter().any(|s| s.contains("no workspace")));
        assert!(guidance.steps.iter().any(|s| s.contains("maw ws create")));
        assert!(guidance.steps.iter().any(|s| s.contains("claims stake")));
    }

    #[test]
    fn test_bone_resume_review_no_workspace() {
        let assessment = BoneResume {
            bone_id: "bd-abc".to_string(),
            title: "Orphaned review".to_string(),
            state: "doing".to_string(),
            workspace: None,
            review: Some(ReviewState {
                review_id: "cr-xyz".to_string(),
                gate: ReviewGateStatus::NeedsReview,
                open_threads: 0,
            }),
        };

        let mut guidance = ProtocolGuidance::new("resume");
        build_bone_guidance(&mut guidance, &assessment, "test-agent", "myproject");

        assert!(
            guidance
                .steps
                .iter()
                .any(|s| s.contains("workspace missing"))
        );
        assert!(
            guidance
                .diagnostics
                .iter()
                .any(|d| d.contains("no associated workspace"))
        );
    }

    #[test]
    fn test_multiple_bones_have_separators() {
        // Verify that render_resume adds separators for multiple bones
        let assessments = vec![
            BoneResume {
                bone_id: "bd-aaa".to_string(),
                title: "First".to_string(),
                state: "doing".to_string(),
                workspace: Some("ws1".to_string()),
                review: None,
            },
            BoneResume {
                bone_id: "bd-bbb".to_string(),
                title: "Second".to_string(),
                state: "doing".to_string(),
                workspace: Some("ws2".to_string()),
                review: None,
            },
        ];

        let mut guidance = ProtocolGuidance::new("resume");
        guidance.status = ProtocolStatus::Resumable;

        for a in &assessments {
            guidance.diagnostic(format!("--- {} ({}) ---", a.bone_id, a.title));
            build_bone_guidance(&mut guidance, a, "test-agent", "myproject");
        }

        assert!(guidance.diagnostics.iter().any(|d| d.contains("bd-aaa")));
        assert!(guidance.diagnostics.iter().any(|d| d.contains("bd-bbb")));
        assert!(guidance.steps.len() >= 4); // At least 2 steps per bone
    }
}
