#!/usr/bin/env bash
set -euo pipefail

# E12-Proto Eval — Run
# Spawns a worker-loop agent via vessel, polls for bone completion,
# captures artifacts (agent log, bus history, bone state, workspace state).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E12_TIMEOUT:-300}  # 5 minutes default
POLL_INTERVAL=15                      # seconds between status checks
STUCK_THRESHOLD=120                   # 2 minutes without progress = stuck

echo "=== E12-Proto Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e12-proto-setup.sh" 2>&1) || {
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

# --- Verify bone exists ---
echo "--- Verifying bone ---"
cd "$PROJECT_DIR"
BONE_STATUS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" 2>&1 || echo "ERROR")
echo "$BONE_STATUS" | head -3
echo "--- bone: OK ---"
echo ""

# --- Spawn worker ---
WORKER_NAME="eval-worker"
WORKER_AGENT="$AGENT_NAME/$WORKER_NAME"

echo "--- Spawning worker: $WORKER_NAME ---"
echo "  Agent: $WORKER_AGENT"
echo "  Bone: $BONE_ID"
echo "  Timeout: ${OVERALL_TIMEOUT}s"
echo ""

BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel spawn \
  -n "$WORKER_NAME" \
  --cwd "$PROJECT_DIR" \
  --env-inherit BOTBUS_DATA_DIR,SSH_AUTH_SOCK \
  -e "AGENT=$WORKER_AGENT" \
  -e "BOTBOX_BEAD=$BONE_ID" \
  -t "$OVERALL_TIMEOUT" \
  -- botbox run worker-loop --agent "$WORKER_AGENT"

echo "Worker spawned. Polling for completion..."
echo ""

# --- Poll loop ---
START_TIME=$(date +%s)
LAST_ACTIVITY_TIME=$START_TIME
FINAL_STATUS="unknown"

while true; do
  ELAPSED=$(( $(date +%s) - START_TIME ))

  if [[ $ELAPSED -ge $OVERALL_TIMEOUT ]]; then
    echo "TIMEOUT: Overall timeout reached (${OVERALL_TIMEOUT}s)"
    FINAL_STATUS="timeout"
    break
  fi

  sleep "$POLL_INTERVAL"
  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo "--- Poll (${ELAPSED}s / ${OVERALL_TIMEOUT}s) ---"

  # Check if worker is still running
  WORKER_RUNNING=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | \
    jq -r ".agents[] | select(.id == \"$WORKER_NAME\") | .id" 2>/dev/null || echo "")

  if [[ -n "$WORKER_RUNNING" ]]; then
    echo "  worker: RUNNING"
  else
    echo "  worker: EXITED"
    FINAL_STATUS="worker-exited"
  fi

  # Check bone state
  cd "$PROJECT_DIR"
  BONE_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" --format json 2>/dev/null || echo "[]")
  BONE_CUR_STATE=$(echo "$BONE_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
  echo "  bone $BONE_ID: $BONE_CUR_STATE"

  if [[ "$BONE_CUR_STATE" == "done" ]]; then
    echo "  Bone is DONE — worker completed!"
    FINAL_STATUS="completed"

    # Grace period for worker to exit
    for WAIT_I in 1 2 3; do
      sleep 5
      WR=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | \
        jq -r ".agents[] | select(.id == \"$WORKER_NAME\") | .id" 2>/dev/null || echo "")
      if [[ -z "$WR" ]]; then
        echo "  Worker exited cleanly."
        break
      fi
      echo "  Waiting for worker to exit... (${WAIT_I}/3)"
    done
    break
  fi

  # If worker exited but bone not done
  if [[ "$FINAL_STATUS" == "worker-exited" ]]; then
    echo "  Worker exited without completing bone (state=$BONE_CUR_STATE)"
    break
  fi

  # Check bus activity for progress
  MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history greeter -n 200 2>/dev/null | wc -l || echo "0")
  echo "  Channel messages: $MSG_COUNT"

  # Activity tracking
  if [[ "$BONE_CUR_STATE" == "doing" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
  fi

  # Stuck detection
  IDLE_TIME=$(( $(date +%s) - LAST_ACTIVITY_TIME ))
  if [[ $IDLE_TIME -ge $STUCK_THRESHOLD ]]; then
    echo "  WARNING: No progress for ${IDLE_TIME}s"
  fi
done

echo ""
echo "--- Final status: $FINAL_STATUS ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Worker agent log
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel tail "$WORKER_NAME" -n 500 > "$ARTIFACTS/agent-worker.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-worker.log"
echo "  log: $ARTIFACTS/agent-worker.log"

# Channel history
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history greeter -n 200 > "$ARTIFACTS/channel-history.log" 2>/dev/null || \
  echo "(no history)" > "$ARTIFACTS/channel-history.log"
echo "  channel: $ARTIFACTS/channel-history.log"

# Bone state
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" --format json > "$ARTIFACTS/bone-final.json" 2>/dev/null || \
  echo "[]" > "$ARTIFACTS/bone-final.json"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" > "$ARTIFACTS/bone-final.txt" 2>/dev/null || true
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn bone comment list "$BONE_ID" > "$ARTIFACTS/bone-comments.txt" 2>/dev/null || true
echo "  bone: $ARTIFACTS/bone-final.json"

# Workspace state
maw ws list --format json > "$ARTIFACTS/workspace-state.json" 2>/dev/null || \
  echo "{}" > "$ARTIFACTS/workspace-state.json"
echo "  workspaces: $ARTIFACTS/workspace-state.json"

# Claims state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list > "$ARTIFACTS/claims-final.txt" 2>/dev/null || \
  echo "(no claims)" > "$ARTIFACTS/claims-final.txt"
echo "  claims: $ARTIFACTS/claims-final.txt"

# Test output (try each workspace — worker may not have merged to default)
{
  for ws_name in $(maw ws list --format json 2>/dev/null | jq -r '.workspaces[].name' 2>/dev/null); do
    echo "=== Workspace: $ws_name ==="
    BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec "$ws_name" -- cargo test 2>&1 || true
    echo ""
  done
} > "$ARTIFACTS/test-output.txt"
echo "  tests: $ARTIFACTS/test-output.txt"

# Final status
cat > "$ARTIFACTS/final-status.txt" << EOF
FINAL_STATUS=$FINAL_STATUS
BONE_ID=$BONE_ID
BONE_STATUS=$BONE_CUR_STATE
WORKER_NAME=$WORKER_NAME
WORKER_AGENT=$WORKER_AGENT
EOF

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel kill "$WORKER_NAME" 2>/dev/null || true
for AGENT_ID in $(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel kill "$AGENT_ID" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e12-proto-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== E12-Proto Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status: $FINAL_STATUS"
echo "Bone: $BONE_ID ($BONE_CUR_STATE)"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  cat $ARTIFACTS/agent-worker.log"
echo "  cat $ARTIFACTS/channel-history.log"
echo ""
