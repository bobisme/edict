#!/usr/bin/env bash
set -euo pipefail

# E11-L2 Verification Script
# Post-run automated checks for the two-agent review cycle eval. Scores the dev
# and reviewer agents across spawn chain, protocol compliance, review cycle,
# code correctness, and friction extraction (diagnostic).
#
# Runs all tool commands via maw exec (v2 layout).

source "${1:?Usage: e11-l2-verify.sh <path-to-.eval-env>}"

echo "=== E11-L2 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BEAD=$BEAD"
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo ""

PASS=0
FAIL=0
WARN=0
SCORE=0
TOTAL=95
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
# Spawn Chain (20 pts)
# ============================================================
echo "=== Spawn Chain (20 pts) ==="
echo ""

CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-history.log" 2>/dev/null || echo "")
DEV_LOG=$(cat "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null || echo "")
REVIEWER_LOG=$(cat "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null || echo "")

# Check 1: Router hook fired (respond.mjs spawned) (4 pts)
echo "--- Check 1: Router hook fired ---"
ROUTER_FIRED=false
if echo "$CHANNEL_HISTORY" | grep -qi "spawn-ack\|respond\|router"; then
  ROUTER_FIRED=true
fi
if [[ -f "$ARTIFACTS/agent-respond.log" ]]; then
  ROUTER_FIRED=true
fi
check "Router hook fired (respond.mjs spawned)" "$($ROUTER_FIRED && echo 0 || echo 1)" 4

# Check 2: respond.mjs triaged correctly (4 pts)
echo ""
echo "--- Check 2: respond.mjs triaged as work ---"
TRIAGED_AS_WORK=false
# respond.mjs should have created a bead or spawned dev-loop
if echo "$CHANNEL_HISTORY" | grep -qi "dev-loop\|bead.*created\|task-request"; then
  TRIAGED_AS_WORK=true
fi
# Or dev agent got spawned (evidence of dev-loop)
if [[ -f "$ARTIFACTS/agent-${DEV_AGENT}.log" ]]; then
  DEV_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null || echo "0")
  if [[ "$DEV_LOG_SIZE" -gt 100 ]]; then
    TRIAGED_AS_WORK=true
  fi
fi
check "respond.mjs triaged as work (not chat/question)" "$($TRIAGED_AS_WORK && echo 0 || echo 1)" 4

# Check 3: dev-loop spawned by respond.mjs (4 pts)
echo ""
echo "--- Check 3: dev-loop spawned ---"
DEV_SPAWNED=false
if [[ -f "$ARTIFACTS/agent-${DEV_AGENT}.log" ]]; then
  DEV_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null || echo "0")
  if [[ "$DEV_LOG_SIZE" -gt 100 ]]; then
    DEV_SPAWNED=true
  fi
fi
if echo "$CHANNEL_HISTORY" | grep -qi "dev.*start\|dev-loop"; then
  DEV_SPAWNED=true
fi
check "dev-loop spawned" "$($DEV_SPAWNED && echo 0 || echo 1)" 4

# Check 4: Reviewer hook fired on @mention (4 pts)
echo ""
echo "--- Check 4: Reviewer hook fired ---"
REVIEWER_FIRED=false
if [[ -f "$ARTIFACTS/agent-${REVIEWER}.log" ]]; then
  REVIEWER_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null || echo "0")
  if [[ "$REVIEWER_LOG_SIZE" -gt 100 ]]; then
    REVIEWER_FIRED=true
  fi
fi
if echo "$CHANNEL_HISTORY" | grep -qi "@.*review\|reviewer.*start\|security.*spawn"; then
  REVIEWER_FIRED=true
fi
check "Reviewer hook fired on @mention" "$($REVIEWER_FIRED && echo 0 || echo 1)" 4

# Check 5: Both agents exited cleanly (4 pts)
echo ""
echo "--- Check 5: Both agents exited cleanly ---"
FINAL_STATUS_DEV=$(grep -E "^DEV_STATUS=" "$ARTIFACTS/final-status.txt" 2>/dev/null | cut -d'=' -f2 || echo "unknown")
FINAL_STATUS_REVIEWER=$(grep -E "^REVIEWER_STATUS=" "$ARTIFACTS/final-status.txt" 2>/dev/null | cut -d'=' -f2 || echo "unknown")
BOTH_CLEAN=false
if [[ "$FINAL_STATUS_DEV" == "completed" && "$FINAL_STATUS_REVIEWER" == "completed" ]]; then
  BOTH_CLEAN=true
fi
check "Both agents exited cleanly (dev: $FINAL_STATUS_DEV, reviewer: $FINAL_STATUS_REVIEWER)" "$($BOTH_CLEAN && echo 0 || echo 1)" 4

# ============================================================
# Protocol Compliance (30 pts)
# ============================================================
echo ""
echo "=== Protocol Compliance (30 pts) ==="
echo ""

cd "$PROJECT_DIR"

# Check 6: Bone state transitions (5 pts)
echo "--- Check 6: Bone state transitions ---"
BEAD_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$BEAD" --format json 2>/dev/null || echo "[]")
BEAD_STATUS=$(echo "$BEAD_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
BEAD_TRANSITIONS=false
# Bone should be done by end (open → doing → done)
if [[ "$BEAD_STATUS" == "done" ]]; then
  BEAD_TRANSITIONS=true
fi
# Also check channel history for status announcements
if echo "$CHANNEL_HISTORY" | grep -qi "doing\|claim\|started.*work"; then
  BEAD_TRANSITIONS=true
fi
check "Bone state transitions (open → doing → done)" "$($BEAD_TRANSITIONS && echo 0 || echo 1)" 5

# Check 7: Progress comments posted (3 pts)
echo ""
echo "--- Check 7: Progress comments ---"
PROGRESS_COMMENTS=false
COMMENT_COUNT=$(echo "$BEAD_JSON" | jq -r '.[0].comments // [] | length' 2>/dev/null || echo "0")
if [[ "$COMMENT_COUNT" -gt 1 ]]; then
  PROGRESS_COMMENTS=true
fi
check "Progress comments posted to bead" "$($PROGRESS_COMMENTS && echo 0 || echo 1)" 3

# Check 8: Workspace created (3 pts)
echo ""
echo "--- Check 8: Workspace created ---"
WS_CREATED=false
WS_JSON=$(cat "$ARTIFACTS/workspace-state.json" 2>/dev/null || echo "{}")
WS_COUNT=$(echo "$WS_JSON" | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "0")
# If bone done and workspace merged, it wouldn't show in current list
if [[ "$BEAD_STATUS" == "done" ]]; then
  WS_CREATED=true
elif [[ "$WS_COUNT" -gt 0 ]]; then
  WS_CREATED=true
fi
if echo "$CHANNEL_HISTORY" | grep -qi "workspace"; then
  WS_CREATED=true
fi
check "Workspace created with maw ws create" "$($WS_CREATED && echo 0 || echo 1)" 3

# Check 9: Claims staked (bone:// and workspace://) (4 pts)
echo ""
echo "--- Check 9: Claims staked ---"
CLAIMS_STAKED=false
# Check artifacts or channel history for claim evidence
if echo "$CHANNEL_HISTORY" | grep -qi "claim.*stake\|claimed.*bone\|claimed.*workspace"; then
  CLAIMS_STAKED=true
fi
# If bone reached doing and workspace created, claims were likely staked
if [[ "$BEAD_STATUS" == "done" || "$BEAD_STATUS" == "doing" ]]; then
  CLAIMS_STAKED=true
fi
check "Claims staked (bone:// and workspace://)" "$($CLAIMS_STAKED && echo 0 || echo 1)" 4

# Check 10: Claims released (5 pts)
echo ""
echo "--- Check 10: Claims released ---"
CLAIMS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list --agent "$DEV_AGENT" 2>/dev/null || true)
WORK_CLAIM_COUNT=$(echo "$CLAIMS" | grep -cE "bone://|workspace://" || true)
check "Claims released after work" "$([ "$WORK_CLAIM_COUNT" -eq 0 ] && echo 0 || echo 1)" 5

# Check 11: bn commands used (2 pts)
echo ""
echo "--- Check 11: bn commands used ---"
BN_USED=false
if echo "$DEV_LOG" | grep -qi "bn "; then
  BN_USED=true
fi
check "bn commands used" "$($BN_USED && echo 0 || echo 1)" 2

# Check 12: Bus labels correct (4 pts)
echo ""
echo "--- Check 12: Bus labels correct ---"
# Labels are metadata, not visible in text format — must use JSON
LIVE_HISTORY_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history "$(basename "$PROJECT_DIR")" -n 100 --format json 2>/dev/null || echo '{"messages":[]}')
LIVE_HISTORY=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history "$(basename "$PROJECT_DIR")" -n 100 2>/dev/null || echo "")
ALL_LABELS=$(echo "$LIVE_HISTORY_JSON" | jq -r '[.messages[].labels // [] | .[]] | .[]' 2>/dev/null || echo "")
LABELS_OK=true
HAS_TASK_CLAIM=false
HAS_REVIEW_REQUEST=false
HAS_REVIEW_DONE=false
HAS_TASK_DONE=false

if echo "$ALL_LABELS" | grep -q "task-claim"; then
  HAS_TASK_CLAIM=true
fi
if echo "$ALL_LABELS" | grep -q "review-request"; then
  HAS_REVIEW_REQUEST=true
fi
if echo "$ALL_LABELS" | grep -q "review-done"; then
  HAS_REVIEW_DONE=true
fi
if echo "$ALL_LABELS" | grep -q "task-done"; then
  HAS_TASK_DONE=true
fi

if ! $HAS_TASK_CLAIM; then
  warn "No task-claim label found"
  LABELS_OK=false
fi
if ! $HAS_REVIEW_REQUEST; then
  warn "No review-request label found"
  LABELS_OK=false
fi
if ! $HAS_REVIEW_DONE; then
  warn "No review-done label found"
  LABELS_OK=false
fi
if ! $HAS_TASK_DONE; then
  warn "No task-done label found"
  LABELS_OK=false
fi

check "Bus labels correct (task-claim, review-request, review-done, task-done)" "$($LABELS_OK && echo 0 || echo 1)" 4

# Check 13: Channel announcements (4 pts)
echo ""
echo "--- Check 13: Channel announcements ---"
ANNOUNCEMENTS_OK=false
# Should have start, progress, review request, completion
if echo "$LIVE_HISTORY" | grep -qi "start\|progress\|review.*request\|complet"; then
  ANNOUNCEMENTS_OK=true
fi
check "Channel announcements (start, progress, completion)" "$($ANNOUNCEMENTS_OK && echo 0 || echo 1)" 4

# ============================================================
# Review Cycle (30 pts)
# ============================================================
echo ""
echo "=== Review Cycle (30 pts) ==="
echo ""

# Check 14: crit reviews create from workspace diff (3 pts)
echo "--- Check 14: crit reviews create ---"
REVIEW_CREATED=false
if echo "$DEV_LOG" | grep -qi "crit reviews create"; then
  REVIEW_CREATED=true
fi
if echo "$LIVE_HISTORY" | grep -qi "review.*created\|crit.*create"; then
  REVIEW_CREATED=true
fi
check "crit reviews create from workspace diff" "$($REVIEW_CREATED && echo 0 || echo 1)" 3

# Check 15: crit reviews request with @reviewer (3 pts)
echo ""
echo "--- Check 15: crit reviews request ---"
REVIEW_REQUESTED=false
if echo "$DEV_LOG" | grep -qi "crit reviews request"; then
  REVIEW_REQUESTED=true
fi
if echo "$LIVE_HISTORY" | grep -qi "review.*request.*@"; then
  REVIEW_REQUESTED=true
fi
check "crit reviews request with @reviewer mention" "$($REVIEW_REQUESTED && echo 0 || echo 1)" 3

# Check 16: Bus message contains @mention (triggers hook) (2 pts)
echo ""
echo "--- Check 16: Bus @mention ---"
BUS_MENTION=false
if echo "$LIVE_HISTORY" | grep -qi "@"; then
  BUS_MENTION=true
fi
check "Bus message contains @mention" "$($BUS_MENTION && echo 0 || echo 1)" 2

# Check 17: Reviewer read code from workspace path (3 pts)
echo ""
echo "--- Check 17: Reviewer read from workspace ---"
REVIEWER_WS_READ=false
if echo "$REVIEWER_LOG" | grep -qi "ws/.*src\|workspace.*path\|read.*ws/"; then
  REVIEWER_WS_READ=true
fi
check "Reviewer read code from workspace path (ws/\$WS/)" "$($REVIEWER_WS_READ && echo 0 || echo 1)" 3

# Check 18: Reviewer identified planted defect (5 pts)
echo ""
echo "--- Check 18: Reviewer found defect ---"
DEFECT_FOUND=false
if echo "$REVIEWER_LOG" | grep -qi "path.*travers\|security\|vulnerab\|defect\|bug"; then
  DEFECT_FOUND=true
fi
# Check crit review for comments (if we can extract review ID)
# For now, use log evidence
check "Reviewer identified planted defect" "$($DEFECT_FOUND && echo 0 || echo 1)" 5

# Check 19: Reviewer BLOCKed with relevant comment (3 pts)
echo ""
echo "--- Check 19: Reviewer blocked ---"
REVIEWER_BLOCKED=false
if echo "$REVIEWER_LOG" | grep -qi "crit block\|block.*review\|BLOCK"; then
  REVIEWER_BLOCKED=true
fi
check "Reviewer BLOCKed review" "$($REVIEWER_BLOCKED && echo 0 || echo 1)" 3

# Check 20: Dev addressed feedback in workspace (3 pts)
echo ""
echo "--- Check 20: Dev fixed in workspace ---"
DEV_FIXED=false
if echo "$DEV_LOG" | grep -qi "fix\|address.*feedback\|reply.*thread"; then
  DEV_FIXED=true
fi
check "Dev addressed feedback in workspace" "$($DEV_FIXED && echo 0 || echo 1)" 3

# Check 21: Dev re-requested review (2 pts)
echo ""
echo "--- Check 21: Dev re-requested review ---"
REREQUEST=false
if echo "$DEV_LOG" | grep -qi "re-request\|request.*again\|crit reviews request"; then
  REREQUEST=true
fi
check "Dev re-requested review after fix" "$($REREQUEST && echo 0 || echo 1)" 2

# Check 22: Reviewer re-reviewed from workspace (3 pts)
echo ""
echo "--- Check 22: Reviewer re-reviewed ---"
REREVIEW=false
if echo "$REVIEWER_LOG" | grep -qi "re-review\|verified.*fix\|read.*src"; then
  REREVIEW=true
fi
check "Reviewer re-reviewed from workspace (not cached)" "$($REREVIEW && echo 0 || echo 1)" 3

# Check 23: Reviewer LGTMd (2 pts)
echo ""
echo "--- Check 23: Reviewer LGTMd ---"
LGTM=false
if echo "$REVIEWER_LOG" | grep -qi "lgtm\|crit lgtm\|approved"; then
  LGTM=true
fi
check "Reviewer LGTMd after fix" "$($LGTM && echo 0 || echo 1)" 2

# Check 24: crit reviews mark-merged after merge (1 pt)
echo ""
echo "--- Check 24: Review marked merged ---"
MARKED_MERGED=false
if echo "$DEV_LOG" | grep -qi "mark-merged\|crit reviews.*merg"; then
  MARKED_MERGED=true
fi
check "crit reviews mark-merged after merge" "$($MARKED_MERGED && echo 0 || echo 1)" 1

# ============================================================
# Code Correctness (15 pts)
# ============================================================
echo ""
echo "=== Code Correctness (15 pts) ==="
echo ""

# Check 25: cargo check passes on main (5 pts)
echo "--- Check 25: cargo check passes ---"
CARGO_OK=false
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  CARGO_OK=true
else
  warn "cargo check failed"
fi
check "cargo check passes on main" "$($CARGO_OK && echo 0 || echo 1)" 5

# Check 26: Endpoint exists and wired (3 pts)
echo ""
echo "--- Check 26: Endpoint exists ---"
ENDPOINT_EXISTS=false
MAIN_RS="$PROJECT_DIR/ws/default/src/main.rs"
if [[ -f "$MAIN_RS" ]] && grep -qi "files" "$MAIN_RS" 2>/dev/null; then
  ENDPOINT_EXISTS=true
fi
check "Endpoint exists and is wired" "$($ENDPOINT_EXISTS && echo 0 || echo 1)" 3

# Check 27: Planted defect fixed (5 pts)
echo ""
echo "--- Check 27: Defect fixed in final code ---"
DEFECT_FIXED=false
# Check if main.rs has path traversal protection (various valid approaches)
if [[ -f "$MAIN_RS" ]] && grep -qi "canonical\|starts_with\|path.*validat\|ParentDir\|parent_dir\|traversal\|\.\.\/" "$MAIN_RS" 2>/dev/null; then
  DEFECT_FIXED=true
fi
check "Planted defect fixed in final code" "$($DEFECT_FIXED && echo 0 || echo 1)" 5

# Check 28: Response format matches spec (2 pts)
echo ""
echo "--- Check 28: Response format ---"
FORMAT_OK=false
# Endpoint should return file contents or 404/500
if [[ -f "$MAIN_RS" ]] && grep -qi "StatusCode\|404\|500" "$MAIN_RS" 2>/dev/null; then
  FORMAT_OK=true
fi
check "Response format matches spec" "$($FORMAT_OK && echo 0 || echo 1)" 2

# ============================================================
# Friction Extraction (diagnostic, not scored)
# ============================================================
echo ""
echo "=== Friction Efficiency (10 pts) ==="
echo ""

echo "Analyzing agent logs for friction signals..."
echo ""

# Count tool errors in dev log
# NOTE: grep -c outputs "0" and exits 1 when no matches — use VAR=$(grep -c ...) || VAR=0 to avoid
# the broken $(grep -c ... || echo "0") pattern that produces "0\n0"
DEV_ERRORS=$(grep -c "Exit code [12]" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null) || DEV_ERRORS=0
DEV_HELP_LOOKUPS=$(grep -c "\-\-help" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null) || DEV_HELP_LOOKUPS=0
DEV_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null) || DEV_RETRIES=0

# Count tool errors in reviewer log
REVIEWER_ERRORS=$(grep -c "Exit code [12]" "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null) || REVIEWER_ERRORS=0
REVIEWER_HELP_LOOKUPS=$(grep -c "\-\-help" "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null) || REVIEWER_HELP_LOOKUPS=0
REVIEWER_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null) || REVIEWER_RETRIES=0

# Count path confusion (wrong workspace references) — grep -c per file to avoid multiline output
_PC_DEV=$(grep -c "No such file\|cannot find\|path.*not found" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null) || _PC_DEV=0
_PC_REV=$(grep -c "No such file\|cannot find\|path.*not found" "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null) || _PC_REV=0
PATH_CONFUSION=$((_PC_DEV + _PC_REV))

# Count duplicate operations
DUPLICATE_OPS=0
if grep -q "bone.*already.*exists\|workspace.*already" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null; then
  DUPLICATE_OPS=$((DUPLICATE_OPS + 1))
fi

echo "Dev agent:"
echo "  Tool errors (exit code 1/2): $DEV_ERRORS"
echo "  --help lookups: $DEV_HELP_LOOKUPS"
echo "  Retry attempts: $DEV_RETRIES"
echo ""
echo "Reviewer agent:"
echo "  Tool errors (exit code 1/2): $REVIEWER_ERRORS"
echo "  --help lookups: $REVIEWER_HELP_LOOKUPS"
echo "  Retry attempts: $REVIEWER_RETRIES"
echo ""
echo "Cross-cutting:"
echo "  Path confusion instances: $PATH_CONFUSION"
echo "  Duplicate operations: $DUPLICATE_OPS"
echo ""

# Totals across both agents
TOTAL_ERRORS=$((DEV_ERRORS + REVIEWER_ERRORS))
TOTAL_HELP=$((DEV_HELP_LOOKUPS + REVIEWER_HELP_LOOKUPS))
TOTAL_RETRIES=$((DEV_RETRIES + REVIEWER_RETRIES))
TOTAL_FRICTION=$((TOTAL_ERRORS + TOTAL_HELP + TOTAL_RETRIES + PATH_CONFUSION + DUPLICATE_OPS))
echo "Totals: $TOTAL_ERRORS errors, $TOTAL_HELP --help, $TOTAL_RETRIES retries, $PATH_CONFUSION path confusion, $DUPLICATE_OPS duplicates"
echo "Total friction signals: $TOTAL_FRICTION"
echo ""

# --- Check 29: Tool errors ---
echo "--- Check 29: Tool errors ---"
TOTAL=$((TOTAL + 5))
if [[ "$TOTAL_ERRORS" -eq 0 ]]; then
  echo "PASS (5 pts): Zero tool errors"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 5 ]]; then
  echo "PARTIAL (3/5 pts): $TOTAL_ERRORS tool errors (threshold: ≤5)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $TOTAL_ERRORS tool errors (threshold: ≤5)"
  FAIL=$((FAIL + 1))
fi
echo ""

# --- Check 30: --help lookups ---
echo "--- Check 30: --help lookups ---"
TOTAL=$((TOTAL + 3))
if [[ "$TOTAL_HELP" -eq 0 ]]; then
  echo "PASS (3 pts): Zero --help lookups"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$TOTAL_HELP" -le 2 ]]; then
  echo "PARTIAL (2/3 pts): $TOTAL_HELP --help lookups (threshold: ≤2)"
  SCORE=$((SCORE + 2)); PASS=$((PASS + 1))
else
  echo "FAIL (0/3 pts): $TOTAL_HELP --help lookups (threshold: ≤2)"
  FAIL=$((FAIL + 1))
fi
echo ""

# --- Check 31: Retry attempts ---
echo "--- Check 31: Retry attempts ---"
TOTAL=$((TOTAL + 2))
if [[ "$TOTAL_RETRIES" -eq 0 ]]; then
  echo "PASS (2 pts): Zero retry attempts"
  SCORE=$((SCORE + 2)); PASS=$((PASS + 1))
elif [[ "$TOTAL_RETRIES" -le 2 ]]; then
  echo "PARTIAL (1/2 pts): $TOTAL_RETRIES retries (threshold: ≤2)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/2 pts): $TOTAL_RETRIES retries (threshold: ≤2)"
  FAIL=$((FAIL + 1))
fi
echo ""

# Additional diagnostics (not scored)
# Phase timing (from artifacts if captured)
if [[ -f "$ARTIFACTS/phase-times.log" ]]; then
  echo "Phase timing:"
  cat "$ARTIFACTS/phase-times.log"
  echo ""
fi

# Iteration counts (from logs)
DEV_ITERATIONS=$(grep -c "iteration\|loop.*start" "$ARTIFACTS/agent-${DEV_AGENT}.log" 2>/dev/null) || DEV_ITERATIONS=0
REVIEWER_ITERATIONS=$(grep -c "iteration\|loop.*start" "$ARTIFACTS/agent-${REVIEWER}.log" 2>/dev/null) || REVIEWER_ITERATIONS=0
echo "Iterations:"
echo "  Dev agent: $DEV_ITERATIONS"
echo "  Reviewer agent: $REVIEWER_ITERATIONS"
echo ""

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
elif [[ "$SCORE" -ge 70 ]]; then
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
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history \$(basename \"\$PROJECT_DIR\")' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-${DEV_AGENT}.log' for dev agent output"
echo "Run 'cat $ARTIFACTS/agent-${REVIEWER}.log' for reviewer output"
echo ""
echo "=== Verification Complete ==="
