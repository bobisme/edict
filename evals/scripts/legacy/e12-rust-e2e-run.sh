#!/usr/bin/env bash
set -euo pipefail

# E12 Rust E2E Eval — Runner
# Exercises every Rust botbox command against a real project:
#   Phase 1: botbox init (non-interactive)
#   Phase 2: botbox sync + sync --check
#   Phase 3: botbox doctor + doctor --strict
#   Phase 4: botbox status (text, json)
#   Phase 5: botbox hooks (install, audit)
#   Phase 6: botbox run (--help for each subcommand)
#   Phase 7: Re-init resilience (init --force on existing project)
#
# Captures all outputs as artifacts for the verify script.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== E12 Rust E2E Eval ==="
echo "Starting at $(date)"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e12-rust-e2e-setup.sh" 2>&1) || {
  echo "$SETUP_OUTPUT"
  echo "FATAL: Setup failed"
  exit 1
}
echo "$SETUP_OUTPUT"

EVAL_DIR=$(echo "$SETUP_OUTPUT" | grep -oP 'EVAL_DIR=\K.*' | head -1)
if [[ -z "$EVAL_DIR" || ! -f "$EVAL_DIR/.eval-env" ]]; then
  echo "FATAL: Setup completed but could not find .eval-env"
  echo "Output was:"
  echo "$SETUP_OUTPUT"
  exit 1
fi

source "$EVAL_DIR/.eval-env"
ARTIFACTS="$EVAL_DIR/artifacts"
echo "--- setup: OK (EVAL_DIR=$EVAL_DIR) ---"
echo ""

# Helper: run a command, capture output, report pass/fail
run_phase() {
  local phase_name="$1"
  local artifact_name="$2"
  shift 2
  echo "--- Phase: $phase_name ---"
  local rc=0
  local output
  output=$("$@" 2>&1) || rc=$?
  echo "$output" > "$ARTIFACTS/${artifact_name}.txt"
  echo "  exit code: $rc"
  if [[ $rc -eq 0 ]]; then
    echo "  PASS"
  else
    echo "  FAIL (exit code $rc)"
    # Show first 20 lines of output for debugging
    echo "$output" | head -20
  fi
  echo ""
  return $rc
}

cd "$PROJECT_DIR"

# ============================================================
# Phase 1: botbox init
# ============================================================
echo "=============================="
echo "=== Phase 1: botbox init ==="
echo "=============================="
echo ""

INIT_RC=0
INIT_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" init \
  --name testproj \
  --type cli \
  --tools bones,maw,crit,botbus,vessel \
  --no-interactive \
  --no-seed-work \
  --language rust \
  --install-command "cargo build" \
  2>&1) || INIT_RC=$?

echo "$INIT_OUTPUT" > "$ARTIFACTS/phase1-init.txt"
echo "$INIT_OUTPUT"
echo ""
echo "Init exit code: $INIT_RC"
echo ""

if [[ $INIT_RC -ne 0 ]]; then
  echo "FATAL: botbox init failed — cannot continue"
  echo "EVAL_DIR=$EVAL_DIR"
  exit 1
fi

# Record post-init state
ls -la "$PROJECT_DIR/" > "$ARTIFACTS/phase1-project-ls.txt" 2>&1 || true
ls -la "$PROJECT_DIR/.agents/botbox/" > "$ARTIFACTS/phase1-agents-ls.txt" 2>&1 || true
cat "$PROJECT_DIR/.botbox.json" > "$ARTIFACTS/phase1-config.json" 2>&1 || true
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list > "$ARTIFACTS/phase1-hooks.txt" 2>&1 || true
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list --format json > "$ARTIFACTS/phase1-hooks.json" 2>&1 || true

# Check if maw init was done (ws/default/ should exist)
if [[ -d "$PROJECT_DIR/ws/default" ]]; then
  echo "maw v2 layout: ws/default/ exists"
  ls "$PROJECT_DIR/ws/default/" > "$ARTIFACTS/phase1-ws-default-ls.txt" 2>&1 || true
else
  echo "WARNING: ws/default/ does not exist — maw v2 layout not created"
fi

echo "--- Phase 1 complete ---"
echo ""

# ============================================================
# Phase 2: botbox sync + sync --check
# ============================================================
echo "=============================="
echo "=== Phase 2: botbox sync ==="
echo "=============================="
echo ""

# First sync (should be a no-op since init just ran)
run_phase "sync (first)" "phase2-sync-first" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" sync --no-commit || true

# Sync --check (should exit 0 since everything is up to date)
SYNC_CHECK_RC=0
SYNC_CHECK_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" sync --check 2>&1) || SYNC_CHECK_RC=$?
echo "$SYNC_CHECK_OUTPUT" > "$ARTIFACTS/phase2-sync-check.txt"
echo "sync --check exit code: $SYNC_CHECK_RC"
if [[ $SYNC_CHECK_RC -eq 0 ]]; then
  echo "  sync --check: PASS (up to date)"
else
  echo "  sync --check: exit $SYNC_CHECK_RC (may indicate staleness)"
fi
echo ""

# Second sync (verify idempotent)
run_phase "sync (second)" "phase2-sync-second" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" sync --no-commit || true

echo "--- Phase 2 complete ---"
echo ""

# ============================================================
# Phase 3: botbox doctor
# ============================================================
echo "=============================="
echo "=== Phase 3: botbox doctor ==="
echo "=============================="
echo ""

# Basic doctor
run_phase "doctor" "phase3-doctor" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" doctor || true

# Doctor --strict
run_phase "doctor --strict" "phase3-doctor-strict" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" doctor --strict || true

# Doctor --format json
DOCTOR_JSON_RC=0
DOCTOR_JSON_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" doctor --format json 2>&1) || DOCTOR_JSON_RC=$?
echo "$DOCTOR_JSON_OUTPUT" > "$ARTIFACTS/phase3-doctor-json.txt"
echo "doctor --format json exit code: $DOCTOR_JSON_RC"
# Verify it's valid JSON
if echo "$DOCTOR_JSON_OUTPUT" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null; then
  echo "  Valid JSON output"
else
  echo "  WARNING: Invalid JSON output"
fi
echo ""

echo "--- Phase 3 complete ---"
echo ""

# ============================================================
# Phase 4: botbox status
# ============================================================
echo "=============================="
echo "=== Phase 4: botbox status ==="
echo "=============================="
echo ""

# Status (text)
run_phase "status" "phase4-status" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" status || true

# Status --format json
STATUS_JSON_RC=0
STATUS_JSON_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" status --format json 2>&1) || STATUS_JSON_RC=$?
echo "$STATUS_JSON_OUTPUT" > "$ARTIFACTS/phase4-status-json.txt"
echo "status --format json exit code: $STATUS_JSON_RC"
if echo "$STATUS_JSON_OUTPUT" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null; then
  echo "  Valid JSON output"
else
  echo "  WARNING: Invalid JSON output"
fi
echo ""

echo "--- Phase 4 complete ---"
echo ""

# ============================================================
# Phase 5: botbox hooks
# ============================================================
echo "=============================="
echo "=== Phase 5: botbox hooks ==="
echo "=============================="
echo ""

# Hooks audit
run_phase "hooks audit" "phase5-hooks-audit" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" hooks audit || true

# Hooks install (re-install — should be idempotent)
run_phase "hooks install" "phase5-hooks-install" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" hooks install || true

# Verify hooks are still registered after install
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list > "$ARTIFACTS/phase5-hooks-after.txt" 2>&1 || true
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list --format json > "$ARTIFACTS/phase5-hooks-after.json" 2>&1 || true

echo "--- Phase 5 complete ---"
echo ""

# ============================================================
# Phase 6: botbox run (--help checks)
# ============================================================
echo "=============================="
echo "=== Phase 6: botbox run ==="
echo "=============================="
echo ""

# Each run subcommand should have --help
for subcmd in agent dev-loop worker-loop reviewer-loop responder triage iteration-start; do
  echo "--- run $subcmd --help ---"
  HELP_RC=0
  HELP_OUTPUT=$("$RUST_BINARY" run "$subcmd" --help 2>&1) || HELP_RC=$?
  echo "$HELP_OUTPUT" > "$ARTIFACTS/phase6-run-${subcmd}-help.txt"
  if [[ $HELP_RC -eq 0 ]]; then
    echo "  PASS"
  else
    echo "  FAIL (exit code $HELP_RC)"
  fi
done
echo ""

echo "--- Phase 6 complete ---"
echo ""

# ============================================================
# Phase 7: Re-init resilience
# ============================================================
echo "=============================="
echo "=== Phase 7: Re-init ==="
echo "=============================="
echo ""

# Init --force on existing project (should not fail)
REINIT_RC=0
REINIT_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" init \
  --name testproj \
  --type cli \
  --tools bones,maw,crit,botbus,vessel \
  --no-interactive \
  --no-seed-work \
  --force \
  --no-commit \
  2>&1) || REINIT_RC=$?

echo "$REINIT_OUTPUT" > "$ARTIFACTS/phase7-reinit.txt"
echo "Re-init exit code: $REINIT_RC"
if [[ $REINIT_RC -eq 0 ]]; then
  echo "  PASS"
else
  echo "  FAIL"
  echo "$REINIT_OUTPUT" | head -20
fi
echo ""

# Doctor after re-init (should still pass)
run_phase "doctor (post-reinit)" "phase7-doctor-post-reinit" \
  env BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" "$RUST_BINARY" doctor || true

echo "--- Phase 7 complete ---"
echo ""

# ============================================================
# Final artifact capture
# ============================================================
echo "=== Capturing final state ==="

# Final project structure
find "$PROJECT_DIR" -maxdepth 3 -not -path '*/target/*' -not -path '*/.jj/*' -not -path '*/.git/*' -not -path '*/.bones/objects/*' | sort > "$ARTIFACTS/final-project-tree.txt" 2>&1 || true

# Final config
cat "$PROJECT_DIR/.botbox.json" > "$ARTIFACTS/final-config.json" 2>&1 || true

# Final hooks
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list --format json > "$ARTIFACTS/final-hooks.json" 2>&1 || true

# AGENTS.md exists and has content
if [[ -f "$PROJECT_DIR/AGENTS.md" ]] || [[ -f "$PROJECT_DIR/ws/default/AGENTS.md" ]]; then
  AGENTS_MD_PATH="$PROJECT_DIR/AGENTS.md"
  [[ -f "$PROJECT_DIR/ws/default/AGENTS.md" ]] && AGENTS_MD_PATH="$PROJECT_DIR/ws/default/AGENTS.md"
  wc -l "$AGENTS_MD_PATH" > "$ARTIFACTS/final-agentsmd-wc.txt" 2>&1 || true
  head -20 "$AGENTS_MD_PATH" > "$ARTIFACTS/final-agentsmd-head.txt" 2>&1 || true
fi

echo "Artifacts saved to: $ARTIFACTS/"
echo ""

# ============================================================
# Summary
# ============================================================
echo "========================================="
echo "=== E12 Rust E2E Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "RUST_BINARY=$RUST_BINARY"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo ""
echo "To verify:"
echo "  $SCRIPT_DIR/e12-rust-e2e-verify.sh $EVAL_DIR/.eval-env"
echo ""
echo "To inspect:"
echo "  ls $ARTIFACTS/"
echo "  cat $ARTIFACTS/phase1-init.txt"
echo "  cat $ARTIFACTS/phase3-doctor-strict.txt"
echo ""
