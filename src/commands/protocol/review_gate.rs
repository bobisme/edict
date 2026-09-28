//! Shared review decision engine for protocol commands.
//!
//! Converts review votes + required reviewers into one canonical result
//! (approved, blocked, needs-review) with diagnostics.
//! This prevents inconsistent policy logic across finish/review/resume/status commands.

use super::adapters::{ReviewDetail, ReviewVote};
use std::collections::HashMap;

/// Result of evaluating a review against a gate policy.
#[derive(Debug, Clone)]
pub struct ReviewGateDecision {
    /// Status: "approved", "blocked", or "needs-review"
    pub status: ReviewGateStatus,
    /// Reviewers who haven't voted yet
    pub missing_approvals: Vec<String>,
    /// Reviewers who blocked after previously approving
    #[allow(dead_code)]
    pub newer_block_after_lgtm: Vec<String>,
    /// Total required reviewers
    pub total_required: usize,
    /// Total voted lgtm
    pub approved_by: Vec<String>,
    /// Total voted block
    pub blocked_by: Vec<String>,
    /// True when votes alone would have approved this review, but the commit
    /// it was approved for no longer matches the commit being merged (or
    /// that couldn't be confirmed) — `status` was downgraded to
    /// `NeedsReview` for this reason specifically, not for missing votes.
    pub stale_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewGateStatus {
    /// All required reviewers have voted lgtm, no blocks
    Approved,
    /// At least one required reviewer blocked (and it's the latest vote from them)
    Blocked,
    /// Review exists but not all required reviewers have voted
    NeedsReview,
}

/// Whether the commit a review was approved for still matches the commit
/// about to be merged.
///
/// Binds the review→commit axis of the gate (the review→bone axis alone
/// only proves *some* version of this bone's work was approved, not that
/// the version being merged is the one reviewers saw).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitFreshness {
    /// The review's last-known target commit matches the workspace HEAD
    /// being merged — the approval covers the code being merged.
    Fresh,
    /// Both commits were determined but differ: new commits landed on the
    /// workspace since the review's diff was last computed (creation or
    /// last re-request). The approval is stale.
    Stale,
    /// One or both commits could not be determined (subprocess failure,
    /// missing field). Fail closed: treated the same as `Stale` — an
    /// approval whose scope can't be confirmed does not satisfy the gate.
    #[default]
    Unknown,
}

impl ReviewGateDecision {
    /// Convert status to string for output.
    #[must_use]
    pub const fn status_str(&self) -> &'static str {
        match self.status {
            ReviewGateStatus::Approved => "approved",
            ReviewGateStatus::Blocked => "blocked",
            ReviewGateStatus::NeedsReview => "needs-review",
        }
    }
}

/// Evaluate a review against required reviewers.
///
/// Returns the canonical gate decision and diagnostics.
///
/// Logic:
/// 1. Current Seal versions may store an approved review at the review status
///    level instead of as a vote row. Treat `status=approved` with
///    `status_changed_by=<required reviewer>` as that reviewer's latest LGTM.
/// 2. If no votes and required reviewers not empty → `NeedsReview`
/// 3. For each required reviewer:
///    - Track their latest vote (by `voted_at` timestamp)
///    - If no vote → add to `missing_approvals`
/// 4. If any required reviewer's latest vote is "block" → Blocked, add to `newer_block_after_lgtm`
/// 5. If all required reviewers have voted lgtm and no blocks → Approved
/// 6. Otherwise → `NeedsReview`
///
/// Does not check whether the approval still covers the commit about to be
/// merged — callers that gate an actual merge action must additionally use
/// [`bind_to_commit`] on the result.
#[must_use]
pub fn evaluate_review_gate(
    review: &ReviewDetail,
    required_reviewers: &[String],
) -> ReviewGateDecision {
    let mut approved_by = Vec::new();
    let mut blocked_by = Vec::new();
    let mut latest_votes: HashMap<String, &ReviewVote> = HashMap::new();

    // Build a map of latest vote per reviewer
    // Track both latest vote and whether they previously LGTM'd
    let mut previous_lgtm: HashMap<String, bool> = HashMap::new();

    for vote in &review.votes {
        let reviewer_key = vote.reviewer.clone();

        // Track if this reviewer LGTM'd at some point
        if vote.is_lgtm() {
            previous_lgtm.insert(reviewer_key.clone(), true);
        }

        latest_votes
            .entry(reviewer_key)
            .and_modify(|existing| {
                // Keep the vote with the later timestamp
                // Lexicographic string comparison works correctly for ISO 8601/RFC3339 timestamps
                if let (Some(existing_voted_at), Some(new_voted_at)) =
                    (&existing.voted_at, &vote.voted_at)
                    && new_voted_at > existing_voted_at
                {
                    *existing = vote;
                }
            })
            .or_insert(vote);
    }

    let mut missing_approvals = Vec::new();
    let mut newer_block_after_lgtm = Vec::new();

    for required in required_reviewers {
        let implicit_lgtm = implicit_status_lgtm(review, required);
        match latest_votes.get(required) {
            Some(vote) => {
                if implicit_lgtm_is_latest(review, vote, implicit_lgtm) || vote.is_lgtm() {
                    approved_by.push(required.clone());
                } else if vote.is_block() {
                    blocked_by.push(required.clone());
                    // Only add to newer_block_after_lgtm if they previously LGTM'd
                    if previous_lgtm.get(required).copied().unwrap_or(false) {
                        newer_block_after_lgtm.push(required.clone());
                    }
                }
            }
            None => {
                if implicit_lgtm {
                    approved_by.push(required.clone());
                } else {
                    missing_approvals.push(required.clone());
                }
            }
        }
    }

    let status = if !missing_approvals.is_empty()
        || (required_reviewers.is_empty() && review.votes.is_empty())
    {
        ReviewGateStatus::NeedsReview
    } else if !newer_block_after_lgtm.is_empty() || !blocked_by.is_empty() {
        ReviewGateStatus::Blocked
    } else if approved_by.len() == required_reviewers.len() {
        ReviewGateStatus::Approved
    } else {
        ReviewGateStatus::NeedsReview
    };

    ReviewGateDecision {
        status,
        missing_approvals,
        newer_block_after_lgtm,
        total_required: required_reviewers.len(),
        approved_by,
        blocked_by,
        stale_approval: false,
    }
}

/// Bind a vote-based decision to the commit actually being merged.
///
/// Votes alone only prove *some* version of this work was approved — not
/// that it's the version about to land. If `decision.status` is `Approved`
/// but `commit_freshness` is not `Fresh`, downgrades to `NeedsReview` and
/// sets `stale_approval`. Leaves `Blocked` and vote-driven `NeedsReview`
/// untouched: those are already correct for their own reasons and
/// shouldn't be relabeled by an unrelated commit-freshness concern.
///
/// Only merge-gating callers need this (`protocol merge`, and `protocol
/// finish`, whose steps merge) — commands that merely report review state
/// (status/resume/review) evaluate votes on their own via
/// [`evaluate_review_gate`]. Get the freshness from [`approval_freshness`].
#[must_use]
pub fn bind_to_commit(
    mut decision: ReviewGateDecision,
    commit_freshness: CommitFreshness,
) -> ReviewGateDecision {
    if decision.status == ReviewGateStatus::Approved && commit_freshness != CommitFreshness::Fresh {
        decision.status = ReviewGateStatus::NeedsReview;
        decision.stale_approval = true;
    }
    decision
}

/// Whether an approval covers the workspace HEAD, and why edict thinks so.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FreshnessCheck {
    pub freshness: CommitFreshness,
    /// Paths that changed after the approved commit, outside the review log.
    /// Set only when edict's own cross-check overrode seal's `approval_stale:false`.
    pub uncovered_paths: Vec<String>,
    /// A diagnostic the caller surfaces (for example, seal gave no
    /// `approved_commit`, so edict could not cross-check).
    pub note: Option<String>,
}

impl FreshnessCheck {
    const fn of(freshness: CommitFreshness) -> Self {
        Self {
            freshness,
            uncovered_paths: Vec::new(),
            note: None,
        }
    }
}

/// Whether every path is inside the review log `.seal/reviews/<review_id>/`.
///
/// The review-log commit (`chore: seal review <id>`) is the one commit that is
/// allowed after the LGTM, so a HEAD that differs from the approved commit by
/// that log alone is still covered by the approval.
#[must_use]
pub fn only_review_log_changed(paths: &[String], review_id: &str) -> bool {
    let prefix = format!(".seal/reviews/{review_id}/");
    paths.iter().all(|p| p.starts_with(&prefix))
}

/// Decide whether a review's approval covers the workspace HEAD.
///
/// `summary` is `seal diff <id> --format json` (`None` when it failed),
/// `workspace_head` is `git rev-parse HEAD` in the workspace, and
/// `changed_paths(approved_commit, head)` lists the paths changed between the
/// two (`None` when that lookup failed). It is only called when the two
/// commits differ.
///
/// Rules:
/// - seal says `approval_stale: true` → stale.
/// - seal says `approval_stale: false` → do not trust it alone (seal 0.30 misses
///   post-LGTM commits on a detached HEAD, seal bn-2ypz). If `approved_commit`
///   equals HEAD → fresh. If it differs and anything outside
///   `.seal/reviews/<id>/` changed → stale. If only the review log changed →
///   fresh. If HEAD or the changed paths are unknown → unknown (fails closed).
///   If seal gives no `approved_commit` → fresh as before, with a diagnostic.
/// - older seal (no `approval_stale`) → compare `target_commit` with HEAD.
pub fn approval_freshness(
    summary: Option<&super::adapters::ReviewDiffSummary>,
    workspace_head: Option<&str>,
    review_id: &str,
    changed_paths: impl FnOnce(&str, &str) -> Option<Vec<String>>,
) -> FreshnessCheck {
    let Some(summary) = summary else {
        return FreshnessCheck::of(CommitFreshness::Unknown);
    };
    match summary.approval_stale {
        Some(true) => FreshnessCheck::of(CommitFreshness::Stale),
        Some(false) => {
            let Some(approved) = summary.approved_commit.as_deref() else {
                let mut check = FreshnessCheck::of(CommitFreshness::Fresh);
                check.note = Some(format!(
                    "seal diff {review_id} reported approval_stale:false without an \
                     approved_commit, so edict could not confirm that the approval covers \
                     the workspace HEAD. Trusting seal."
                ));
                return check;
            };
            let Some(head) = workspace_head else {
                return FreshnessCheck::of(CommitFreshness::Unknown);
            };
            if approved == head {
                return FreshnessCheck::of(CommitFreshness::Fresh);
            }
            match changed_paths(approved, head) {
                None => FreshnessCheck::of(CommitFreshness::Unknown),
                Some(paths) if only_review_log_changed(&paths, review_id) => {
                    FreshnessCheck::of(CommitFreshness::Fresh)
                }
                Some(paths) => {
                    let uncovered_paths = paths
                        .into_iter()
                        .filter(|p| !only_review_log_changed(std::slice::from_ref(p), review_id))
                        .collect();
                    FreshnessCheck {
                        freshness: CommitFreshness::Stale,
                        uncovered_paths,
                        note: None,
                    }
                }
            }
        }
        None => FreshnessCheck::of(match (summary.target_commit.as_deref(), workspace_head) {
            (Some(target), Some(head)) if target == head => CommitFreshness::Fresh,
            (Some(_), Some(_)) => CommitFreshness::Stale,
            _ => CommitFreshness::Unknown,
        }),
    }
}

fn implicit_status_lgtm(review: &ReviewDetail, reviewer: &str) -> bool {
    review.status == "approved" && review.status_changed_by.as_deref() == Some(reviewer)
}

fn implicit_lgtm_is_latest(review: &ReviewDetail, vote: &ReviewVote, implicit_lgtm: bool) -> bool {
    if !implicit_lgtm {
        return false;
    }

    match (&review.status_changed_at, &vote.voted_at) {
        (Some(status_changed_at), Some(voted_at)) => status_changed_at >= voted_at,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::protocol::adapters::ReviewDiffSummary;

    // --- approval_freshness (bn-2cyu) ---

    const APPROVED: &str = "c658bbf939ffae9f5594c7b69b880c209e57b721";
    const HEAD: &str = "cb3bbc3445b4a88fabc99f24bbbef77557137900";

    /// What seal 0.30 reports after a post-LGTM commit on a detached HEAD:
    /// `approval_stale:false`, `uncovered_commits:0`, and `approved_commit` /
    /// `target_commit` still at the LGTM'd commit.
    fn seal_030(approved: Option<&str>) -> ReviewDiffSummary {
        ReviewDiffSummary {
            target_commit: Some(APPROVED.into()),
            approval_stale: Some(false),
            approved_commit: approved.map(Into::into),
            uncovered_commits: Some(0),
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "stands in for the fallible changed-paths lookup"
    )]
    fn paths(p: &[&str]) -> Option<Vec<String>> {
        Some(p.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn post_lgtm_code_commit_is_stale_even_when_seal_says_fresh() {
        let check = approval_freshness(
            Some(&seal_030(Some(APPROVED))),
            Some(HEAD),
            "cr-1",
            |a, h| {
                assert_eq!((a, h), (APPROVED, HEAD));
                paths(&["src/main.rs", ".seal/reviews/cr-1/events.jsonl"])
            },
        );
        assert_eq!(check.freshness, CommitFreshness::Stale);
        assert_eq!(check.uncovered_paths, vec!["src/main.rs".to_string()]);
    }

    #[test]
    fn review_log_commit_alone_is_not_stale() {
        let check = approval_freshness(
            Some(&seal_030(Some(APPROVED))),
            Some(HEAD),
            "cr-1",
            |_, _| paths(&[".seal/reviews/cr-1/events.jsonl"]),
        );
        assert_eq!(check.freshness, CommitFreshness::Fresh);
        assert!(check.note.is_none());
    }

    #[test]
    fn another_reviews_log_is_not_the_allowed_commit() {
        let check = approval_freshness(
            Some(&seal_030(Some(APPROVED))),
            Some(HEAD),
            "cr-1",
            |_, _| paths(&[".seal/reviews/cr-10/events.jsonl"]),
        );
        assert_eq!(check.freshness, CommitFreshness::Stale);
    }

    #[test]
    fn approved_commit_equal_to_head_is_fresh_without_a_diff() {
        let check = approval_freshness(
            Some(&seal_030(Some(APPROVED))),
            Some(APPROVED),
            "cr-1",
            |_, _| panic!("no diff needed when the commits match"),
        );
        assert_eq!(check.freshness, CommitFreshness::Fresh);
    }

    #[test]
    fn missing_approved_commit_trusts_seal_with_a_diagnostic() {
        let check = approval_freshness(Some(&seal_030(None)), Some(HEAD), "cr-1", |_, _| {
            panic!("no approved commit to diff from")
        });
        assert_eq!(check.freshness, CommitFreshness::Fresh);
        assert!(
            check
                .note
                .as_deref()
                .is_some_and(|n| n.contains("approved_commit"))
        );
    }

    #[test]
    fn unknown_head_or_failed_diff_fails_closed() {
        let s = seal_030(Some(APPROVED));
        assert_eq!(
            approval_freshness(Some(&s), None, "cr-1", |_, _| None).freshness,
            CommitFreshness::Unknown
        );
        assert_eq!(
            approval_freshness(Some(&s), Some(HEAD), "cr-1", |_, _| None).freshness,
            CommitFreshness::Unknown
        );
    }

    #[test]
    fn seal_stale_wins_and_older_seal_compares_target() {
        let mut s = seal_030(Some(APPROVED));
        s.approval_stale = Some(true);
        assert_eq!(
            approval_freshness(Some(&s), Some(APPROVED), "cr-1", |_, _| None).freshness,
            CommitFreshness::Stale
        );
        s.approval_stale = None;
        assert_eq!(
            approval_freshness(Some(&s), Some(APPROVED), "cr-1", |_, _| None).freshness,
            CommitFreshness::Fresh
        );
        assert_eq!(
            approval_freshness(Some(&s), Some(HEAD), "cr-1", |_, _| None).freshness,
            CommitFreshness::Stale
        );
        assert_eq!(
            approval_freshness(None, Some(HEAD), "cr-1", |_, _| None).freshness,
            CommitFreshness::Unknown
        );
    }

    fn make_vote(reviewer: &str, vote: &str, voted_at: &str) -> ReviewVote {
        ReviewVote {
            reviewer: reviewer.to_string(),
            vote: vote.to_string(),
            voted_at: Some(voted_at.to_string()),
        }
    }

    fn make_review(votes: Vec<ReviewVote>) -> ReviewDetail {
        ReviewDetail {
            review_id: "cr-test".into(),
            title: None,
            status: "open".into(),
            status_changed_at: None,
            status_changed_by: None,
            change_id: None,
            votes,
            open_thread_count: 0,
        }
    }

    #[test]
    fn test_approved_all_reviewers_lgtm() {
        let review = make_review(vec![
            make_vote("edict-security", "lgtm", "2026-02-16T10:00:00Z"),
            make_vote("edict-perf", "lgtm", "2026-02-16T10:05:00Z"),
        ]);
        let required = vec!["edict-security".to_string(), "edict-perf".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert!(decision.missing_approvals.is_empty());
        assert!(decision.newer_block_after_lgtm.is_empty());
        assert_eq!(decision.approved_by.len(), 2);
        assert_eq!(decision.blocked_by.len(), 0);
    }

    #[test]
    fn test_blocked_one_reviewer_blocks() {
        let review = make_review(vec![
            make_vote("edict-security", "lgtm", "2026-02-16T10:00:00Z"),
            make_vote("edict-perf", "block", "2026-02-16T10:05:00Z"),
        ]);
        let required = vec!["edict-security".to_string(), "edict-perf".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Blocked);
        assert!(decision.missing_approvals.is_empty());
        // edict-perf only has a block vote (no prior LGTM), so it should NOT be in newer_block_after_lgtm
        assert!(decision.newer_block_after_lgtm.is_empty());
        assert_eq!(decision.blocked_by, vec!["edict-perf"]);
    }

    #[test]
    fn test_needs_review_missing_approvals() {
        let review = make_review(vec![make_vote(
            "edict-security",
            "lgtm",
            "2026-02-16T10:00:00Z",
        )]);
        let required = vec!["edict-security".to_string(), "edict-perf".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert_eq!(decision.missing_approvals, vec!["edict-perf"]);
        assert_eq!(decision.approved_by.len(), 1);
    }

    #[test]
    fn test_needs_review_no_votes() {
        let review = make_review(vec![]);
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert_eq!(decision.missing_approvals, vec!["edict-security"]);
    }

    #[test]
    fn test_approved_empty_required_reviewers() {
        let review = make_review(vec![]);
        let required = vec![];

        let decision = evaluate_review_gate(&review, &required);

        // Empty required list with no votes should be NeedsReview (or Approved?).
        // Current logic: NeedsReview if no required reviewers and no votes.
        // Actually, if there are no required reviewers, the review is approved by default.
        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert_eq!(decision.total_required, 0);
    }

    #[test]
    fn test_blocked_then_lgtm_latest_is_lgtm() {
        let review = make_review(vec![
            make_vote("edict-security", "block", "2026-02-16T10:00:00Z"),
            make_vote("edict-security", "lgtm", "2026-02-16T10:05:00Z"),
        ]);
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        // Latest vote is lgtm, so should be approved
        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert!(decision.newer_block_after_lgtm.is_empty());
    }

    #[test]
    fn test_lgtm_then_block_latest_is_block() {
        let review = make_review(vec![
            make_vote("edict-security", "lgtm", "2026-02-16T10:00:00Z"),
            make_vote("edict-security", "block", "2026-02-16T10:05:00Z"),
        ]);
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        // Latest vote is block, so should be blocked
        assert_eq!(decision.status, ReviewGateStatus::Blocked);
        assert_eq!(decision.blocked_by, vec!["edict-security"]);
        assert_eq!(decision.newer_block_after_lgtm, vec!["edict-security"]);
    }

    #[test]
    fn test_multiple_reviewers_mixed() {
        let review = make_review(vec![
            make_vote("edict-security", "lgtm", "2026-02-16T10:00:00Z"),
            make_vote("edict-perf", "lgtm", "2026-02-16T10:05:00Z"),
            make_vote("edict-other", "block", "2026-02-16T10:10:00Z"),
        ]);
        let required = vec![
            "edict-security".to_string(),
            "edict-perf".to_string(),
            "edict-other".to_string(),
        ];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Blocked);
        assert_eq!(decision.approved_by.len(), 2);
        assert_eq!(decision.blocked_by.len(), 1);
    }

    #[test]
    fn test_reviewer_not_in_required_list_ignored() {
        let review = make_review(vec![
            make_vote("edict-security", "lgtm", "2026-02-16T10:00:00Z"),
            make_vote("random-reviewer", "block", "2026-02-16T10:05:00Z"),
        ]);
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        // random-reviewer's block should be ignored
        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert_eq!(decision.blocked_by.len(), 0);
    }

    #[test]
    fn test_status_approved_by_required_reviewer_counts_as_lgtm() {
        let mut review = make_review(vec![]);
        review.status = "approved".into();
        review.status_changed_by = Some("edict-security".into());
        review.status_changed_at = Some("2026-02-16T10:05:00Z".into());
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert!(decision.missing_approvals.is_empty());
    }

    #[test]
    fn test_status_approved_by_one_required_reviewer_does_not_cover_others() {
        let mut review = make_review(vec![]);
        review.status = "approved".into();
        review.status_changed_by = Some("edict-security".into());
        review.status_changed_at = Some("2026-02-16T10:05:00Z".into());
        let required = vec!["edict-security".to_string(), "edict-perf".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert_eq!(decision.missing_approvals, vec!["edict-perf"]);
    }

    #[test]
    fn test_status_approved_after_block_overrides_same_reviewer_block() {
        let mut review = make_review(vec![make_vote(
            "edict-security",
            "block",
            "2026-02-16T10:00:00Z",
        )]);
        review.status = "approved".into();
        review.status_changed_by = Some("edict-security".into());
        review.status_changed_at = Some("2026-02-16T10:05:00Z".into());
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert!(decision.blocked_by.is_empty());
    }

    #[test]
    fn test_status_approved_before_later_block_does_not_override_block() {
        let mut review = make_review(vec![make_vote(
            "edict-security",
            "block",
            "2026-02-16T10:10:00Z",
        )]);
        review.status = "approved".into();
        review.status_changed_by = Some("edict-security".into());
        review.status_changed_at = Some("2026-02-16T10:05:00Z".into());
        let required = vec!["edict-security".to_string()];

        let decision = evaluate_review_gate(&review, &required);

        assert_eq!(decision.status, ReviewGateStatus::Blocked);
        assert_eq!(decision.blocked_by, vec!["edict-security"]);
    }

    // --- Commit freshness (review→commit binding) ---

    #[test]
    fn stale_commit_downgrades_approval_to_needs_review() {
        let review = make_review(vec![make_vote(
            "edict-security",
            "lgtm",
            "2026-02-16T10:00:00Z",
        )]);
        let required = vec!["edict-security".to_string()];

        let decision = bind_to_commit(
            evaluate_review_gate(&review, &required),
            CommitFreshness::Stale,
        );

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert!(decision.stale_approval);
        // Votes are untouched — the reviewer really did approve; it's the
        // commit binding that's wrong, not who voted.
        assert_eq!(decision.approved_by, vec!["edict-security"]);
        assert!(decision.missing_approvals.is_empty());
    }

    #[test]
    fn unknown_commit_fails_closed_like_stale() {
        let review = make_review(vec![make_vote(
            "edict-security",
            "lgtm",
            "2026-02-16T10:00:00Z",
        )]);
        let required = vec!["edict-security".to_string()];

        let decision = bind_to_commit(
            evaluate_review_gate(&review, &required),
            CommitFreshness::Unknown,
        );

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert!(decision.stale_approval);
    }

    #[test]
    fn fresh_commit_does_not_disturb_approval() {
        let review = make_review(vec![make_vote(
            "edict-security",
            "lgtm",
            "2026-02-16T10:00:00Z",
        )]);
        let required = vec!["edict-security".to_string()];

        let decision = bind_to_commit(
            evaluate_review_gate(&review, &required),
            CommitFreshness::Fresh,
        );

        assert_eq!(decision.status, ReviewGateStatus::Approved);
        assert!(!decision.stale_approval);
    }

    #[test]
    fn stale_commit_does_not_relabel_an_already_blocked_review() {
        // A genuine block should read as "blocked by a reviewer", not get
        // relabeled by an unrelated commit-freshness concern.
        let review = make_review(vec![make_vote(
            "edict-security",
            "block",
            "2026-02-16T10:00:00Z",
        )]);
        let required = vec!["edict-security".to_string()];

        let decision = bind_to_commit(
            evaluate_review_gate(&review, &required),
            CommitFreshness::Stale,
        );

        assert_eq!(decision.status, ReviewGateStatus::Blocked);
        assert!(!decision.stale_approval);
    }

    #[test]
    fn stale_commit_does_not_relabel_missing_votes() {
        let review = make_review(vec![]);
        let required = vec!["edict-security".to_string()];

        let decision = bind_to_commit(
            evaluate_review_gate(&review, &required),
            CommitFreshness::Stale,
        );

        assert_eq!(decision.status, ReviewGateStatus::NeedsReview);
        assert!(!decision.stale_approval);
        assert_eq!(decision.missing_approvals, vec!["edict-security"]);
    }
}
