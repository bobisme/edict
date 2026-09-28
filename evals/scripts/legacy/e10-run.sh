#!/usr/bin/env bash
set -euo pipefail

# E10 Full Lifecycle Eval — Orchestrator
# Runs setup → phases 1-8 → verify in sequence.
# Reports failures without aborting remaining phases.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== E10 Full Lifecycle Eval ==="
echo "Starting at $(date)"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e10-setup.sh" 2>&1) || {
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

EVAL_ENV="$EVAL_DIR/.eval-env"
echo "--- setup: OK (EVAL_DIR=$EVAL_DIR) ---"
echo ""

# --- Run phases ---
FAILED=()
PHASES=(phase1 phase2 phase3 phase4 phase4_5 phase5 phase6 phase7 phase8)

for phase in "${PHASES[@]}"; do
  echo ""
  echo "--- Running $phase ($(date +%H:%M:%S)) ---"
  if "$SCRIPT_DIR/e10-$phase.sh" "$EVAL_ENV"; then
    echo "--- $phase: OK ---"
  else
    RC=$?
    echo "--- $phase: FAILED (exit $RC) ---"
    FAILED+=("$phase")
  fi
done

# --- Verification ---
echo ""
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e10-verify.sh" "$EVAL_ENV" || FAILED+=("verify")

# --- Friction Analysis ---
echo ""
echo "--- Running friction analysis ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e10-friction.sh" "$EVAL_ENV" || echo "WARNING: friction analysis failed"

# --- Summary ---
echo ""
echo "========================================="
echo "=== E10 Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""

if [[ ${#FAILED[@]} -eq 0 ]]; then
  echo "All phases completed successfully."
else
  echo "Failed phases: ${FAILED[*]}"
fi

echo ""
echo "Artifacts: $EVAL_DIR/artifacts/"
echo "EVAL_DIR=$EVAL_DIR"
echo "EVAL_ENV=$EVAL_ENV"
echo ""
echo "To inspect results:"
echo "  source $EVAL_ENV"
echo "  ls $EVAL_DIR/artifacts/"
echo "  cat $EVAL_DIR/artifacts/phase1.stdout.log"
