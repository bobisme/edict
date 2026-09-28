#!/usr/bin/env bash
set -euo pipefail

# E11-L1 Verification Script
# Post-run automated checks for the vessel-native eval. Scores the agent's
# performance across 9 criteria (50 pts total).
#
# Runs all tool commands via maw exec (v2 layout).

source "${1:?Usage: e11-l1-verify.sh <path-to-.eval-env>}"

echo "=== E11-L1 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BEAD=$BEAD"
echo ""

PASS=0
FAIL=0
WARN=0
SCORE=0
TOTAL=50
ARTIFACTS="$EVAL_DIR/artifacts"

check() {
  local label="$1"
  local result="$2"  # 0 = pass, 1 = fail
  local pts="${3:-0}"
  if [[ "$result" -eq 0 ]]; then
    echo "PASS ($pts pts): $label"
    PASS=$((PASS + 1))
    SCORE=$((SCORE + pts))
  else
    echo "FAIL (0/$pts pts): $label"
    FAIL=$((FAIL + 1))
  fi
}

warn() {
  local label="$1"
  echo "WARN: $label"
  WARN=$((WARN + 1))
}

# ============================================================
# Check 1: Hook fired and agent spawned (5 pts)
# Evidence: bus history shows spawn-ack or agent activity beyond setup
# ============================================================
echo "--- Check 1: Hook fired and agent spawned ---"

CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-history.log" 2>/dev/null || echo "")
AGENT_LOG=$(cat "$ARTIFACTS/agent-echo-dev.log" 2>/dev/null || echo "")

HOOK_FIRED=false
# Check for spawn-ack in channel history
if echo "$CHANNEL_HISTORY" | grep -qi "spawn-ack\|hook.*fired\|agent.*start\|dev-loop\|iteration"; then
  HOOK_FIRED=true
fi
# Check if agent log has content (agent was spawned by vessel)
if [[ -f "$ARTIFACTS/agent-echo-dev.log" ]] && ! grep -q "already exited" "$ARTIFACTS/agent-echo-dev.log" 2>/dev/null; then
  AGENT_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-echo-dev.log" 2>/dev/null || echo "0")
  if [[ "$AGENT_LOG_SIZE" -gt 100 ]]; then
    HOOK_FIRED=true
  fi
fi
# Check if there are messages from echo-dev (not just setup) in channel history
if echo "$CHANNEL_HISTORY" | grep -qi "echo-dev"; then
  HOOK_FIRED=true
fi

check "Hook fired and agent spawned" "$($HOOK_FIRED && echo 0 || echo 1)" 5

# ============================================================
# Check 2: Bone claimed / doing at some point (5 pts)
# ============================================================
echo ""
echo "--- Check 2: Bone claimed (doing) ---"

BEAD_CLAIMED=false
# Check current bone state
cd "$PROJECT_DIR"
BEAD_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json 2>/dev/null || echo "[]")
BEAD_STATUS=$(echo "$BEAD_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")

# If bone is doing or done, it was claimed at some point
if [[ "$BEAD_STATUS" == "doing" || "$BEAD_STATUS" == "done" ]]; then
  BEAD_CLAIMED=true
fi
# Also check channel history for claim messages
if echo "$CHANNEL_HISTORY" | grep -qi "task-claim\|claim.*bone\|doing\|started.*work\|working.*on"; then
  BEAD_CLAIMED=true
fi

check "Bone claimed (doing at some point)" "$($BEAD_CLAIMED && echo 0 || echo 1)" 5

# ============================================================
# Check 3: Workspace created (5 pts)
# ============================================================
echo ""
echo "--- Check 3: Workspace created ---"

WS_CREATED=false
# Check current workspace state
WS_JSON=$(cat "$ARTIFACTS/workspace-state.json" 2>/dev/null || echo "{}")
WS_COUNT=$(echo "$WS_JSON" | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "0")

# If non-default workspaces exist now, one was created
if [[ "$WS_COUNT" -gt 0 ]]; then
  WS_CREATED=true
fi
# Check channel history for workspace mentions
if echo "$CHANNEL_HISTORY" | grep -qi "workspace\|ws.*creat"; then
  WS_CREATED=true
fi
# Check agent log for maw ws create
if echo "$AGENT_LOG" | grep -qi "maw ws create\|workspace.*creat"; then
  WS_CREATED=true
fi
# If workspace was created and merged, it wouldn't show in current list,
# but we'd see evidence in the bone being done + code on main
if [[ "$BEAD_STATUS" == "done" ]]; then
  WS_CREATED=true
fi

check "Workspace created" "$($WS_CREATED && echo 0 || echo 1)" 5

# ============================================================
# Check 4: Code implemented and compiles (10 pts)
# ============================================================
echo ""
echo "--- Check 4: Code implemented and compiles ---"

CODE_OK=false
cd "$PROJECT_DIR"

# Check if cargo check passes from default workspace
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  # Check if /version endpoint exists in source
  MAIN_RS="$PROJECT_DIR/ws/default/src/main.rs"
  if [[ -f "$MAIN_RS" ]] && grep -qi "version" "$MAIN_RS" 2>/dev/null; then
    CODE_OK=true
  else
    warn "cargo check passes but /version endpoint not found in main.rs"
  fi
else
  warn "cargo check failed"
fi

check "Code implemented and compiles" "$($CODE_OK && echo 0 || echo 1)" 10

# ============================================================
# Check 5: Workspace merged (5 pts)
# ============================================================
echo ""
echo "--- Check 5: Workspace merged ---"

# Re-read current workspace state (may have changed since artifacts captured)
cd "$PROJECT_DIR"
CURRENT_WS_COUNT=$(maw ws list --format json 2>/dev/null | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "0")
check "Workspace merged (no non-default workspaces remain)" "$([ "$CURRENT_WS_COUNT" -eq 0 ] && echo 0 || echo 1)" 5

# ============================================================
# Check 6: Bone done (5 pts)
# ============================================================
echo ""
echo "--- Check 6: Bone done ---"

# Re-read current bone state
cd "$PROJECT_DIR"
CURRENT_BEAD_STATUS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json 2>/dev/null | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
check "Bone done" "$([ "$CURRENT_BEAD_STATUS" = "done" ] && echo 0 || echo 1)" 5

# ============================================================
# Check 7: Claims released (5 pts)
# ============================================================
echo ""
echo "--- Check 7: Claims released ---"

CLAIMS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list --agent "$ECHO_DEV" 2>/dev/null || true)
# Only count bone:// and workspace:// claims — agent:// claims are managed by hooks
WORK_CLAIM_COUNT=$(echo "$CLAIMS" | grep -cE "bone://|workspace://" || true)
check "Claims released (no bone:// or workspace:// claims)" "$([ "$WORK_CLAIM_COUNT" -eq 0 ] && echo 0 || echo 1)" 5

# ============================================================
# Check 8: Agent exited cleanly (5 pts)
# Evidence: final-status.txt says "completed" (agent finished on its own)
# vs "timeout" or "agent-exited" (we had to kill it)
# ============================================================
echo ""
echo "--- Check 8: Agent exited cleanly ---"

FINAL_STATUS=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "unknown")
AGENT_CLEAN=false
if [[ "$FINAL_STATUS" == "completed" ]]; then
  AGENT_CLEAN=true
fi
check "Agent exited cleanly (final status: $FINAL_STATUS)" "$($AGENT_CLEAN && echo 0 || echo 1)" 5

# ============================================================
# Check 9: Bus labels correct (5 pts)
# ============================================================
echo ""
echo "--- Check 9: Bus labels correct ---"

# Re-read channel history from live data
cd "$PROJECT_DIR"
LIVE_HISTORY=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history echo -n 50 2>/dev/null || echo "")

LABELS_OK=true
HAS_TASK_CLAIM=false
HAS_TASK_DONE=false

if echo "$LIVE_HISTORY" | grep -qi "task-claim\|claim"; then
  HAS_TASK_CLAIM=true
fi
if echo "$LIVE_HISTORY" | grep -qi "task-done\|completed\|closed\|released\|finished"; then
  HAS_TASK_DONE=true
fi

if ! $HAS_TASK_CLAIM; then
  warn "No task-claim label found in channel history"
  LABELS_OK=false
fi
if ! $HAS_TASK_DONE; then
  warn "No task-done label found in channel history"
  LABELS_OK=false
fi

check "Bus labels correct (task-claim and task-done)" "$($LABELS_OK && echo 0 || echo 1)" 5

# ============================================================
# Summary
# ============================================================
echo ""
echo "=== Verification Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "WARN: $WARN"
echo "SCORE: $SCORE / $TOTAL"
echo ""

if [[ "$FAIL" -eq 0 ]]; then
  echo "RESULT: ALL CHECKS PASSED ($SCORE/$TOTAL)"
elif [[ "$SCORE" -ge 35 ]]; then
  echo "RESULT: PASS ($SCORE/$TOTAL) — $FAIL checks failed"
else
  echo "RESULT: FAIL ($SCORE/$TOTAL) — $FAIL checks failed"
fi

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BEAD=$BEAD"
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history echo' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-echo-dev.log' for agent output"
echo ""
echo "=== Verification Complete ==="
