#!/usr/bin/env bash
set -euo pipefail

# E12-Proto Eval — Setup
# Creates a minimal Rust project with a single todo!() function,
# initializes botbox with review disabled, creates one bone for a worker to implement.
#
# Validates that agents USE protocol commands (start, review, finish, cleanup, resume)
# during a real worker-loop cycle and that the commands produce correct output.

# --- Preflight ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-/tmp/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e12-proto-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/greeter"
PROJECT_REMOTE="$EVAL_DIR/greeter-remote.git"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

AGENT_NAME="greeter-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e12-proto"
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

# --- src/greet.rs (the todo! stub the agent must implement) ---
cat > src/greet.rs << 'RUST_EOF'
/// Return a greeting string for the given name.
///
/// Examples:
///   hello("Alice") => "hello, Alice"
///   hello("world") => "hello, world"
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

# Safety net: ensure bones is initialized (botbox init may skip it in bare repo mode)
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" maw exec default -- bn init 2>/dev/null || true

# ============================================================
# Patch .botbox.json: disable review, set model
# ============================================================
cd "$PROJECT_DIR"
CONFIG_FILE=".botbox.json"
EVAL_MODEL="${EVAL_MODEL:-sonnet}"
PATCHED=$(jq \
  --arg model "$EVAL_MODEL" \
  '
  .review.enabled = false |
  .project.checkCommand = "cargo test" |
  .agents.worker.model = $model |
  .agents.worker.timeout = 300 |
  .pushMain = false
' "$CONFIG_FILE")
echo "$PATCHED" > "$CONFIG_FILE"

echo "Patched .botbox.json:"
jq '{review: .review, agents: {worker: .agents.worker}, pushMain: .pushMain}' "$CONFIG_FILE"

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
# Create the bone for the worker to implement
# ============================================================
cd "$PROJECT_DIR"
BONE_OUTPUT=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" AGENT="$AGENT_NAME" maw exec default -- bn create \
  --title "Implement greet::hello function" \
  --description "Replace the todo!() in src/greet.rs with a working implementation. The function should return \"hello, {name}\" for any given name. Two tests are already provided — make them pass. Run cargo test to verify." \
  --kind task 2>&1)

# Extract bone ID from output like "✓ Created bn-xxxx: ..."
BONE_ID=$(echo "$BONE_OUTPUT" | grep -oP 'bn-[a-z0-9]+' | head -1)
if [[ -z "$BONE_ID" ]]; then
  echo "FATAL: Could not create bone"
  echo "$BONE_OUTPUT"
  exit 1
fi
echo "Created bone: $BONE_ID"

# ============================================================
# Projects registry
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$AGENT_NAME" projects \
  "project: greeter  repo: $PROJECT_DIR  lead: $AGENT_NAME  tools: cli"

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$AGENT_NAME" --channels projects --mark-read >/dev/null 2>&1 || true

# ============================================================
# Save env
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export AGENT_NAME="$AGENT_NAME"
export BONE_ID="$BONE_ID"
EOF

echo ""
echo "=== E12-Proto Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "AGENT_NAME=$AGENT_NAME"
echo "BONE_ID=$BONE_ID"
echo ""
echo "Project: greeter — minimal Rust project with todo!() in greet.rs"
echo "Bone: $BONE_ID — Implement greet::hello function"
echo "Review: disabled"
echo "Worker model: $EVAL_MODEL"
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
