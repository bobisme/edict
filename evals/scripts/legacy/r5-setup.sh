#!/usr/bin/env bash
set -euo pipefail

# R5 Cross-Project Coordination Eval — Setup
# Creates two projects:
#   r5-utils: Rust lib crate with a buggy validate_name() function
#   r5-app:   Rust/Axum API with a bone that references r5-utils
# Populates the #projects registry on botbus so the agent can discover r5-utils.
# Uses BOTBUS_DATA_DIR isolation to avoid polluting the real registry.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EVAL_DIR=$(mktemp -d)
echo "EVAL_DIR=$EVAL_DIR"

# --- Isolated botbus ---
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init --quiet

UTILS_DIR="$EVAL_DIR/r5-utils"
APP_DIR="$EVAL_DIR/r5-app"
mkdir -p "$UTILS_DIR" "$APP_DIR"

# ==============================
# r5-utils: library with a bug
# ==============================
cd "$UTILS_DIR"
jj git init
botbox init --name r5-utils --type library --tools bones,maw,crit,botbus --init-bones --no-interactive

# --- Copy latest workflow docs ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust lib project ---
cargo init --lib --name r5-utils

# --- Cargo.toml (lib crate) ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r5-utils"
version = "0.2.0"
edition = "2021"

[lib]
name = "r5_utils"
path = "src/lib.rs"
CARGO_EOF

# --- src/lib.rs with buggy validate_name ---
cat > src/lib.rs << 'RUST_EOF'
/// Name validation utilities.
///
/// Provides functions for validating user-provided names
/// according to our standard rules.

/// Validates a name string.
///
/// # Rules
/// - Rejects names shorter than 2 characters
/// - Rejects names longer than 50 characters
/// - Returns Ok(()) if the name is valid
///
/// # Examples
/// ```
/// use r5_utils::validate_name;
/// assert!(validate_name("Alice").is_ok());
/// assert!(validate_name("").is_err());
/// ```
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.len() < 1 {
        return Err("Name too short (minimum 2 characters)".to_string());
    }
    if name.len() > 50 {
        return Err("Name too long (maximum 50 characters)".to_string());
    }
    Ok(())
}

/// Validates an email address (basic format check).
pub fn validate_email(email: &str) -> Result<(), String> {
    if !email.contains('@') {
        return Err("Invalid email: missing @".to_string());
    }
    if email.len() > 254 {
        return Err("Email too long".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_name_rejected() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn test_valid_name() {
        assert!(validate_name("Alice").is_ok());
    }

    #[test]
    fn test_long_name_rejected() {
        let long = "a".repeat(51);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn test_valid_email() {
        assert!(validate_email("alice@example.com").is_ok());
    }

    #[test]
    fn test_invalid_email() {
        assert!(validate_email("not-an-email").is_err());
    }
}
RUST_EOF

# --- Verify it compiles and tests pass ---
cargo check 2>&1
cargo test 2>&1
echo "r5-utils: cargo check + cargo test OK"

# --- Init crit and maw ---
crit init
maw init

# --- Create a couple existing bones so it looks like a real project ---
bn create \
  --title "Add validate_phone() function" \
  --description "Add phone number validation: must be 10-15 digits, optional + prefix." \
  --kind feature

bn create \
  --title "Add unit tests for validate_email edge cases" \
  --description "Cover: multiple @, unicode, very long local part, missing domain." \
  --kind task

# --- Commit baseline ---
jj describe -m "r5-utils: validation library with validate_name and validate_email"
jj new

echo "r5-utils initialized at $UTILS_DIR"

# ==============================
# r5-app: API project
# ==============================
cd "$APP_DIR"
jj git init
botbox init --name r5-app --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Copy latest workflow docs ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust project ---
cargo init --name r5-app

# --- Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r5-app"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
CARGO_EOF

# --- src/main.rs with AppState and /health ---
cat > src/main.rs << 'RUST_EOF'
use axum::{routing::get, Router};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicU64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Clone)]
pub struct AppState {
    pub users: Arc<Mutex<Vec<User>>>,
    pub next_id: Arc<AtomicU64>,
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let state = AppState {
        users: Arc::new(Mutex::new(Vec::new())),
        next_id: Arc::new(AtomicU64::new(1)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("bind");
    println!("Listening on :3000");
    axum::serve(listener, app).await.expect("serve");
}
RUST_EOF

# --- Verify it compiles ---
cargo check 2>&1
echo "r5-app: cargo check OK"

# --- Init crit and maw ---
crit init
maw init

# --- Generate agent identity ---
AGENT=$(bus generate-name)
echo "AGENT=$AGENT"

# --- Register on channel ---
bus send --agent setup r5-app "R5 eval environment initialized" -L mesh -L setup
bus mark-read --agent "$AGENT" r5-app

# --- Create the main bone ---
BEAD=$(bn create \
  --title "Add POST /users endpoint with name validation" \
  --description "$(cat << DESC_EOF
Add a POST /users endpoint that creates a new user with name validation.

## Requirements

- POST /users accepts JSON body: {"name": "..."}
- Validate the name before creating the user
- Return 201 with the created user as JSON: {"id": N, "name": "..."}
- Return 400 with {"error": "..."} if validation fails
- Create a new module src/create_user.rs with the handler
- Add the route to the Router in src/main.rs (use axum::routing::post)

## Name Validation

Use the validation logic from the r5-utils project as a reference.
The r5-utils library is at: $UTILS_DIR
Read the validate_name() function in $UTILS_DIR/src/lib.rs.

**Our validation requirements**: names must be at least 2 characters and at most 50 characters.

Verify that the upstream validate_name() logic matches our requirements before porting it.
If you find bugs or discrepancies in the upstream code, follow .agents/botbox/report-issue.md
to file them in the upstream project.

## Acceptance Criteria

- POST /users with {"name": "Alice"} returns 201 with user
- POST /users with {"name": ""} returns 400
- POST /users with {"name": "A"} returns 400 (single char too short)
- cargo check passes

## Testing

Verify with curl:
  curl -X POST -H 'Content-Type: application/json' -d '{"name":"Alice"}' http://localhost:3000/users
DESC_EOF
)" \
  --kind task)
echo "BEAD=$BEAD"

# --- Commit baseline ---
jj describe -m "r5-app: initial project setup with User struct and health endpoint"
jj new

echo "r5-app initialized at $APP_DIR"

# ==============================
# Populate #projects registry
# ==============================
bus send --agent r5-utils-dev projects \
  "project: r5-utils  repo: $UTILS_DIR  lead: r5-utils-dev  tools: validate, parse" \
  -L mesh -L project-registry

bus send --agent r5-app-dev projects \
  "project: r5-app  repo: $APP_DIR  lead: r5-app-dev  tools: api" \
  -L mesh -L project-registry

# Mark the projects channel as read for the agent so it doesn't show up in inbox
bus mark-read --agent "$AGENT" projects

echo ""
echo "=== R5 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "AGENT=$AGENT"
echo "APP_DIR=$APP_DIR"
echo "UTILS_DIR=$UTILS_DIR"
echo "BEAD=$BEAD"
echo ""
echo "Projects:"
echo "  r5-utils: $UTILS_DIR (lib with buggy validate_name — checks < 1 instead of < 2)"
echo "  r5-app:   $APP_DIR (API with POST /users bone referencing r5-utils)"
echo ""
echo "Registry:"
echo "  bus history projects  (should show both project entries)"
echo ""
echo "Verify:"
echo "  cd $UTILS_DIR && cargo check && cargo test"
echo "  cd $APP_DIR && cargo check"
echo "  cd $APP_DIR && bn next  (should show 1 bone)"
echo "  bus history projects     (should show 2 registry entries)"
echo ""
echo "Next: source $APP_DIR/.eval-env && bash $REPO_DIR/evals/scripts/r5-run.sh"

# --- Save env ---
cat > "$APP_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR"
export AGENT="$AGENT"
export APP_DIR="$APP_DIR"
export UTILS_DIR="$UTILS_DIR"
export BEAD="$BEAD"
EOF
