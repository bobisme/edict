#!/usr/bin/env bash
set -euo pipefail

# E12 Rust E2E Eval — Setup
# Creates a test project using the Rust botbox binary, verifying the full
# init → sync → doctor → status lifecycle works end-to-end.
#
# This eval tests the Rust rewrite of botbox — the same commands that were
# previously implemented in JavaScript. It does NOT test agent loops or
# mission orchestration (those are covered by e10/e11).
#
# Tests:
#   1. botbox init  — creates project structure, .botbox.json, AGENTS.md, hooks
#   2. botbox sync  — updates docs, scripts, hooks; --check mode detects staleness
#   3. botbox doctor — validates config and companion tools; --strict mode
#   4. botbox status — shows project status (beads, workspaces, inbox, agents, claims)
#   5. botbox hooks  — install, audit
#   6. botbox run    — verify subcommands are registered (--help check only)
#
# Does NOT launch real agents or send bus messages for agent spawning.

# --- Preflight ---
REQUIRED_CMDS=(bus bn maw crit vessel jj jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# The Rust binary must be built — find it
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BOTBOX_PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUST_BINARY="$BOTBOX_PROJECT_ROOT/target/debug/botbox"

if [[ ! -x "$RUST_BINARY" ]]; then
  echo "Rust binary not found at $RUST_BINARY"
  echo "Building..."
  (cd "$BOTBOX_PROJECT_ROOT" && cargo build 2>&1) || { echo "FATAL: cargo build failed" >&2; exit 1; }
fi

if [[ ! -x "$RUST_BINARY" ]]; then
  echo "FATAL: Rust binary still not found after build" >&2
  exit 1
fi

echo "Using Rust binary: $RUST_BINARY"
echo "Binary version: $("$RUST_BINARY" --version 2>/dev/null || echo unknown)"

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e12-rust-e2e-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/testproj"
PROJECT_REMOTE="$EVAL_DIR/testproj-remote.git"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

TEST_AGENT="testproj-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  echo "botbox_rust=$("$RUST_BINARY" --version 2>/dev/null || echo unknown)"
  for cmd in bus bn maw crit vessel jj; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e12-rust-e2e"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# ============================================================
# Fake git remote (so maw push / jj git push succeed)
# ============================================================
git init --bare "$PROJECT_REMOTE"

# ============================================================
# Create a minimal project skeleton
# ============================================================
cd "$PROJECT_DIR"
jj git init
jj git remote add origin "$PROJECT_REMOTE"

mkdir -p src

# Simple Rust project to give botbox something to work with
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "testproj"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "testproj"
path = "src/main.rs"
CARGO_EOF

cat > src/main.rs << 'RUST_EOF'
fn main() {
    println!("Hello from testproj!");
}
RUST_EOF

cat > .gitignore << 'EOF'
/target/
EOF

# Initial commit
jj describe -m "testproj: initial skeleton"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Save env (before init — init will be tested in the run script)
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export PROJECT_REMOTE="$PROJECT_REMOTE"
export TEST_AGENT="$TEST_AGENT"
export RUST_BINARY="$RUST_BINARY"
EOF

echo ""
echo "=== E12 Rust E2E Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "RUST_BINARY=$RUST_BINARY"
echo "TEST_AGENT=$TEST_AGENT"
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
