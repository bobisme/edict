//! JSON adapters for companion tool output.
//!
//! Tolerant parsing for bus claims, maw workspaces, bn show, and seal review.
//! Each adapter handles optional/new fields gracefully and produces clear
//! parse errors. ProtocolContext consumes these instead of ad-hoc parsing.

use serde::Deserialize;

// --- Bus Claims ---

/// Parsed output from `bus claims list --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaimsResponse {
    #[serde(default)]
    pub claims: Vec<Claim>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claim {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub memo: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

impl Claim {
    /// Extract bone IDs from `bone://project/bd-xxx` patterns.
    pub fn bone_ids(&self) -> Vec<&str> {
        self.patterns
            .iter()
            .filter_map(|p| {
                p.strip_prefix("bone://")
                    .and_then(|rest| rest.split('/').nth(1))
            })
            .collect()
    }

    /// Extract workspace names from `workspace://project/ws-name` patterns.
    pub fn workspace_names(&self) -> Vec<&str> {
        self.patterns
            .iter()
            .filter_map(|p| {
                p.strip_prefix("workspace://")
                    .and_then(|rest| rest.split('/').nth(1))
            })
            .collect()
    }
}

// --- Maw Workspaces ---

/// Parsed output from `maw ws list --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspacesResponse {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub advice: Vec<WorkspaceAdvice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub name: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub change_id: Option<String>,
    #[serde(default)]
    pub commit_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceAdvice {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

// --- Bones (bn show) ---

/// Parsed output from `bn show <id> --format json`.
///
/// bn show returns a single JSON object.
#[derive(Debug, Clone, Deserialize)]
pub struct BoneInfo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(rename = "kind", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub urgency: Option<String>,
}

/// Parse `bn show --format json` output. Returns the bone info.
pub fn parse_bone_show(json: &str) -> Result<BoneInfo, AdapterError> {
    // bn show returns a single object
    serde_json::from_str(json).map_err(|e| AdapterError::ParseFailed {
        tool: "bn show",
        detail: e.to_string(),
    })
}

// --- Seal Reviews ---

/// Parsed output from `seal reviews list --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewsListResponse {
    #[serde(default)]
    pub reviews: Vec<ReviewSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewSummary {
    pub review_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub change_id: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
}

/// Parsed output from `seal review <id> --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewDetailResponse {
    pub review: ReviewDetail,
    #[serde(default)]
    pub threads: Vec<ReviewThread>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewDetail {
    pub review_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub change_id: Option<String>,
    #[serde(default)]
    pub votes: Vec<ReviewVote>,
    #[serde(default)]
    pub open_thread_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewVote {
    pub reviewer: String,
    pub vote: String,
    #[serde(default)]
    pub voted_at: Option<String>,
}

impl ReviewVote {
    pub fn is_lgtm(&self) -> bool {
        self.vote == "lgtm"
    }

    pub fn is_block(&self) -> bool {
        self.vote == "block"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewThread {
    pub thread_id: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

// --- Adapter Errors ---

#[derive(Debug, Clone)]
pub enum AdapterError {
    ParseFailed { tool: &'static str, detail: String },
    NotFound { tool: &'static str, detail: String },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdapterError::ParseFailed { tool, detail } => {
                write!(f, "failed to parse {tool} output: {detail}")
            }
            AdapterError::NotFound { tool, detail } => {
                write!(f, "{tool}: {detail}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

// --- Convenience parsers ---

/// Parse `bus claims list --format json`.
pub fn parse_claims(json: &str) -> Result<ClaimsResponse, AdapterError> {
    serde_json::from_str(json).map_err(|e| AdapterError::ParseFailed {
        tool: "bus claims list",
        detail: e.to_string(),
    })
}

/// Parse `maw ws list --format json`.
pub fn parse_workspaces(json: &str) -> Result<WorkspacesResponse, AdapterError> {
    serde_json::from_str(json).map_err(|e| AdapterError::ParseFailed {
        tool: "maw ws list",
        detail: e.to_string(),
    })
}

/// Parse `seal reviews list --format json`.
pub fn parse_reviews_list(json: &str) -> Result<ReviewsListResponse, AdapterError> {
    serde_json::from_str(json).map_err(|e| AdapterError::ParseFailed {
        tool: "seal reviews list",
        detail: e.to_string(),
    })
}

/// Parse `seal review <id> --format json`.
pub fn parse_review_detail(json: &str) -> Result<ReviewDetailResponse, AdapterError> {
    serde_json::from_str(json).map_err(|e| AdapterError::ParseFailed {
        tool: "seal review",
        detail: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Claims parsing ---

    #[test]
    fn parse_claims_basic() {
        let json = r#"{"claims": [
            {"agent": "myapp-dev", "patterns": ["bone://myapp/bd-abc"], "active": true, "memo": "bd-abc"},
            {"agent": "myapp-dev", "patterns": ["workspace://myapp/frost-castle"], "active": true}
        ]}"#;
        let resp = parse_claims(json).unwrap();
        assert_eq!(resp.claims.len(), 2);
        assert_eq!(resp.claims[0].agent, "myapp-dev");
        assert_eq!(resp.claims[0].bone_ids(), vec!["bd-abc"]);
        assert_eq!(resp.claims[1].workspace_names(), vec!["frost-castle"]);
    }

    #[test]
    fn parse_claims_empty() {
        let json = r#"{"claims": []}"#;
        let resp = parse_claims(json).unwrap();
        assert!(resp.claims.is_empty());
    }

    #[test]
    fn parse_claims_missing_optional_fields() {
        let json = r#"{"claims": [{"agent": "dev", "patterns": ["bone://p/bd-x"]}]}"#;
        let resp = parse_claims(json).unwrap();
        assert!(!resp.claims[0].active); // defaults to false
        assert!(resp.claims[0].memo.is_none());
        assert!(resp.claims[0].expires_at.is_none());
    }

    #[test]
    fn parse_claims_extra_fields_tolerated() {
        let json = r#"{"claims": [{"agent": "dev", "patterns": [], "some_new_field": 42}]}"#;
        let resp = parse_claims(json).unwrap();
        assert_eq!(resp.claims.len(), 1);
    }

    #[test]
    fn parse_claims_invalid_json() {
        let result = parse_claims("not json");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("bus claims list"));
    }

    // --- Workspace parsing ---

    #[test]
    fn parse_workspaces_basic() {
        let json = r#"{"workspaces": [
            {"name": "default", "is_default": true, "is_current": false, "change_id": "abc123"},
            {"name": "frost-castle", "is_default": false, "is_current": true, "change_id": "def456"}
        ], "advice": []}"#;
        let resp = parse_workspaces(json).unwrap();
        assert_eq!(resp.workspaces.len(), 2);
        assert!(resp.workspaces[0].is_default);
        assert_eq!(resp.workspaces[1].name, "frost-castle");
    }

    #[test]
    fn parse_workspaces_with_advice() {
        let json = r#"{"workspaces": [], "advice": [
            {"level": "warn", "message": "stale workspace detected", "details": "frost-castle"}
        ]}"#;
        let resp = parse_workspaces(json).unwrap();
        assert_eq!(resp.advice.len(), 1);
        assert!(resp.advice[0].message.contains("stale"));
    }

    #[test]
    fn parse_workspaces_missing_advice() {
        let json = r#"{"workspaces": [{"name": "default", "is_default": true}]}"#;
        let resp = parse_workspaces(json).unwrap();
        assert!(resp.advice.is_empty());
    }

    // --- Bone parsing ---

    #[test]
    fn parse_bone_show_basic() {
        let json = r#"{"id": "bd-abc", "title": "Fix login", "state": "doing", "assignees": ["myapp-dev"], "labels": ["bug"]}"#;
        let bone = parse_bone_show(json).unwrap();
        assert_eq!(bone.id, "bd-abc");
        assert_eq!(bone.title, "Fix login");
        assert_eq!(bone.state, "doing");
        assert_eq!(bone.assignees, vec!["myapp-dev"]);
        assert_eq!(bone.labels, vec!["bug"]);
    }

    #[test]
    fn parse_bone_show_minimal() {
        let json = r#"{"id": "bd-abc"}"#;
        let bone = parse_bone_show(json).unwrap();
        assert_eq!(bone.id, "bd-abc");
        assert_eq!(bone.title, "");
        assert_eq!(bone.state, "");
        assert!(bone.assignees.is_empty());
        assert!(bone.labels.is_empty());
    }

    #[test]
    fn parse_bone_show_invalid_json() {
        let result = parse_bone_show("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bn show"));
    }

    #[test]
    fn parse_bone_show_extra_fields() {
        let json = r#"{"id": "bd-x", "title": "t", "state": "open", "some_future_field": true}"#;
        let bone = parse_bone_show(json).unwrap();
        assert_eq!(bone.id, "bd-x");
    }

    // --- Review parsing ---

    #[test]
    fn parse_reviews_list_basic() {
        let json = r#"{"reviews": [
            {"review_id": "cr-abc", "title": "feat: login", "status": "open", "change_id": "xyz"}
        ]}"#;
        let resp = parse_reviews_list(json).unwrap();
        assert_eq!(resp.reviews.len(), 1);
        assert_eq!(resp.reviews[0].review_id, "cr-abc");
    }

    #[test]
    fn parse_reviews_list_empty() {
        let json = r#"{"reviews": []}"#;
        let resp = parse_reviews_list(json).unwrap();
        assert!(resp.reviews.is_empty());
    }

    #[test]
    fn parse_review_detail_with_votes() {
        let json = r#"{
            "review": {
                "review_id": "cr-abc",
                "status": "reviewed",
                "votes": [
                    {"reviewer": "myapp-security", "vote": "lgtm", "voted_at": "2026-02-16T10:00:00Z"},
                    {"reviewer": "myapp-perf", "vote": "block", "voted_at": "2026-02-16T11:00:00Z"}
                ],
                "open_thread_count": 2
            },
            "threads": [
                {"thread_id": "th-1", "file": "src/main.rs", "line": 42, "resolved": false, "comments": [
                    {"author": "myapp-security", "body": "Missing validation", "created_at": "2026-02-16T10:00:00Z"}
                ]}
            ]
        }"#;
        let resp = parse_review_detail(json).unwrap();
        assert_eq!(resp.review.review_id, "cr-abc");
        assert_eq!(resp.review.votes.len(), 2);
        assert!(resp.review.votes[0].is_lgtm());
        assert!(resp.review.votes[1].is_block());
        assert_eq!(resp.review.open_thread_count, 2);
        assert_eq!(resp.threads.len(), 1);
        assert_eq!(resp.threads[0].comments.len(), 1);
    }

    #[test]
    fn parse_review_detail_minimal() {
        let json = r#"{"review": {"review_id": "cr-x", "status": "open"}, "threads": []}"#;
        let resp = parse_review_detail(json).unwrap();
        assert_eq!(resp.review.review_id, "cr-x");
        assert!(resp.review.votes.is_empty());
        assert_eq!(resp.review.open_thread_count, 0);
    }

    #[test]
    fn parse_review_detail_extra_fields() {
        let json = r#"{"review": {"review_id": "cr-x", "status": "open", "new_field": "val"}, "threads": []}"#;
        let resp = parse_review_detail(json).unwrap();
        assert_eq!(resp.review.review_id, "cr-x");
    }

    // --- Claim helper tests ---

    #[test]
    fn claim_bone_id_extraction() {
        let claim = Claim {
            agent: "dev".into(),
            patterns: vec![
                "bone://myapp/bd-abc".into(),
                "workspace://myapp/ws".into(),
                "agent://myapp-dev".into(),
            ],
            active: true,
            memo: None,
            expires_at: None,
        };
        assert_eq!(claim.bone_ids(), vec!["bd-abc"]);
        assert_eq!(claim.workspace_names(), vec!["ws"]);
    }

    #[test]
    fn claim_no_matching_patterns() {
        let claim = Claim {
            agent: "dev".into(),
            patterns: vec!["agent://myapp-dev".into()],
            active: true,
            memo: None,
            expires_at: None,
        };
        assert!(claim.bone_ids().is_empty());
        assert!(claim.workspace_names().is_empty());
    }
}
