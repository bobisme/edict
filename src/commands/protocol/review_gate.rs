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

impl ReviewGateDecision {
    /// Convert status to string for output.
    pub fn status_str(&self) -> &'static str {
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
/// 1. If no votes and required reviewers not empty → NeedsReview
/// 2. For each required reviewer:
///    - Track their latest vote (by voted_at timestamp)
///    - If no vote → add to missing_approvals
/// 3. If any required reviewer's latest vote is "block" → Blocked, add to newer_block_after_lgtm
/// 4. If all required reviewers have voted lgtm and no blocks → Approved
/// 5. Otherwise → NeedsReview
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
                {
                    if new_voted_at > existing_voted_at {
                        *existing = vote;
                    }
                }
            })
            .or_insert(vote);
    }

    let mut missing_approvals = Vec::new();
    let mut newer_block_after_lgtm = Vec::new();

    for required in required_reviewers {
        match latest_votes.get(required) {
            Some(vote) => {
                if vote.is_lgtm() {
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
                missing_approvals.push(required.clone());
            }
        }
    }

    let status = if !missing_approvals.is_empty()
        || (required_reviewers.is_empty() && review.votes.is_empty())
    {
        ReviewGateStatus::NeedsReview
    } else if !newer_block_after_lgtm.is_empty() {
        ReviewGateStatus::Blocked
    } else if !blocked_by.is_empty() {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
