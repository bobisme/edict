#!/usr/bin/env bash
set -euo pipefail

# R4 Integration Eval — Setup
# Creates a fresh eval environment with a seeded bone for the dev agent to find.

EVAL_DIR=$(mktemp -d)
cd "$EVAL_DIR"
echo "EVAL_DIR=$EVAL_DIR"

# --- Init repo and botbox ---
jj git init
botbox init --name r4-eval --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Init Rust project ---
cargo init --name r4-eval

# --- Init crit and maw ---
crit init
maw init

# --- Generate agent identities ---
DEV_AGENT=$(botbus generate-name)
REVIEWER=$(botbus generate-name)
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"

# --- Create channel and register agents ---
botbus send --agent setup r4-eval "R4 eval environment initialized" -L mesh -L setup
botbus mark-read --agent "$DEV_AGENT" r4-eval
botbus mark-read --agent "$REVIEWER" r4-eval

# --- Create the task bone ---
bn create --title "Add file serving endpoint at GET /files/:name" \
  --description "Create a GET /files/:name endpoint that reads files from ./data and returns contents. Return 404 if not found, 500 on read errors." \
  --kind task

# --- Seed test data ---
mkdir -p data
echo "Hello from test file" > data/test.txt

# --- Commit baseline ---
jj describe -m "initial project setup"
jj new

# --- Save env for phases ---
cat > .eval-env << EOF
export EVAL_DIR="$EVAL_DIR"
export DEV_AGENT="$DEV_AGENT"
export REVIEWER="$REVIEWER"
EOF

echo ""
echo "=== R4 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo ""
echo "Source .eval-env before running phases:"
echo "  cd $EVAL_DIR && source .eval-env"
