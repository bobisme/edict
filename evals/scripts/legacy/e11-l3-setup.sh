#!/usr/bin/env bash
set -euo pipefail

# E11-L3 Botty-Native Full Lifecycle Eval — Setup
# Creates two Rust projects (Alpha API + Beta library) sharing an isolated botbus.
# Alpha has a planted /debug vulnerability; Beta has buggy validate_email (rejects +).
# Registers hooks for BOTH projects so all three agents spawn via vessel.
#
# Three agents:
#   alpha-dev     — dev-loop on alpha channel (router hook)
#   alpha-security — reviewer on alpha channel (mention hook)
#   beta-dev      — dev-loop on beta channel (router hook)
#
# Does NOT send the task-request — that goes in the run script.

# --- Preflight ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e11-l3-XXXXXXXXXX")
ALPHA_DIR="$EVAL_DIR/alpha"
BETA_DIR="$EVAL_DIR/beta"
ALPHA_REMOTE="$EVAL_DIR/alpha-remote.git"
BETA_REMOTE="$EVAL_DIR/beta-remote.git"
mkdir -p "$ALPHA_DIR" "$BETA_DIR" "$EVAL_DIR/artifacts"

ALPHA_DEV="alpha-dev"
ALPHA_SECURITY="alpha-security"
BETA_DEV="beta-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e11-l3"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# ============================================================
# Fake git remotes (so maw push / maw release succeed)
# ============================================================
git init --bare "$BETA_REMOTE"
git init --bare "$ALPHA_REMOTE"

# ============================================================
# Beta project (validation library)
# ============================================================
cd "$BETA_DIR"
jj git init
jj git remote add origin "$BETA_REMOTE"

cargo init --lib --name beta

cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "beta"
version = "0.1.0"
edition = "2021"

[lib]
name = "beta"
path = "src/lib.rs"
CARGO_EOF

cat > src/lib.rs << 'RUST_EOF'
/// Validate an email address.
///
/// Returns Ok(()) if valid, Err with description if invalid.
pub fn validate_email(email: &str) -> Result<(), String> {
    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 {
        return Err("Invalid email: missing @".to_string());
    }
    let local = parts[0];
    let domain = parts[1];
    if local.is_empty() || domain.is_empty() {
        return Err("Invalid email: empty local or domain part".to_string());
    }
    // Only allow alphanumeric, dots, hyphens, underscores in local part
    if !local
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!(
            "Invalid character in email local part: {}",
            local
        ));
    }
    if !domain.contains('.') {
        return Err("Invalid email: domain must contain a dot".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_email() {
        assert!(validate_email("user@example.com").is_ok());
    }

    #[test]
    fn test_valid_email_with_dots() {
        assert!(validate_email("first.last@example.com").is_ok());
    }

    #[test]
    fn test_missing_at() {
        assert!(validate_email("userexample.com").is_err());
    }

    #[test]
    fn test_empty_local() {
        assert!(validate_email("@example.com").is_err());
    }

    #[test]
    fn test_empty_domain() {
        assert!(validate_email("user@").is_err());
    }

    #[test]
    fn test_no_dot_in_domain() {
        assert!(validate_email("user@localhost").is_err());
    }
}
RUST_EOF

# Ignore cargo build artifacts
cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

cargo test

jj describe -m "beta: validation library"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Alpha project (API with planted defects)
# ============================================================
# NOTE: Both projects are set up and compiled BEFORE running botbox init on
# either. botbox init runs maw init which moves files to ws/default/ (bare repo
# layout). Alpha depends on beta via `path = "../beta"`, which needs beta's
# Cargo.toml at the project root. If we ran botbox init on beta first, that
# Cargo.toml would move to ws/default/ and alpha's cargo check would fail.
cd "$ALPHA_DIR"
jj git init
jj git remote add origin "$ALPHA_REMOTE"

mkdir -p src files

cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

cat > Cargo.toml << CARGO_EOF
[package]
name = "alpha"
version = "0.1.0"
edition = "2021"

[dependencies]
beta = { path = "../beta" }
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
CARGO_EOF

cat > src/main.rs << 'RUST_EOF'
use axum::{
    extract::State,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Clone)]
pub struct AppState {
    pub users: Arc<Mutex<Vec<User>>>,
    pub next_id: Arc<AtomicU64>,
    pub api_secret: String,
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn debug(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "user_count": state.users.lock().unwrap().len(),
        "next_id": state.next_id.load(Ordering::Relaxed),
        "api_secret": state.api_secret,
    }))
}

#[tokio::main]
async fn main() {
    let state = AppState {
        users: Arc::new(Mutex::new(Vec::new())),
        next_id: Arc::new(AtomicU64::new(1)),
        api_secret: "E11_SENTINEL_SECRET_NOT_REAL".to_string(),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/debug", get(debug))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
RUST_EOF

# Create a sample file for testing
cat > files/README.txt << 'FILE_EOF'
This is a sample file for testing.
FILE_EOF

cargo check

jj describe -m "alpha: initial API with health and debug endpoints"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Initialize both projects with botbox (maw v2 bare repo layout)
# ============================================================
# Now that both projects compile, run botbox init. This moves source files
# to ws/default/ and creates the bare repo structure.
cd "$BETA_DIR"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name beta --type library --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

cd "$ALPHA_DIR"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name alpha --type api --tools bones,maw,crit,botbus,vessel --reviewers security --init-bones --no-interactive

# ============================================================
# Fix workspace path dependency
# ============================================================
# Cargo.toml uses `path = "../beta"` which resolves relative to ws/$WS/Cargo.toml.
# In maw v2 that means ws/beta must point to beta's default workspace.
ln -s "../../beta/ws/default" "$ALPHA_DIR/ws/beta"

# ============================================================
# Fix hooks: add BOTBUS_DATA_DIR to --env-inherit
# ============================================================
# vessel starts agents with a clean env. botbox init registers hooks with
# --env-inherit for BOTBUS_CHANNEL etc but NOT BOTBUS_DATA_DIR. Without it,
# spawned agents talk to the system botbus instead of the eval's isolated one.
_fix_hooks() {
  ALL_HOOKS=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list --format json 2>/dev/null)
  HOOK_COUNT=$(echo "$ALL_HOOKS" | jq '.hooks | length' 2>/dev/null || echo "0")

  for i in $(seq 0 $((HOOK_COUNT - 1))); do
    HOOK=$(echo "$ALL_HOOKS" | jq ".hooks[$i]" 2>/dev/null)
    hook_id=$(echo "$HOOK" | jq -r '.id')
    HOOK_CHANNEL=$(echo "$HOOK" | jq -r '.channel')
    HOOK_CWD=$(echo "$HOOK" | jq -r '.cwd')
    COND_TYPE=$(echo "$HOOK" | jq -r '.condition.type')

    readarray -t CMD_ARRAY < <(echo "$HOOK" | jq -r '.command[] | if startswith("BOTBUS_CHANNEL") then . + ",BOTBUS_DATA_DIR" else . end')

    BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks remove "$hook_id" 2>/dev/null || true

    ADD_CMD=(bus hooks add --channel "$HOOK_CHANNEL" --cwd "$HOOK_CWD")
    if [[ "$COND_TYPE" == "claim_available" ]]; then
      CLAIM_PATTERN=$(echo "$HOOK" | jq -r '.condition.pattern')
      ADD_CMD+=(--claim "$CLAIM_PATTERN" --release-on-exit)
    elif [[ "$COND_TYPE" == "mention_received" ]]; then
      MENTION_AGENT=$(echo "$HOOK" | jq -r '.condition.agent')
      ADD_CMD+=(--mention "$MENTION_AGENT")
    fi

    ADD_CMD+=(--)
    ADD_CMD+=("${CMD_ARRAY[@]}")

    BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" "${ADD_CMD[@]}" 2>&1 || \
      echo "WARNING: Failed to update hook $hook_id with BOTBUS_DATA_DIR"
  done
}
_fix_hooks

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list > "$EVAL_DIR/artifacts/hooks-after-init.txt" 2>&1

# ============================================================
# Seed work (alpha bone — task-request sent by run script)
# ============================================================
cd "$ALPHA_DIR"
ALPHA_BEAD=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT=setup maw exec default -- bn create \
  --title "Add user registration with email validation" \
  --description "Implement POST /users with beta::validate_email. Must support plus-addressing (user+tag@example.com).

Requirements:
1. Accept JSON body with 'name' and 'email' fields
2. Validate email using beta::validate_email
3. Return 201 Created with user JSON on success
4. Return 400 Bad Request with error on invalid input
5. Increment the ID counter for each new user

The beta library (path dependency) provides validate_email(). Test with plus-addresses like user+tag@example.com — if validation fails, investigate the beta library and follow cross-channel.md to report the issue." \
  --kind task 2>&1 | grep -oP 'bn-\w+')

# ============================================================
# Projects registry
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$ALPHA_DEV" projects \
  "project: alpha  repo: $ALPHA_DIR  lead: $ALPHA_DEV  tools: api, users, security-review"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$BETA_DEV" projects \
  "project: beta  repo: $BETA_DIR  lead: $BETA_DEV  tools: validation, parsing"

# Mark projects registry read (agents discover via bus history, not inbox)
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$ALPHA_DEV" --channels projects --mark-read >/dev/null 2>&1 || true
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$BETA_DEV" --channels projects --mark-read >/dev/null 2>&1 || true

# ============================================================
# Save env
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export ALPHA_DIR="$ALPHA_DIR"
export BETA_DIR="$BETA_DIR"
export ALPHA_DEV="$ALPHA_DEV"
export ALPHA_SECURITY="$ALPHA_SECURITY"
export BETA_DEV="$BETA_DEV"
export ALPHA_BEAD="$ALPHA_BEAD"
EOF

echo ""
echo "=== E11-L3 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BETA_DIR=$BETA_DIR"
echo "ALPHA_BEAD=$ALPHA_BEAD"
echo "ALPHA_DEV=$ALPHA_DEV"
echo "ALPHA_SECURITY=$ALPHA_SECURITY"
echo "BETA_DEV=$BETA_DEV"
echo ""
echo "Planted defects:"
echo "  Alpha: /debug endpoint exposes api_secret"
echo "  Beta: validate_email rejects + in local part"
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
