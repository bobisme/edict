//! `ProtocolContext`: cross-tool shared state collector.
//!
//! Gathers rite claims, maw workspaces, and bone/review status in a single
//! structure to avoid duplicating subprocess calls across protocol commands.
//! Lazy evaluation: state is fetched on-demand via subprocess calls, not upfront.

use std::process::Command;

use super::adapters::{self, BoneInfo, Claim, ReviewDetail, ReviewDetailResponse, Workspace};
use super::review_select;

/// Cross-tool state collector for protocol commands.
///
/// Provides cached access to rite claims and maw workspaces (fetched on construction),
/// plus lazy on-demand methods for bone/review status.
#[derive(Debug, Clone)]
pub struct ProtocolContext {
    #[allow(dead_code)]
    project: String,
    agent: String,
    claims: Vec<Claim>,
    workspaces: Vec<Workspace>,
}

#[cfg(test)]
impl ProtocolContext {
    /// Build a context from fixed data, with no subprocess calls.
    ///
    /// Lets sibling modules test claim-dependent logic (which bone gates a merge)
    /// against hand-built claim sets, including hostile ones.
    pub(super) fn for_test(agent: &str, claims: Vec<Claim>, workspaces: Vec<Workspace>) -> Self {
        Self {
            project: "edict".to_string(),
            agent: agent.to_string(),
            claims,
            workspaces,
        }
    }
}

impl ProtocolContext {
    /// Collect shared state from rite and maw.
    ///
    /// Calls:
    /// - `rite claims list --format json --agent <agent>`
    /// - `maw ws list --format json`
    ///
    /// # Errors
    ///
    /// Returns error if either subprocess fails or output is unparseable.
    pub fn collect(project: &str, agent: &str) -> Result<Self, ContextError> {
        // Fetch rite claims
        let claims_output = Self::run_subprocess(&[
            "rite", "claims", "list", "--agent", agent, "--format", "json",
        ])?;
        let claims_resp = adapters::parse_claims(&claims_output)
            .map_err(|e| ContextError::ParseFailed(format!("claims: {e}")))?;

        // Fetch maw workspaces
        let workspaces_output = Self::run_subprocess(&["maw", "ws", "list", "--format", "json"])?;
        let workspaces_resp = adapters::parse_workspaces(&workspaces_output)
            .map_err(|e| ContextError::ParseFailed(format!("workspaces: {e}")))?;

        Ok(Self {
            project: project.to_string(),
            agent: agent.to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        })
    }

    /// Get all held bone claims as (`bone_id`, pattern) tuples.
    #[must_use]
    pub fn held_bone_claims(&self) -> Vec<(&str, &str)> {
        let mut result = Vec::new();
        for claim in &self.claims {
            if claim.agent == self.agent {
                for pattern in &claim.patterns {
                    if let Some(bone_id) = pattern
                        .strip_prefix("bone://")
                        .and_then(|rest| rest.split('/').nth(1))
                    {
                        result.push((bone_id, pattern.as_str()));
                    }
                }
            }
        }
        result
    }

    /// Get all held workspace claims as (`workspace_name`, pattern) tuples.
    #[must_use]
    pub fn held_workspace_claims(&self) -> Vec<(&str, &str)> {
        let mut result = Vec::new();
        for claim in &self.claims {
            if claim.agent == self.agent {
                for pattern in &claim.patterns {
                    if let Some(ws_name) = pattern
                        .strip_prefix("workspace://")
                        .and_then(|rest| rest.split('/').nth(1))
                    {
                        result.push((ws_name, pattern.as_str()));
                    }
                }
            }
        }
        result
    }

    /// Find a workspace by name.
    #[allow(dead_code)]
    #[must_use]
    pub fn find_workspace(&self, name: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|ws| ws.name == name)
    }

    /// Correlate a bone claim with its workspace claim.
    ///
    /// Tries memo-based correlation first (most precise), then falls back to
    /// finding any non-default workspace claim from this agent.
    ///
    /// The fallback carries the traffic today because the memo path never matches:
    /// `rite claims list --format json` emits the field as `message`, and
    /// [`adapters::Claim`] deserializes `memo` with no alias for it, so `memo` is
    /// always `None`. Do NOT "fix" that by adding the alias alone — the memo is
    /// free text chosen by whoever staked the claim, and
    /// `merge::find_bone_for_workspace` uses the same memo to decide which bone's
    /// review gates a merge. Arming it without that function's agent + held-bone-claim
    /// corroboration would let a claim memo nominate someone else's approved bone.
    #[must_use]
    pub fn workspace_for_bone(&self, bone_id: &str) -> Option<&str> {
        // First pass: memo-based correlation (precise, works when rite includes memo)
        for claim in &self.claims {
            if claim.agent == self.agent
                && let Some(memo) = &claim.memo
                && memo == bone_id
            {
                for pattern in &claim.patterns {
                    if let Some(ws_name) = pattern
                        .strip_prefix("workspace://")
                        .and_then(|rest| rest.split('/').nth(1))
                    {
                        return Some(ws_name);
                    }
                }
            }
        }

        // Fallback: find any non-default workspace claim from this agent.
        // Workers hold one bone + one workspace, so this is unambiguous.
        for claim in &self.claims {
            if claim.agent == self.agent {
                for pattern in &claim.patterns {
                    if let Some(ws_name) = pattern
                        .strip_prefix("workspace://")
                        .and_then(|rest| rest.split('/').nth(1))
                        && ws_name != "default"
                    {
                        return Some(ws_name);
                    }
                }
            }
        }
        None
    }

    /// Fetch bone status by calling `maw exec default -- bn show <id> --format json`.
    ///
    /// # Errors
    ///
    /// Returns error if the bone ID is invalid, the subprocess fails, or the output is unparseable.
    #[allow(
        clippy::unused_self,
        reason = "part of the ProtocolContext query interface, symmetric with its stateful methods"
    )]
    pub fn bone_status(&self, bone_id: &str) -> Result<BoneInfo, ContextError> {
        Self::validate_bone_id(bone_id)?;
        let output = Self::run_subprocess(&[
            "maw", "exec", "default", "--", "bn", "show", bone_id, "--format", "json",
        ])?;
        let bone = adapters::parse_bone_show(&output)
            .map_err(|e| ContextError::ParseFailed(format!("bone {bone_id}: {e}")))?;
        Ok(bone)
    }

    /// List the live reviews that gate `bone_id`, newest-relevant first.
    ///
    /// `seal reviews list` is repo-global, so the raw listing includes merged
    /// reviews for unrelated bones. Callers gating on review state must use this
    /// rather than [`Self::reviews_in_workspace`].
    ///
    /// # Errors
    ///
    /// Returns error if the workspace name is invalid or the reviews output is unparseable.
    pub fn reviews_for_bone(
        &self,
        workspace: &str,
        bone_id: &str,
    ) -> Result<Vec<adapters::ReviewSummary>, ContextError> {
        let reviews = self.reviews_in_workspace(workspace)?;
        Ok(review_select::live_reviews_for_bone(&reviews, bone_id)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Find the review that gates `bone_id`, with full detail.
    ///
    /// Returns `None` when the bone has no live review — callers treat that as
    /// "review still needed", so an unrelated or merged review can never satisfy
    /// the gate.
    ///
    /// The status is checked TWICE, against two different snapshots: once on the
    /// summary from `seal reviews list`, and again on the detail from `seal review
    /// <id>`. They are separate subprocess calls, so a review that merges between
    /// them (or any list/detail disagreement in seal) yields `summary=open` with
    /// `detail=merged` — and it is the detail status that `evaluate_review_gate`
    /// and `implicit_status_lgtm` go on to read. Re-checking here covers every
    /// caller from the one place they all funnel through, rather than trusting
    /// each of the five to remember.
    #[must_use]
    pub fn find_review_for_bone(
        &self,
        workspace: &str,
        bone_id: &str,
    ) -> Option<(String, ReviewDetail)> {
        let reviews = self.reviews_in_workspace(workspace).ok()?;
        let summary = review_select::select_for_bone(&reviews, bone_id)?;
        let detail = self.review_status(&summary.review_id, workspace).ok()?;

        if !review_select::is_live_status(&detail.status) {
            return None;
        }

        Some((summary.review_id.clone(), detail))
    }

    /// List all reviews visible from a workspace (`maw exec <ws> -- seal reviews list`).
    ///
    /// This listing is repo-global and NOT scoped to the workspace or its bone.
    /// Use [`Self::reviews_for_bone`] for anything that gates on review state.
    ///
    /// Returns empty list if no reviews exist or seal is not configured.
    ///
    /// # Errors
    ///
    /// Returns error if the workspace name is invalid or the reviews output is unparseable.
    #[allow(
        clippy::unused_self,
        reason = "part of the ProtocolContext query interface, symmetric with its stateful methods"
    )]
    pub fn reviews_in_workspace(
        &self,
        workspace: &str,
    ) -> Result<Vec<adapters::ReviewSummary>, ContextError> {
        Self::validate_workspace_name(workspace)?;
        let output = Self::run_subprocess(&[
            "maw", "exec", workspace, "--", "seal", "reviews", "list", "--format", "json",
        ]);
        match output {
            Ok(json) => {
                let resp = adapters::parse_reviews_list(&json).map_err(|e| {
                    ContextError::ParseFailed(format!("reviews list in {workspace}: {e}"))
                })?;
                Ok(resp.reviews)
            }
            Err(_) => {
                // seal may not be configured or workspace may not have reviews
                Ok(Vec::new())
            }
        }
    }

    /// Fetch review status by calling `maw exec <ws> -- seal review <id> --format json`.
    ///
    /// # Errors
    ///
    /// Returns error if the review ID or workspace name is invalid, the subprocess fails, or the output is unparseable.
    #[allow(
        clippy::unused_self,
        reason = "part of the ProtocolContext query interface, symmetric with its stateful methods"
    )]
    pub fn review_status(
        &self,
        review_id: &str,
        workspace: &str,
    ) -> Result<ReviewDetail, ContextError> {
        Self::validate_review_id(review_id)?;
        Self::validate_workspace_name(workspace)?;
        let output = Self::run_subprocess(&[
            "maw", "exec", workspace, "--", "seal", "review", review_id, "--format", "json",
        ])?;
        let review_resp: ReviewDetailResponse = serde_json::from_str(&output)
            .map_err(|e| ContextError::ParseFailed(format!("review {review_id}: {e}")))?;
        Ok(review_resp.review)
    }

    /// Check for claim conflicts by querying all claims.
    ///
    /// Returns the conflicting claim if another agent holds the bone.
    ///
    /// # Errors
    ///
    /// Returns error if the subprocess fails or the claims output is unparseable.
    pub fn check_bone_claim_conflict(&self, bone_id: &str) -> Result<Option<String>, ContextError> {
        let output = Self::run_subprocess(&["rite", "claims", "list", "--format", "json"])?;
        let claims_resp = adapters::parse_claims(&output)
            .map_err(|e| ContextError::ParseFailed(format!("all claims: {e}")))?;

        for claim in &claims_resp.claims {
            if claim.agent != self.agent {
                for pattern in &claim.patterns {
                    if let Some(id) = pattern
                        .strip_prefix("bone://")
                        .and_then(|rest| rest.split('/').nth(1))
                        && id == bone_id
                    {
                        return Ok(Some(claim.agent.clone()));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Validate that a bone ID is safe for subprocess use.
    ///
    /// Bone ID prefixes vary by project (e.g., `bd-`, `bn-`, `bm-`).
    /// We validate the format (short alphanumeric with hyphens) without
    /// hardcoding a specific prefix.
    fn validate_bone_id(id: &str) -> Result<(), ContextError> {
        if !id.is_empty()
            && id.len() <= 20
            && id.contains('-')
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            Ok(())
        } else {
            Err(ContextError::ParseFailed(format!("invalid bone ID: {id}")))
        }
    }

    /// Validate that a workspace name is safe (alphanumeric + hyphens only).
    fn validate_workspace_name(name: &str) -> Result<(), ContextError> {
        if !name.is_empty()
            && name.len() <= 64
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            Ok(())
        } else {
            Err(ContextError::ParseFailed(format!(
                "invalid workspace name: {name}"
            )))
        }
    }

    /// Validate that a review ID matches the expected pattern (cr-xxxx).
    fn validate_review_id(id: &str) -> Result<(), ContextError> {
        if id.starts_with("cr-")
            && id.len() <= 20
            && id[3..].chars().all(|c| c.is_ascii_alphanumeric())
        {
            Ok(())
        } else {
            Err(ContextError::ParseFailed(format!(
                "invalid review ID: {id}"
            )))
        }
    }

    /// Run a subprocess and capture stdout.
    fn run_subprocess(args: &[&str]) -> Result<String, ContextError> {
        let mut cmd = Command::new(args[0]);
        for arg in &args[1..] {
            cmd.arg(arg);
        }

        let output = cmd
            .output()
            .map_err(|e| ContextError::SubprocessFailed(format!("{}: {e}", args[0])))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ContextError::SubprocessFailed(format!(
                "{} exited with status {}: {}",
                args[0],
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        String::from_utf8(output.stdout).map_err(|e| {
            ContextError::SubprocessFailed(format!("invalid UTF-8 from {}: {e}", args[0]))
        })
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.agent
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }
}

/// Errors during context collection and state queries.
#[derive(Debug, Clone)]
pub enum ContextError {
    /// Subprocess execution failed (command not found, permission denied, etc.)
    SubprocessFailed(String),
    /// Output parsing failed (invalid JSON, missing fields, etc.)
    ParseFailed(String),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubprocessFailed(msg) => write!(f, "subprocess failed: {msg}"),
            Self::ParseFailed(msg) => write!(f, "parse failed: {msg}"),
        }
    }
}

impl std::error::Error for ContextError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock responses for testing without subprocess calls.
    // Bus creates separate claims per stake call (no memo in JSON output).
    const CLAIMS_JSON: &str = r#"{"claims": [
        {"agent": "crimson-storm", "patterns": ["bone://edict/bd-3cqv"], "active": true},
        {"agent": "crimson-storm", "patterns": ["workspace://edict/frost-forest"], "active": true},
        {"agent": "green-vertex", "patterns": ["bone://edict/bd-3t1d"], "active": true}
    ]}"#;

    const WORKSPACES_JSON: &str = r#"{"workspaces": [
        {"name": "default", "is_default": true, "is_current": false, "change_id": "abc123"},
        {"name": "frost-forest", "is_default": false, "is_current": true, "change_id": "def456"}
    ], "advice": []}"#;

    #[test]
    fn test_held_bone_claims() {
        let claims_resp = adapters::parse_claims(CLAIMS_JSON).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "crimson-storm".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let bead_claims = ctx.held_bone_claims();
        assert_eq!(bead_claims.len(), 1);
        assert_eq!(bead_claims[0].0, "bd-3cqv");
    }

    #[test]
    fn test_held_workspace_claims() {
        let claims_resp = adapters::parse_claims(CLAIMS_JSON).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "crimson-storm".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let ws_claims = ctx.held_workspace_claims();
        assert_eq!(ws_claims.len(), 1);
        assert_eq!(ws_claims[0].0, "frost-forest");
    }

    #[test]
    fn test_find_workspace() {
        let claims_resp = adapters::parse_claims(CLAIMS_JSON).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "crimson-storm".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let ws = ctx.find_workspace("frost-forest");
        assert!(ws.is_some());
        assert_eq!(ws.unwrap().name, "frost-forest");
        assert!(!ws.unwrap().is_default);
    }

    #[test]
    fn test_workspace_for_bone() {
        let claims_resp = adapters::parse_claims(CLAIMS_JSON).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "crimson-storm".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let ws = ctx.workspace_for_bone("bd-3cqv");
        assert_eq!(ws, Some("frost-forest"));
    }

    #[test]
    fn test_workspace_for_bone_fallback_no_memo() {
        // When rite omits memo from JSON, fallback finds workspace by agent match
        let json = r#"{"claims": [
            {"agent": "dev-agent", "patterns": ["bone://proj/bd-abc"], "active": true},
            {"agent": "dev-agent", "patterns": ["workspace://proj/ember-tower"], "active": true}
        ]}"#;
        let claims_resp = adapters::parse_claims(json).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "proj".to_string(),
            agent: "dev-agent".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let ws = ctx.workspace_for_bone("bd-abc");
        assert_eq!(ws, Some("ember-tower"));
    }

    #[test]
    fn test_workspace_for_bone_skips_default() {
        // Fallback must not return "default" workspace
        let json = r#"{"claims": [
            {"agent": "dev-agent", "patterns": ["bone://proj/bd-abc"], "active": true},
            {"agent": "dev-agent", "patterns": ["workspace://proj/default"], "active": true}
        ]}"#;
        let claims_resp = adapters::parse_claims(json).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "proj".to_string(),
            agent: "dev-agent".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let ws = ctx.workspace_for_bone("bd-abc");
        assert_eq!(ws, None); // default is filtered out
    }

    #[test]
    fn test_held_bone_claims_other_agent() {
        let claims_resp = adapters::parse_claims(CLAIMS_JSON).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        // Using green-vertex context
        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "green-vertex".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        let bead_claims = ctx.held_bone_claims();
        assert_eq!(bead_claims.len(), 1);
        assert_eq!(bead_claims[0].0, "bd-3t1d");
    }

    #[test]
    fn test_empty_claims() {
        let empty = r#"{"claims": []}"#;
        let claims_resp = adapters::parse_claims(empty).unwrap();
        let workspaces_resp = adapters::parse_workspaces(WORKSPACES_JSON).unwrap();

        let ctx = ProtocolContext {
            project: "edict".to_string(),
            agent: "crimson-storm".to_string(),
            claims: claims_resp.claims,
            workspaces: workspaces_resp.workspaces,
        };

        assert!(ctx.held_bone_claims().is_empty());
        assert!(ctx.held_workspace_claims().is_empty());
    }
}
