#!/usr/bin/env bash
set -euo pipefail

# E12-Proto-Lead Eval — Setup
# Creates a greeter project with a PRE-COMPLETED worker workspace.
# The bone is done, code is implemented, tests pass, workspace is left for the lead.
# Tests whether the dev-loop lead uses `botbox protocol merge` to merge it.

# --- Preflight ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-/tmp/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e12-lead-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/greeter"
PROJECT_REMOTE="$EVAL_DIR/greeter-remote.git"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

LEAD_AGENT="greeter-dev"
WORKER_AGENT="greeter-dev/eval-worker"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e12-proto-lead"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# ============================================================
# Fake git remote (so maw push succeeds)
# ============================================================
git init --bare "$PROJECT_REMOTE"

# ============================================================
# Create greeter project
# ============================================================
cd "$PROJECT_DIR"
jj git init
jj git remote add origin "$PROJECT_REMOTE"

mkdir -p src

# --- Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "greeter"
version = "0.1.0"
edition = "2021"

[dependencies]
CARGO_EOF

# --- src/main.rs ---
cat > src/main.rs << 'RUST_EOF'
mod greet;

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "world".to_string());
    println!("{}", greet::hello(&name));
}
RUST_EOF

# --- src/greet.rs (the todo! stub — worker already implemented this) ---
cat > src/greet.rs << 'RUST_EOF'
/// Return a greeting string for the given name.
pub fn hello(_name: &str) -> String {
    todo!("Return a greeting: hello, <name>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_name() {
        assert_eq!(hello("Alice"), "hello, Alice");
    }

    #[test]
    fn greets_world() {
        assert_eq!(hello("world"), "hello, world");
    }
}
RUST_EOF

# --- Justfile ---
cat > Justfile << 'JUST_EOF'
check:
    cargo test

install:
    cargo build --release
JUST_EOF

# --- .gitignore ---
cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

# --- Compile to verify skeleton ---
cargo check

# --- Initial commit ---
jj describe -m "greeter: minimal Rust project with todo!() stub"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Initialize with botbox
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name greeter --type cli --tools bones,maw,crit,botbus,vessel \
    --language rust --no-seed-work --no-interactive

# Safety net: ensure bones is initialized
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" maw exec default -- bn init 2>/dev/null || true

# ============================================================
# Patch .botbox.json: disable review, configure dev-loop
# ============================================================
cd "$PROJECT_DIR"
CONFIG_FILE=".botbox.json"
EVAL_MODEL="${EVAL_MODEL:-sonnet}"
PATCHED=$(jq \
  --arg model "$EVAL_MODEL" \
  '
  .review.enabled = false |
  .project.checkCommand = "cargo test" |
  .agents.dev.model = $model |
  .agents.dev.timeout = 300 |
  .agents.dev.maxLoops = 3 |
  .agents.dev.pause = 1 |
  .agents.worker.model = $model |
  .agents.worker.timeout = 300 |
  .pushMain = false
' "$CONFIG_FILE")
echo "$PATCHED" > "$CONFIG_FILE"

echo "Patched .botbox.json:"
jq '{review: .review, agents: {dev: .agents.dev}, pushMain: .pushMain}' "$CONFIG_FILE"

# ============================================================
# Fix hooks: add BOTBUS_DATA_DIR to --env-inherit
# ============================================================
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
# Create bone (as if a human filed it)
# ============================================================
cd "$PROJECT_DIR"
BONE_OUTPUT=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT="$LEAD_AGENT" maw exec default -- bn create \
  --title "Implement greet::hello function" \
  --description "Replace the todo!() in src/greet.rs with a working implementation. The function should return \"hello, {name}\" for any given name. Two tests are already provided — make them pass." \
  --kind task 2>&1)

BONE_ID=$(echo "$BONE_OUTPUT" | grep -oP 'bn-[a-z0-9]+' | head -1)
if [[ -z "$BONE_ID" ]]; then
  echo "FATAL: Could not create bone"
  echo "$BONE_OUTPUT"
  exit 1
fi
echo "Created bone: $BONE_ID"

# Mark doing (worker started it)
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT="$WORKER_AGENT" maw exec default -- bn do "$BONE_ID"

# ============================================================
# Create worker workspace and implement the code
# ============================================================
WORKER_WS="storm-reef"
maw ws create "$WORKER_WS"
echo "Created workspace: $WORKER_WS"

# Implement greet.rs in the workspace
cat > "$PROJECT_DIR/ws/$WORKER_WS/src/greet.rs" << 'RUST_EOF'
/// Return a greeting string for the given name.
pub fn hello(name: &str) -> String {
    format!("hello, {}", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_name() {
        assert_eq!(hello("Alice"), "hello, Alice");
    }

    #[test]
    fn greets_world() {
        assert_eq!(hello("world"), "hello, world");
    }
}
RUST_EOF

# Verify tests pass in workspace
echo "Running tests in workspace..."
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" maw exec "$WORKER_WS" -- cargo test 2>&1 | tail -3

# Describe the commit
maw exec "$WORKER_WS" -- jj describe -m "feat: implement greet::hello function"

# ============================================================
# Complete the bone (worker finished)
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT="$WORKER_AGENT" maw exec default -- bn bone comment add "$BONE_ID" \
  "Implemented greet::hello in workspace $WORKER_WS. Tests passing (2/2). Ready for merge."

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT="$WORKER_AGENT" maw exec default -- bn done "$BONE_ID" \
  --reason "Completed in workspace $WORKER_WS"

# ============================================================
# Stake claims (as the worker — lead will see these)
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus claims stake \
  --agent "$WORKER_AGENT" "bone://greeter/$BONE_ID" -m "$BONE_ID"

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus claims stake \
  --agent "$WORKER_AGENT" "workspace://greeter/$WORKER_WS" -m "$BONE_ID"

# ============================================================
# Send task-done message (worker announcing completion)
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$WORKER_AGENT" greeter \
  "Completed $BONE_ID: Implement greet::hello function in workspace $WORKER_WS. Tests pass. Ready for merge." \
  -L task-done

# Mark read for the worker so it doesn't re-appear
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$WORKER_AGENT" --channels greeter --mark-read >/dev/null 2>&1 || true

# ============================================================
# Projects registry
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$LEAD_AGENT" projects \
  "project: greeter  repo: $PROJECT_DIR  lead: $LEAD_AGENT  tools: cli"

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$LEAD_AGENT" --channels projects --mark-read >/dev/null 2>&1 || true

# ============================================================
# Verify pre-conditions
# ============================================================
echo ""
echo "--- Pre-condition verification ---"
echo "Bone state:"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" maw exec default -- bn show "$BONE_ID" 2>&1 | head -3
echo ""
echo "Workspaces:"
maw ws list 2>&1
echo ""
echo "Claims:"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus claims list 2>&1
echo ""
echo "Channel history:"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus history greeter -n 5 2>&1

# ============================================================
# Save env
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export LEAD_AGENT="$LEAD_AGENT"
export WORKER_AGENT="$WORKER_AGENT"
export BONE_ID="$BONE_ID"
export WORKER_WS="$WORKER_WS"
EOF

echo ""
echo "=== E12-Proto-Lead Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "LEAD_AGENT=$LEAD_AGENT"
echo "WORKER_AGENT=$WORKER_AGENT"
echo "BONE_ID=$BONE_ID"
echo "WORKER_WS=$WORKER_WS"
echo ""
echo "Scenario: Worker completed $BONE_ID in workspace $WORKER_WS."
echo "  - Code implemented, tests passing"
echo "  - Bone done, claims staked, task-done announced"
echo "  - Workspace left for lead to merge"
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
