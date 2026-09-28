#!/usr/bin/env bash
set -euo pipefail

# E12-Proto-Review Eval — Run
# Spawns a worker-loop agent, waits for review creation, auto-LGTMs it,
# then waits for bone completion. Captures artifacts for verification.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E12_TIMEOUT:-360}  # 6 minutes (extra time for review round-trip)
POLL_INTERVAL=10                      # seconds between status checks
STUCK_THRESHOLD=120                   # 2 minutes without progress = stuck

echo "=== E12-Proto-Review Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e12-proto-review-setup.sh" 2>&1) || {
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
echo "  Review: ENABLED"
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

# --- State tracking ---
START_TIME=$(date +%s)
LAST_ACTIVITY_TIME=$START_TIME
FINAL_STATUS="unknown"
REVIEW_LGTM_DONE=false
REVIEW_ID_FOUND=""
REVIEW_WS_FOUND=""
WORKER_RESPAWNED=false

# --- Helper: find review in non-default workspaces ---
_find_review() {
  cd "$PROJECT_DIR"
  for ws_name in $(maw ws list --format json 2>/dev/null | jq -r '.workspaces[] | select(.is_default == false) | .name' 2>/dev/null); do
    # Try crit reviews list in this workspace
    REVIEW_LIST=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec "$ws_name" -- crit reviews list --format json 2>/dev/null || echo '{"reviews":[]}')
    RID=$(echo "$REVIEW_LIST" | jq -r '.reviews[0].review_id // empty' 2>/dev/null || echo "")
    if [[ -n "$RID" ]]; then
      REVIEW_ID_FOUND="$RID"
      REVIEW_WS_FOUND="$ws_name"
      return 0
    fi
  done
  return 1
}

# --- Poll loop ---
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

  # Check bus for review-request messages
  REVIEW_MSG=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history greeter -n 50 2>/dev/null | grep -i "review" || true)
  if [[ -n "$REVIEW_MSG" ]]; then
    echo "  review activity on channel: YES"
  fi

  # --- Auto-LGTM: find and approve review ---
  if [[ "$REVIEW_LGTM_DONE" == "false" ]] && [[ "$BONE_CUR_STATE" == "doing" ]]; then
    if _find_review; then
      echo ""
      echo "  >>> REVIEW FOUND: $REVIEW_ID_FOUND in workspace $REVIEW_WS_FOUND"
      echo "  >>> Auto-LGTMing as greeter-security..."
      echo ""

      # LGTM the review
      LGTM_OUTPUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec "$REVIEW_WS_FOUND" -- \
        crit lgtm "$REVIEW_ID_FOUND" -m "Eval auto-approve: implementation looks correct." \
        --agent "greeter-security" 2>&1 || echo "LGTM_ERROR")
      echo "  >>> LGTM result: $LGTM_OUTPUT"

      # Announce review done on bus (worker may be waiting for this)
      BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent "greeter-security" greeter \
        "Review $REVIEW_ID_FOUND: LGTM @$WORKER_AGENT" -L review-done 2>/dev/null || true

      REVIEW_LGTM_DONE=true
      echo "  >>> Review LGTM'd. Worker should pick up on next iteration."
      echo ""

      # Save review info for verify
      echo "REVIEW_ID=$REVIEW_ID_FOUND" >> "$EVAL_DIR/.eval-env"
      echo "REVIEW_WS=$REVIEW_WS_FOUND" >> "$EVAL_DIR/.eval-env"
    fi
  fi

  if [[ "$REVIEW_LGTM_DONE" == "true" ]]; then
    echo "  review: LGTM'd ($REVIEW_ID_FOUND)"
  else
    echo "  review: waiting..."
  fi

  # Check for bone done
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

  # If worker exited but bone not done — re-spawn for iteration 2 after LGTM
  if [[ "$FINAL_STATUS" == "worker-exited" ]]; then
    if [[ "$REVIEW_LGTM_DONE" == "true" ]] && [[ "$WORKER_RESPAWNED" == "false" ]]; then
      echo ""
      echo "  >>> Worker exited after review request. LGTM is applied."
      echo "  >>> Capturing iteration 1 log before re-spawning..."

      # Save iteration 1 log (vessel tail will be lost after re-spawn with same name)
      BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel tail "$WORKER_NAME" -n 500 > "$ARTIFACTS/agent-worker-iter1.log" 2>/dev/null || true

      echo "  >>> Re-spawning worker for iteration 2 (finish flow)..."
      echo ""

      REMAINING=$(( OVERALL_TIMEOUT - ELAPSED ))
      BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel spawn \
        -n "$WORKER_NAME" \
        --cwd "$PROJECT_DIR" \
        --env-inherit BOTBUS_DATA_DIR,SSH_AUTH_SOCK \
        -e "AGENT=$WORKER_AGENT" \
        -e "BOTBOX_BEAD=$BONE_ID" \
        -t "$REMAINING" \
        -- botbox run worker-loop --agent "$WORKER_AGENT"

      WORKER_RESPAWNED=true
      FINAL_STATUS="unknown"
      echo "  >>> Worker re-spawned (timeout: ${REMAINING}s)"
      continue
    fi

    echo "  Worker exited without completing bone (state=$BONE_CUR_STATE)"
    break
  fi

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

# Worker agent log (concatenate iteration 1 + iteration 2 if both exist)
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel tail "$WORKER_NAME" -n 500 > "$ARTIFACTS/agent-worker-iter2.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-worker-iter2.log"
if [[ -f "$ARTIFACTS/agent-worker-iter1.log" ]]; then
  {
    echo "=== ITERATION 1 ==="
    cat "$ARTIFACTS/agent-worker-iter1.log"
    echo ""
    echo "=== ITERATION 2 ==="
    cat "$ARTIFACTS/agent-worker-iter2.log"
  } > "$ARTIFACTS/agent-worker.log"
else
  cp "$ARTIFACTS/agent-worker-iter2.log" "$ARTIFACTS/agent-worker.log"
fi
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

# Review state (capture review details if we found one)
if [[ -n "$REVIEW_ID_FOUND" && -n "$REVIEW_WS_FOUND" ]]; then
  # Try to capture review — workspace may be destroyed by now
  BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec "$REVIEW_WS_FOUND" -- \
    crit review "$REVIEW_ID_FOUND" --format json > "$ARTIFACTS/review-final.json" 2>/dev/null || \
    echo '{"error":"workspace destroyed"}' > "$ARTIFACTS/review-final.json"
  echo "  review: $ARTIFACTS/review-final.json"
else
  echo '{"error":"no review found"}' > "$ARTIFACTS/review-final.json"
  echo "  review: NO REVIEW FOUND"
fi

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
REVIEW_LGTM_DONE=$REVIEW_LGTM_DONE
REVIEW_ID=$REVIEW_ID_FOUND
REVIEW_WS=$REVIEW_WS_FOUND
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
"$SCRIPT_DIR/e12-proto-review-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "============================================="
echo "=== E12-Proto-Review Complete ($(date +%H:%M:%S)) ==="
echo "============================================="
echo ""
echo "Final status: $FINAL_STATUS"
echo "Bone: $BONE_ID ($BONE_CUR_STATE)"
echo "Review: $REVIEW_ID_FOUND (LGTM: $REVIEW_LGTM_DONE)"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  cat $ARTIFACTS/agent-worker.log"
echo "  cat $ARTIFACTS/channel-history.log"
echo "  cat $ARTIFACTS/review-final.json"
echo ""
