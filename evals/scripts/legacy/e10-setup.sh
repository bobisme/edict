#!/usr/bin/env bash
set -euo pipefail

# E10 Full Lifecycle Eval — Setup
# Creates two Rust projects (Alpha API + Beta library) sharing an isolated botbus instance.
# Alpha has a planted /debug vulnerability; Beta has a buggy validate_email (rejects +).
# Seeds a registration bone on Alpha, registers projects in #projects channel.
#
# Both projects use maw v2 layout (bare repo + ws/default/).

# --- Preflight: fail fast on missing dependencies ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo claude jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
# Use persistent directory so forensics survive reboots (tmpfs wipes /tmp)
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e10-XXXXXXXXXX")
ALPHA_DIR="$EVAL_DIR/alpha"
BETA_DIR="$EVAL_DIR/beta"
mkdir -p "$ALPHA_DIR" "$BETA_DIR" "$EVAL_DIR/artifacts"

ALPHA_DEV="alpha-dev"
ALPHA_SECURITY="alpha-security"
BETA_DEV="beta-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Capture tool versions for forensics ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "model_alpha=opus"
  echo "model_beta=sonnet"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# --- Beta project ---
# Create source files FIRST (before botbox init converts to bare repo via maw init)
cd "$BETA_DIR"
jj git init

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

cargo test

# Initial commit and bookmark (before maw init converts to bare)
jj describe -m "beta: validation library"
jj bookmark create main -r @
jj new

# botbox init handles: maw init (→ bare repo + ws/default/), bn init, crit init, hooks
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name beta --type library --tools bones,maw,crit,botbus --init-bones --no-interactive

# --- Alpha project ---
# Create source files FIRST (before botbox init converts to bare repo via maw init)
cd "$ALPHA_DIR"
jj git init

cargo init --name alpha

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
        api_secret: "E10_SENTINEL_SECRET_NOT_REAL".to_string(),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/debug", get(debug))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
RUST_EOF

cargo check

# Initial commit and bookmark (before maw init converts to bare)
jj describe -m "alpha: initial API with health and debug endpoints"
jj bookmark create main -r @
jj new

# botbox init handles: maw init (→ bare repo + ws/default/), bn init, crit init, hooks
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name alpha --type api --tools bones,maw,crit,botbus,vessel --reviewers security --init-bones --no-interactive

# --- Hooks (verify they were registered by botbox init) ---
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list > "$EVAL_DIR/artifacts/hooks-after-init.txt" 2>&1

# --- Fix workspace path dependency ---
# Cargo.toml uses `path = "../beta"` which resolves relative to the Cargo.toml location.
# In maw v2, Cargo.toml is at ws/$WS/Cargo.toml, so `../beta` → ws/beta.
# Create symlink: ws/beta → beta's default workspace (where Cargo.toml + src/ live).
ln -s "../../beta/ws/default" "$ALPHA_DIR/ws/beta"

# --- Seed work ---
cd "$ALPHA_DIR"
BEAD=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT=setup maw exec default -- bn create \
  --title "Add user registration with email validation" \
  --description "Implement POST /users with beta::validate_email. Must support plus-addressing (user+tag@example.com)." \
  --kind feature 2>&1 | grep -oP 'bn-\w+')

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent setup alpha \
  "New task: Add POST /users registration endpoint. Must support standard email formats including subaddressing (user+tag@example.com). Use beta's validate_email for validation." \
  -L task-request

# --- Projects registry ---
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$BETA_DEV" projects \
  "project: beta  repo: $BETA_DIR  lead: $BETA_DEV  tools: validation, parsing"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$ALPHA_DEV" projects \
  "project: alpha  repo: $ALPHA_DIR  lead: $ALPHA_DEV  tools: api, users"

# --- Mark projects registry messages read (agents discover projects via `bus history`, not inbox) ---
# NOTE: Do NOT mark alpha channel as read for alpha-dev — they need to discover the task-request in Phase 1
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$ALPHA_DEV" --channels projects --mark-read >/dev/null 2>&1 || true
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$BETA_DEV" --channels projects --mark-read >/dev/null 2>&1 || true

# --- Save env ---
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export ALPHA_DIR="$ALPHA_DIR"
export BETA_DIR="$BETA_DIR"
export ALPHA_DEV="$ALPHA_DEV"
export ALPHA_SECURITY="$ALPHA_SECURITY"
export BETA_DEV="$BETA_DEV"
export BEAD="$BEAD"
EOF

echo ""
echo "=== E10 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BETA_DIR=$BETA_DIR"
echo "BEAD=$BEAD"
echo ""
echo "Source .eval-env before running phases:"
echo "  source $EVAL_DIR/.eval-env"
