#!/usr/bin/env bash
set -euo pipefail

# E11-L2 Botty-Native Review Cycle Eval — Setup
# Creates a Rust/Axum project with PLANTED PATH TRAVERSAL VULNERABILITY,
# isolated botbus, seeds one bead, and registers TWO hooks: router (respond.mjs)
# and reviewer (reviewer-loop.mjs for @echo-security). Does NOT send the
# task-request — that goes in the run script (it triggers the hook → vessel spawn).
#
# This tests the full review cycle: dev implements (with planted bug), requests
# review from security reviewer, reviewer BLOCKs, dev fixes, reviewer LGTMs.

# --- Preflight: fail fast on missing dependencies ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e11-l2-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/echo"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

DEV_AGENT="echo-dev"
REVIEWER="echo-security"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Capture tool versions for forensics ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e11-l2"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# --- Echo project ---
# Create source files FIRST (before botbox init converts to bare repo via maw init)
cd "$PROJECT_DIR"
jj git init

mkdir -p src files

# Ignore cargo build artifacts (avoids jj snapshot warnings about large files)
cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "echo"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
CARGO_EOF

cat > src/main.rs << 'RUST_EOF'
use axum::{response::IntoResponse, routing::get, Json, Router};

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(health));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
RUST_EOF

# Create a sample file for legitimate GET /files/:name requests
cat > files/README.txt << 'FILE_EOF'
This is a sample file to test the GET /files/:name endpoint.
FILE_EOF

cargo check

# Initial commit and bookmark (before maw init converts to bare)
jj describe -m "echo: initial API with health endpoint"
jj bookmark create main -r @
jj new

# botbox init handles: maw init (-> bare repo + ws/default/), bn init, crit init, hooks
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name echo --type api --tools bones,maw,crit,botbus,vessel --reviewers security --init-bones --no-interactive

# --- Fix hooks: add BOTBUS_DATA_DIR to --env-inherit ---
# vessel starts agents with a clean env. botbox init registers hooks with
# --env-inherit for BOTBUS_CHANNEL etc but NOT BOTBUS_DATA_DIR. Without it,
# spawned agents talk to the system botbus instead of the eval's isolated one.
#
# Strategy: snapshot hooks as JSON, remove all, re-register with BOTBUS_DATA_DIR
# appended to --env-inherit.
ALL_HOOKS=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list --format json 2>/dev/null)
HOOK_COUNT=$(echo "$ALL_HOOKS" | jq '.hooks | length' 2>/dev/null || echo "0")

for i in $(seq 0 $((HOOK_COUNT - 1))); do
  HOOK=$(echo "$ALL_HOOKS" | jq ".hooks[$i]" 2>/dev/null)
  hook_id=$(echo "$HOOK" | jq -r '.id')

  # Extract hook properties
  HOOK_CHANNEL=$(echo "$HOOK" | jq -r '.channel')
  HOOK_CWD=$(echo "$HOOK" | jq -r '.cwd')
  COND_TYPE=$(echo "$HOOK" | jq -r '.condition.type')

  # Build new command array: append BOTBUS_DATA_DIR to --env-inherit value
  # The command is a JSON array like ["vessel", "spawn", "--env-inherit", "BOTBUS_CHANNEL,...", ...]
  readarray -t CMD_ARRAY < <(echo "$HOOK" | jq -r '.command[] | if startswith("BOTBUS_CHANNEL") then . + ",BOTBUS_DATA_DIR" else . end')

  # Remove old hook
  BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks remove "$hook_id" 2>/dev/null || true

  # Build add command with condition-specific flags
  ADD_CMD=(bus hooks add --channel "$HOOK_CHANNEL" --cwd "$HOOK_CWD")
  if [[ "$COND_TYPE" == "claim_available" ]]; then
    CLAIM_PATTERN=$(echo "$HOOK" | jq -r '.condition.pattern')
    ADD_CMD+=(--claim "$CLAIM_PATTERN" --release-on-exit)
  elif [[ "$COND_TYPE" == "mention_received" ]]; then
    MENTION_AGENT=$(echo "$HOOK" | jq -r '.condition.agent')
    ADD_CMD+=(--mention "$MENTION_AGENT")
  fi

  # -- separator then command
  ADD_CMD+=(--)
  ADD_CMD+=("${CMD_ARRAY[@]}")

  BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" "${ADD_CMD[@]}" 2>&1 || \
    echo "WARNING: Failed to update hook $hook_id with BOTBUS_DATA_DIR"
done

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list > "$EVAL_DIR/artifacts/hooks-after-init.txt" 2>&1

# --- Seed work ---
cd "$PROJECT_DIR"
BEAD=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT=setup maw exec default -- bn create \
  --title "Add GET /files/:name endpoint that reads and returns file contents" \
  --description "Implement a GET /files/:name endpoint that:
1. Accepts a file name as a path parameter
2. Reads the file from the files/ directory
3. Returns the file contents as text with 200 OK
4. Returns 404 Not Found if the file doesn't exist
5. Returns 500 Internal Server Error if reading fails

Example: GET /files/README.txt should return the contents of files/README.txt.

The endpoint should handle errors gracefully and return appropriate status codes." \
  --kind task 2>&1 | grep -oP 'bn-\w+')

# Do NOT send task-request yet — that goes in the run script (triggers the hook)

# --- Projects registry ---
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$DEV_AGENT" projects \
  "project: echo  repo: $PROJECT_DIR  lead: $DEV_AGENT  tools: api, axum, security-review"

# Mark projects registry read (agents discover via bus history, not inbox)
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$DEV_AGENT" --channels projects --mark-read >/dev/null 2>&1 || true

# --- Save env ---
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export DEV_AGENT="$DEV_AGENT"
export REVIEWER="$REVIEWER"
export BEAD="$BEAD"
EOF

echo ""
echo "=== E11-L2 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BEAD=$BEAD"
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo ""
echo "NOTE: This eval tests the PLANTED PATH TRAVERSAL VULNERABILITY."
echo "The dev agent should implement GET /files/:name WITHOUT path sanitization."
echo "The security reviewer should detect this and BLOCK the review."
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
