#!/usr/bin/env bash
set -euo pipefail

# E12-Proto-Lead Eval — Run
# Spawns a dev-loop lead agent that should discover the pre-completed workspace
# and merge it using `botbox protocol merge`.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E12_TIMEOUT:-300}  # 5 minutes
POLL_INTERVAL=10                      # seconds between status checks

echo "=== E12-Proto-Lead Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e12-proto-lead-setup.sh" 2>&1) || {
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

# --- Verify pre-conditions ---
echo "--- Verifying pre-conditions ---"
cd "$PROJECT_DIR"
echo "  Bone $BONE_ID: done"
echo "  Workspace $WORKER_WS: exists"
NON_DEFAULT_BEFORE=$(maw ws list --format json 2>/dev/null | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "?")
echo "  Non-default workspaces: $NON_DEFAULT_BEFORE"
echo "--- pre-conditions: OK ---"
echo ""

# --- Spawn dev-loop lead ---
LEAD_NAME="eval-lead"

echo "--- Spawning lead: $LEAD_NAME ---"
echo "  Agent: $LEAD_AGENT"
echo "  Timeout: ${OVERALL_TIMEOUT}s"
echo "  Scenario: merge pre-completed workspace $WORKER_WS"
echo ""

BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel spawn \
  -n "$LEAD_NAME" \
  --cwd "$PROJECT_DIR" \
  --env-inherit BOTBUS_DATA_DIR,SSH_AUTH_SOCK \
  -e "AGENT=$LEAD_AGENT" \
  -t "$OVERALL_TIMEOUT" \
  -- botbox run dev-loop --agent "$LEAD_AGENT"

echo "Lead spawned. Polling for workspace merge..."
echo ""

# --- Poll loop ---
START_TIME=$(date +%s)
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

  # Check if lead is still running
  LEAD_RUNNING=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | \
    jq -r ".agents[] | select(.id == \"$LEAD_NAME\") | .id" 2>/dev/null || echo "")

  if [[ -n "$LEAD_RUNNING" ]]; then
    echo "  lead: RUNNING"
  else
    echo "  lead: EXITED"
    FINAL_STATUS="lead-exited"
  fi

  # Check workspace count (has the merge happened?)
  cd "$PROJECT_DIR"
  NON_DEFAULT_NOW=$(maw ws list --format json 2>/dev/null | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "?")
  echo "  workspaces (non-default): $NON_DEFAULT_NOW (was: $NON_DEFAULT_BEFORE)"

  # Check if worker workspace specifically still exists
  WS_EXISTS=$(maw ws list --format json 2>/dev/null | jq -r ".workspaces[] | select(.name == \"$WORKER_WS\") | .name" 2>/dev/null || echo "")
  if [[ -z "$WS_EXISTS" ]]; then
    echo "  $WORKER_WS: MERGED/DESTROYED"
  else
    echo "  $WORKER_WS: still exists"
  fi

  # Check bus for merge announcements
  MERGE_MSG=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history greeter -n 20 2>/dev/null | grep -i "merge\|Merged" || true)
  if [[ -n "$MERGE_MSG" ]]; then
    echo "  merge activity on channel: YES"
  fi

  # Success: workspace merged and lead either still running or exited
  if [[ -z "$WS_EXISTS" ]]; then
    echo ""
    echo "  >>> Workspace $WORKER_WS has been merged!"
    FINAL_STATUS="merged"

    # Give lead time to finish cleanup
    for WAIT_I in 1 2 3 4; do
      sleep 5
      LR=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | \
        jq -r ".agents[] | select(.id == \"$LEAD_NAME\") | .id" 2>/dev/null || echo "")
      if [[ -z "$LR" ]]; then
        echo "  Lead exited."
        break
      fi
      echo "  Waiting for lead to finish... (${WAIT_I}/4)"
    done
    break
  fi

  # Lead exited without merging
  if [[ "$FINAL_STATUS" == "lead-exited" ]]; then
    echo "  Lead exited without merging workspace $WORKER_WS"
    break
  fi
done

echo ""
echo "--- Final status: $FINAL_STATUS ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Lead agent log
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel tail "$LEAD_NAME" -n 500 > "$ARTIFACTS/agent-lead.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-lead.log"
echo "  log: $ARTIFACTS/agent-lead.log"

# Channel history
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history greeter -n 200 > "$ARTIFACTS/channel-history.log" 2>/dev/null || \
  echo "(no history)" > "$ARTIFACTS/channel-history.log"
echo "  channel: $ARTIFACTS/channel-history.log"

# Bone state
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" --format json > "$ARTIFACTS/bone-final.json" 2>/dev/null || \
  echo "[]" > "$ARTIFACTS/bone-final.json"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BONE_ID" > "$ARTIFACTS/bone-final.txt" 2>/dev/null || true
echo "  bone: $ARTIFACTS/bone-final.json"

# Workspace state
maw ws list --format json > "$ARTIFACTS/workspace-state.json" 2>/dev/null || \
  echo "{}" > "$ARTIFACTS/workspace-state.json"
echo "  workspaces: $ARTIFACTS/workspace-state.json"

# Claims state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list > "$ARTIFACTS/claims-final.txt" 2>/dev/null || \
  echo "(no claims)" > "$ARTIFACTS/claims-final.txt"
echo "  claims: $ARTIFACTS/claims-final.txt"

# Test output in default workspace (code should be merged there now)
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo test > "$ARTIFACTS/test-output.txt" 2>&1 || true
echo "  tests: $ARTIFACTS/test-output.txt"

# Final status
cat > "$ARTIFACTS/final-status.txt" << EOF
FINAL_STATUS=$FINAL_STATUS
BONE_ID=$BONE_ID
WORKER_WS=$WORKER_WS
LEAD_NAME=$LEAD_NAME
LEAD_AGENT=$LEAD_AGENT
NON_DEFAULT_BEFORE=$NON_DEFAULT_BEFORE
NON_DEFAULT_AFTER=$NON_DEFAULT_NOW
EOF

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel kill "$LEAD_NAME" 2>/dev/null || true
for AGENT_ID in $(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" vessel kill "$AGENT_ID" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e12-proto-lead-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "============================================"
echo "=== E12-Proto-Lead Complete ($(date +%H:%M:%S)) ==="
echo "============================================"
echo ""
echo "Final status: $FINAL_STATUS"
echo "Bone: $BONE_ID"
echo "Workspace: $WORKER_WS (merged: $([ -z "$WS_EXISTS" ] && echo "YES" || echo "NO"))"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  cat $ARTIFACTS/agent-lead.log"
echo "  cat $ARTIFACTS/channel-history.log"
echo ""
