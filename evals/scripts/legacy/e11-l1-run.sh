#!/usr/bin/env bash
set -euo pipefail

# E11-L1 Botty-Native End-to-End Eval — Orchestrator
# Runs setup, sends the task-request (which fires the hook → vessel spawns dev-loop),
# polls for completion, captures artifacts, and runs verification.
#
# KEY DIFFERENCE from E10: No sequential `claude -p` phases. Instead, we send one
# message and watch the system work autonomously through hooks and loop scripts.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E11_TIMEOUT:-900}  # 15 minutes default
POLL_INTERVAL=30                      # seconds between status checks
STUCK_THRESHOLD=300                   # 5 minutes without progress = stuck

echo "=== E11-L1 Botty-Native End-to-End Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e11-l1-setup.sh" 2>&1) || {
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

# --- Verify hooks are registered ---
echo "--- Verifying hooks ---"
HOOKS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list 2>&1)
echo "$HOOKS"
if echo "$HOOKS" | grep -q "echo"; then
  echo "--- hooks: OK ---"
else
  echo "WARNING: No echo hooks found — agent may not spawn"
fi
echo ""

# --- Send task-request (this triggers the hook → vessel spawn) ---
echo "--- Sending task-request ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup echo \
  "New task: Add GET /version endpoint returning JSON with name and version fields. The endpoint should return {\"name\":\"echo\",\"version\":\"0.1.0\"}. See bead $BEAD for details." \
  -L task-request
echo "Task-request sent. Hook should fire shortly."
echo ""

# --- Poll loop ---
echo "--- Polling for completion (timeout: ${OVERALL_TIMEOUT}s) ---"
START_TIME=$(date +%s)
LAST_BEAD_STATUS=""
LAST_ACTIVITY_TIME=$START_TIME
LAST_MSG_COUNT=0
FINAL_STATUS="timeout"

while true; do
  ELAPSED=$(( $(date +%s) - START_TIME ))

  # Check overall timeout
  if [[ $ELAPSED -ge $OVERALL_TIMEOUT ]]; then
    echo "TIMEOUT: Overall timeout reached (${OVERALL_TIMEOUT}s)"
    FINAL_STATUS="timeout"
    break
  fi

  sleep "$POLL_INTERVAL"
  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo ""
  echo "--- Poll (${ELAPSED}s / ${OVERALL_TIMEOUT}s) ---"

  # Check vessel — is an agent running?
  BOTTY_LIST=$(vessel list 2>/dev/null || echo "(vessel list failed)")
  echo "  Agents: $BOTTY_LIST"

  # Check bone status
  BEAD_JSON=$(cd "$PROJECT_DIR" && BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json 2>/dev/null || echo "[]")
  BEAD_STATUS=$(echo "$BEAD_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
  echo "  Bone $BEAD: $BEAD_STATUS"

  # Track activity for stuck detection
  if [[ "$BEAD_STATUS" != "$LAST_BEAD_STATUS" ]]; then
    LAST_BEAD_STATUS="$BEAD_STATUS"
    LAST_ACTIVITY_TIME=$(date +%s)
  fi

  # Check for new bus messages (activity indicator)
  # Count total messages — if count grows since last poll, there's activity
  CURRENT_MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history echo -n 100 2>/dev/null | wc -l || echo "0")
  if [[ "$CURRENT_MSG_COUNT" -gt "$LAST_MSG_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_MSG_COUNT=$CURRENT_MSG_COUNT
  fi

  # Check for completion
  if [[ "$BEAD_STATUS" == "done" ]]; then
    echo "  Bone is DONE — agent completed the task!"
    FINAL_STATUS="completed"
    # Give agent a moment to finish cleanup (release claims, sync, etc.)
    sleep 10
    break
  fi

  # Stuck detection
  IDLE_TIME=$(( $(date +%s) - LAST_ACTIVITY_TIME ))
  if [[ $IDLE_TIME -ge $STUCK_THRESHOLD ]]; then
    echo "  WARNING: No activity for ${IDLE_TIME}s (threshold: ${STUCK_THRESHOLD}s)"
    # Check if agent is still alive
    if ! vessel list 2>/dev/null | grep -q "echo"; then
      echo "  Agent exited without closing bone — marking as agent-exited"
      FINAL_STATUS="agent-exited"
      break
    fi
  fi
done

echo ""
echo "--- Final status: $FINAL_STATUS ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Agent log (may fail if agent already exited)
vessel tail "$ECHO_DEV" -n 500 > "$ARTIFACTS/agent-echo-dev.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-echo-dev.log"
echo "  agent log: $ARTIFACTS/agent-echo-dev.log"

# Also try to capture any dev-loop worker agents
for agent_name in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  if [[ "$agent_name" != "$ECHO_DEV" ]]; then
    vessel tail "$agent_name" -n 500 > "$ARTIFACTS/agent-${agent_name}.log" 2>/dev/null || true
    echo "  worker log: $ARTIFACTS/agent-${agent_name}.log"
  fi
done

# Channel history
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history echo -n 50 > "$ARTIFACTS/channel-history.log" 2>/dev/null || \
  echo "(no channel history)" > "$ARTIFACTS/channel-history.log"
echo "  channel history: $ARTIFACTS/channel-history.log"

# Bone state
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json > "$ARTIFACTS/bone-state.json" 2>/dev/null || \
  echo "[]" > "$ARTIFACTS/bone-state.json"
echo "  bone state: $ARTIFACTS/bone-state.json"

# Workspace state
cd "$PROJECT_DIR"
maw ws list --format json > "$ARTIFACTS/workspace-state.json" 2>/dev/null || \
  echo "{}" > "$ARTIFACTS/workspace-state.json"
echo "  workspace state: $ARTIFACTS/workspace-state.json"

# Claims state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list > "$ARTIFACTS/claims-state.txt" 2>/dev/null || \
  echo "(no claims)" > "$ARTIFACTS/claims-state.txt"
echo "  claims state: $ARTIFACTS/claims-state.txt"

# Save final status
echo "$FINAL_STATUS" > "$ARTIFACTS/final-status.txt"

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
vessel kill "$ECHO_DEV" 2>/dev/null || true
# Kill any worker agents that might still be running
for agent_name in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  vessel kill "$agent_name" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e11-l1-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== E11-L1 Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status: $FINAL_STATUS"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  ls $ARTIFACTS/"
echo "  cat $ARTIFACTS/agent-echo-dev.log"
echo "  cat $ARTIFACTS/channel-history.log"
