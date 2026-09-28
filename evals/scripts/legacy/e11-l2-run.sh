#!/usr/bin/env bash
set -euo pipefail

# E11-L2 Botty-Native Review Cycle Eval — Orchestrator
# Runs setup, sends the task-request (which fires the router hook → dev-loop spawns),
# polls for both dev and reviewer agents, tracks bead status AND review status,
# captures artifacts, and runs verification.
#
# KEY DIFFERENCE from L1: Must track TWO agents (dev + reviewer) and monitor
# review status via crit. The dev agent should implement with planted bug,
# create review, @mention security, reviewer should spawn, BLOCK, dev should fix,
# re-request, reviewer should re-review and LGTM, then dev merges.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E11_TIMEOUT:-1200}  # 20 minutes default (configurable)
POLL_INTERVAL=30                       # seconds between status checks
STUCK_THRESHOLD=300                    # 5 minutes without progress = stuck

echo "=== E11-L2 Botty-Native Review Cycle Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e11-l2-setup.sh" 2>&1) || {
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
ROUTER_HOOK_OK=false
REVIEWER_HOOK_OK=false
if echo "$HOOKS" | grep -qi "claim.*echo\|respond"; then
  ROUTER_HOOK_OK=true
  echo "  Router hook: OK"
fi
if echo "$HOOKS" | grep -qi "mention.*security\|security.*reviewer"; then
  REVIEWER_HOOK_OK=true
  echo "  Reviewer hook: OK"
fi
if ! $ROUTER_HOOK_OK; then
  echo "WARNING: No router hook found — dev agent may not spawn"
fi
if ! $REVIEWER_HOOK_OK; then
  echo "WARNING: No reviewer hook found — reviewer may not spawn"
fi
echo "--- hooks: OK ---"
echo ""

# --- Send task-request (this triggers the router hook → dev-loop spawns) ---
echo "--- Sending task-request ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup echo \
  "New task: Add GET /files/:name endpoint that reads and returns file contents from the files/ directory. The endpoint should accept a file name as a path parameter, read the file, and return its contents as text. Handle 404 for missing files and 500 for read errors. See bone $BEAD for details." \
  -L task-request
echo "Task-request sent. Router hook should fire shortly and spawn dev agent."
echo ""

# --- Poll loop ---
echo "--- Polling for completion (timeout: ${OVERALL_TIMEOUT}s) ---"
START_TIME=$(date +%s)
LAST_BEAD_STATUS=""
LAST_ACTIVITY_TIME=$START_TIME
LAST_MSG_COUNT=0
LAST_REVIEW_COUNT=0
FINAL_STATUS_DEV="unknown"
FINAL_STATUS_REVIEWER="unknown"

# Phase timing for diagnostics
PHASE_TIMES=""
DEV_SPAWN_TIME=""
REVIEWER_SPAWN_TIME=""
BEAD_CLOSED_TIME=""

while true; do
  ELAPSED=$(( $(date +%s) - START_TIME ))

  # Check overall timeout
  if [[ $ELAPSED -ge $OVERALL_TIMEOUT ]]; then
    echo "TIMEOUT: Overall timeout reached (${OVERALL_TIMEOUT}s)"
    FINAL_STATUS_DEV="timeout"
    FINAL_STATUS_REVIEWER="timeout"
    break
  fi

  sleep "$POLL_INTERVAL"
  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo ""
  echo "--- Poll (${ELAPSED}s / ${OVERALL_TIMEOUT}s) ---"

  # Check vessel — are agents running?
  BOTTY_JSON=$(vessel list --format json 2>/dev/null || echo '{"agents":[]}')
  DEV_RUNNING=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$DEV_AGENT\") | .id" 2>/dev/null || echo "")
  REVIEWER_RUNNING=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$REVIEWER\") | .id" 2>/dev/null || echo "")

  if [[ -n "$DEV_RUNNING" ]]; then
    echo "  Dev agent: RUNNING"
    if [[ -z "$DEV_SPAWN_TIME" ]]; then
      DEV_SPAWN_TIME=$ELAPSED
      PHASE_TIMES+="dev_spawn=${DEV_SPAWN_TIME}s\n"
    fi
  else
    echo "  Dev agent: NOT RUNNING"
  fi

  if [[ -n "$REVIEWER_RUNNING" ]]; then
    echo "  Reviewer agent: RUNNING"
    if [[ -z "$REVIEWER_SPAWN_TIME" ]]; then
      REVIEWER_SPAWN_TIME=$ELAPSED
      PHASE_TIMES+="reviewer_spawn=${REVIEWER_SPAWN_TIME}s\n"
    fi
  else
    echo "  Reviewer agent: NOT RUNNING"
  fi

  # Check bone status
  cd "$PROJECT_DIR"
  BEAD_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json 2>/dev/null || echo "[]")
  BEAD_STATUS=$(echo "$BEAD_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
  echo "  Bone $BEAD: $BEAD_STATUS"

  # Track activity for stuck detection
  if [[ "$BEAD_STATUS" != "$LAST_BEAD_STATUS" ]]; then
    LAST_BEAD_STATUS="$BEAD_STATUS"
    LAST_ACTIVITY_TIME=$(date +%s)
    if [[ "$BEAD_STATUS" == "done" && -z "$BEAD_CLOSED_TIME" ]]; then
      BEAD_CLOSED_TIME=$ELAPSED
      PHASE_TIMES+="bead_closed=${BEAD_CLOSED_TIME}s\n"
    fi
  fi

  # Check for new bus messages (activity indicator)
  CURRENT_MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history echo -n 100 2>/dev/null | wc -l || echo "0")
  if [[ "$CURRENT_MSG_COUNT" -gt "$LAST_MSG_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_MSG_COUNT=$CURRENT_MSG_COUNT
  fi

  # Check review status (key L2 metric)
  REVIEW_JSON=$(cd "$PROJECT_DIR" && BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- crit reviews list --format json 2>/dev/null || echo '{"reviews":[]}')
  REVIEW_COUNT=$(echo "$REVIEW_JSON" | jq '.reviews | length' 2>/dev/null || echo "0")
  echo "  Reviews: $REVIEW_COUNT"
  if [[ "$REVIEW_COUNT" -gt 0 ]]; then
    # Show review states
    echo "$REVIEW_JSON" | jq -r '.reviews[] | "    \(.review_id): \(.state) (votes: \(.vote_summary.lgtm) LGTM, \(.vote_summary.block) BLOCK)"' 2>/dev/null || true
  fi
  if [[ "$REVIEW_COUNT" -gt "$LAST_REVIEW_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_REVIEW_COUNT=$REVIEW_COUNT
  fi

  # Check for completion
  if [[ "$BEAD_STATUS" == "done" ]]; then
    echo "  Bone is DONE — dev agent completed the task!"
    # Give agents time to finish cleanup (release claims, sync, exit)
    # Dev-loop needs to cycle through hasWork() check which takes an iteration
    FINAL_STATUS_DEV="completed-still-running"
    FINAL_STATUS_REVIEWER="completed"
    for WAIT_I in 1 2 3 4 5 6; do
      sleep 15
      BOTTY_FINAL=$(vessel list --format json 2>/dev/null || echo '{"agents":[]}')
      DEV_STILL_RUNNING=$(echo "$BOTTY_FINAL" | jq -r ".agents[] | select(.id == \"$DEV_AGENT\") | .id" 2>/dev/null || echo "")
      REVIEWER_STILL_RUNNING=$(echo "$BOTTY_FINAL" | jq -r ".agents[] | select(.id == \"$REVIEWER\") | .id" 2>/dev/null || echo "")
      [[ -z "$DEV_STILL_RUNNING" ]] && FINAL_STATUS_DEV="completed"
      [[ -n "$REVIEWER_STILL_RUNNING" ]] && FINAL_STATUS_REVIEWER="completed-still-running"
      if [[ -z "$DEV_STILL_RUNNING" && -z "$REVIEWER_STILL_RUNNING" ]]; then
        break
      fi
      echo "  Waiting for agents to exit... (${WAIT_I}/6)"
    done
    break
  fi

  # Stuck detection
  IDLE_TIME=$(( $(date +%s) - LAST_ACTIVITY_TIME ))
  if [[ $IDLE_TIME -ge $STUCK_THRESHOLD ]]; then
    echo "  WARNING: No activity for ${IDLE_TIME}s (threshold: ${STUCK_THRESHOLD}s)"
    # Check if agents are still alive
    DEV_ALIVE=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$DEV_AGENT\") | .id" 2>/dev/null || echo "")
    REVIEWER_ALIVE=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$REVIEWER\") | .id" 2>/dev/null || echo "")
    if [[ -z "$DEV_ALIVE" && -z "$REVIEWER_ALIVE" ]]; then
      echo "  Both agents exited without closing bone — marking as agent-exited"
      FINAL_STATUS_DEV="agent-exited"
      FINAL_STATUS_REVIEWER="agent-exited"
      break
    fi
  fi
done

echo ""
echo "--- Final status: dev=$FINAL_STATUS_DEV, reviewer=$FINAL_STATUS_REVIEWER ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Dev agent log
vessel tail "$DEV_AGENT" -n 500 > "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-${DEV_AGENT}.log"
echo "  dev log: $ARTIFACTS/agent-${DEV_AGENT}.log"

# Reviewer agent log
vessel tail "$REVIEWER" -n 500 > "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-${REVIEWER}.log"
echo "  reviewer log: $ARTIFACTS/agent-${REVIEWER}.log"

# Respond.mjs log (if it spawned)
vessel tail "respond" -n 500 > "$ARTIFACTS/agent-respond.log" 2>/dev/null || \
  echo "(respond agent not found or already exited)" > "$ARTIFACTS/agent-respond.log"

# Also try to capture any dev-loop worker agents
for agent_name in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  if [[ "$agent_name" != "$DEV_AGENT" && "$agent_name" != "$REVIEWER" && "$agent_name" != "respond" ]]; then
    vessel tail "$agent_name" -n 500 > "$ARTIFACTS/agent-${agent_name}.log" 2>/dev/null || true
    echo "  worker log: $ARTIFACTS/agent-${agent_name}.log"
  fi
done

# Channel history
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history echo -n 100 > "$ARTIFACTS/channel-history.log" 2>/dev/null || \
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

# Review state (critical for L2)
cd "$PROJECT_DIR"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- crit reviews list --format json > "$ARTIFACTS/reviews-state.json" 2>/dev/null || \
  echo '{"reviews":[]}' > "$ARTIFACTS/reviews-state.json"
echo "  reviews state: $ARTIFACTS/reviews-state.json"

# Save final status
cat > "$ARTIFACTS/final-status.txt" << EOF
DEV_STATUS=$FINAL_STATUS_DEV
REVIEWER_STATUS=$FINAL_STATUS_REVIEWER
EOF

# Save phase timing
echo -e "$PHASE_TIMES" > "$ARTIFACTS/phase-times.log" 2>/dev/null || true
echo "  phase times: $ARTIFACTS/phase-times.log"

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
vessel kill "$DEV_AGENT" 2>/dev/null || true
vessel kill "$REVIEWER" 2>/dev/null || true
vessel kill "respond" 2>/dev/null || true
# Kill any worker agents that might still be running
for agent_name in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  vessel kill "$agent_name" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e11-l2-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== E11-L2 Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status:"
echo "  Dev agent: $FINAL_STATUS_DEV"
echo "  Reviewer: $FINAL_STATUS_REVIEWER"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Phase timing:"
if [[ -n "$DEV_SPAWN_TIME" ]]; then
  echo "  Dev spawn: ${DEV_SPAWN_TIME}s"
fi
if [[ -n "$REVIEWER_SPAWN_TIME" ]]; then
  echo "  Reviewer spawn: ${REVIEWER_SPAWN_TIME}s"
fi
if [[ -n "$BEAD_CLOSED_TIME" ]]; then
  echo "  Bone done: ${BEAD_CLOSED_TIME}s"
fi
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  ls $ARTIFACTS/"
echo "  cat $ARTIFACTS/agent-${DEV_AGENT}.log"
echo "  cat $ARTIFACTS/agent-${REVIEWER}.log"
echo "  cat $ARTIFACTS/channel-history.log"
echo "  cat $ARTIFACTS/reviews-state.json"
