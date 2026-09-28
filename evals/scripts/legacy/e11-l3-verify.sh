#!/usr/bin/env bash
set -euo pipefail

# E11-L3 Verification Script
# Automated scoring for the two-project, three-agent vessel-native eval.
# Checks spawn chain, protocol compliance, cross-project coordination,
# review cycle, code correctness, and friction.

source "${1:?Usage: e11-l3-verify.sh <path-to-.eval-env>}"

echo "=== E11-L3 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BETA_DIR=$BETA_DIR"
echo "ALPHA_BEAD=$ALPHA_BEAD"
echo "ALPHA_DEV=$ALPHA_DEV"
echo "ALPHA_SECURITY=$ALPHA_SECURITY"
echo "BETA_DEV=$BETA_DEV"
echo ""

PASS=0
FAIL=0
WARN=0
SCORE=0
TOTAL=0
ARTIFACTS="$EVAL_DIR/artifacts"

check() {
  local label="$1"
  local result="$2"  # 0 = pass, 1 = fail
  local pts="${3:-0}"
  TOTAL=$((TOTAL + pts))
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
  echo "WARN: $1"
  WARN=$((WARN + 1))
}

# Load artifacts
ALPHA_HISTORY=$(cat "$ARTIFACTS/channel-alpha-history.log" 2>/dev/null || echo "")
BETA_HISTORY=$(cat "$ARTIFACTS/channel-beta-history.log" 2>/dev/null || echo "")
ALPHA_DEV_LOG=$(cat "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null || echo "")
ALPHA_SEC_LOG=$(cat "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null || echo "")
BETA_DEV_LOG=$(cat "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null || echo "")

# JSON artifacts for label extraction
ALPHA_HISTORY_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history alpha -n 200 --format json 2>/dev/null || echo '{"messages":[]}')
BETA_HISTORY_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history beta -n 200 --format json 2>/dev/null || echo '{"messages":[]}')
ALPHA_LABELS=$(echo "$ALPHA_HISTORY_JSON" | jq -r '[.messages[].labels // [] | .[]] | .[]' 2>/dev/null || echo "")
BETA_LABELS=$(echo "$BETA_HISTORY_JSON" | jq -r '[.messages[].labels // [] | .[]] | .[]' 2>/dev/null || echo "")

# ============================================================
# Spawn Chain (25 pts)
# ============================================================
echo "=== Spawn Chain (25 pts) ==="
echo ""

# Check 1: Alpha router hook fired (5 pts)
echo "--- Check 1: Alpha router hook fired ---"
ALPHA_ROUTER_FIRED=false
if [[ -f "$ARTIFACTS/agent-${ALPHA_DEV}.log" ]]; then
  DEV_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null || echo "0")
  [[ "$DEV_LOG_SIZE" -gt 100 ]] && ALPHA_ROUTER_FIRED=true
fi
if echo "$ALPHA_HISTORY" | grep -qi "dev-loop\|spawn\|respond"; then
  ALPHA_ROUTER_FIRED=true
fi
check "Alpha router hook fired (respond → dev-loop)" "$($ALPHA_ROUTER_FIRED && echo 0 || echo 1)" 5

# Check 2: Beta-dev spawned (cross-project trigger) (5 pts)
echo ""
echo "--- Check 2: Beta-dev spawned ---"
BETA_DEV_SPAWNED=false
if [[ -f "$ARTIFACTS/agent-${BETA_DEV}.log" ]]; then
  BD_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null || echo "0")
  [[ "$BD_LOG_SIZE" -gt 100 ]] && BETA_DEV_SPAWNED=true
fi
if echo "$BETA_HISTORY" | grep -qi "beta-dev\|spawn\|respond"; then
  BETA_DEV_SPAWNED=true
fi
check "Beta-dev spawned via cross-project communication" "$($BETA_DEV_SPAWNED && echo 0 || echo 1)" 5

# Check 3: Alpha-security spawned (mention hook) (5 pts)
echo ""
echo "--- Check 3: Alpha-security spawned ---"
ALPHA_SEC_SPAWNED=false
if [[ -f "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" ]]; then
  SEC_LOG_SIZE=$(wc -c < "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null || echo "0")
  [[ "$SEC_LOG_SIZE" -gt 100 ]] && ALPHA_SEC_SPAWNED=true
fi
check "Alpha-security spawned via @mention hook" "$($ALPHA_SEC_SPAWNED && echo 0 || echo 1)" 5

# Check 4: Agents spawned in expected order (5 pts)
echo ""
echo "--- Check 4: Spawn order ---"
SPAWN_ORDER_OK=false
# Read phase times
ALPHA_DEV_T=$(grep "alpha_dev_spawn=" "$ARTIFACTS/phase-times.log" 2>/dev/null | grep -oP '\d+' | head -1 || echo "0")
BETA_DEV_T=$(grep "beta_dev_spawn=" "$ARTIFACTS/phase-times.log" 2>/dev/null | grep -oP '\d+' | head -1 || echo "0")
ALPHA_SEC_T=$(grep "alpha_security_spawn=" "$ARTIFACTS/phase-times.log" 2>/dev/null | grep -oP '\d+' | head -1 || echo "0")
# Expected: alpha-dev first, then beta-dev, then alpha-security
if [[ "$ALPHA_DEV_T" -gt 0 ]]; then
  SPAWN_ORDER_OK=true
  if [[ "$BETA_DEV_T" -gt 0 && "$BETA_DEV_T" -lt "$ALPHA_DEV_T" ]]; then
    SPAWN_ORDER_OK=false
    warn "beta-dev spawned before alpha-dev"
  fi
  if [[ "$ALPHA_SEC_T" -gt 0 && "$ALPHA_SEC_T" -lt "$ALPHA_DEV_T" ]]; then
    SPAWN_ORDER_OK=false
    warn "alpha-security spawned before alpha-dev"
  fi
fi
check "Agents spawned in expected order (alpha-dev → beta-dev → alpha-security)" "$($SPAWN_ORDER_OK && echo 0 || echo 1)" 5

# Check 5: All agents exited cleanly (5 pts)
echo ""
echo "--- Check 5: All agents exited cleanly ---"
AD_STATUS=$(grep -E "^ALPHA_DEV_STATUS=" "$ARTIFACTS/final-status.txt" 2>/dev/null | cut -d'=' -f2 || echo "unknown")
AS_STATUS=$(grep -E "^ALPHA_SECURITY_STATUS=" "$ARTIFACTS/final-status.txt" 2>/dev/null | cut -d'=' -f2 || echo "unknown")
BD_STATUS=$(grep -E "^BETA_DEV_STATUS=" "$ARTIFACTS/final-status.txt" 2>/dev/null | cut -d'=' -f2 || echo "unknown")
ALL_CLEAN=false
if [[ "$AD_STATUS" == "completed" && "$AS_STATUS" == "completed" && "$BD_STATUS" == "completed" ]]; then
  ALL_CLEAN=true
fi
check "All agents exited cleanly (ad:$AD_STATUS as:$AS_STATUS bd:$BD_STATUS)" "$($ALL_CLEAN && echo 0 || echo 1)" 5

# ============================================================
# Cross-Project Coordination (30 pts)
# ============================================================
echo ""
echo "=== Cross-Project Coordination (30 pts) ==="
echo ""

# Check 6: Alpha-dev discovered beta validate_email bug (5 pts)
echo "--- Check 6: Bug discovery ---"
BUG_DISCOVERED=false
if echo "$ALPHA_DEV_LOG" | grep -qi "validate_email\|plus.*address\|\\+.*reject\|beta.*bug\|email.*fail"; then
  BUG_DISCOVERED=true
fi
check "Alpha-dev discovered beta validate_email bug" "$($BUG_DISCOVERED && echo 0 || echo 1)" 5

# Check 7: Alpha-dev queried #projects registry (3 pts)
echo ""
echo "--- Check 7: Projects registry query ---"
PROJECTS_QUERIED=false
if echo "$ALPHA_DEV_LOG" | grep -qi "bus history projects\|bus inbox.*projects\|#projects"; then
  PROJECTS_QUERIED=true
fi
check "Alpha-dev queried #projects registry" "$($PROJECTS_QUERIED && echo 0 || echo 1)" 3

# Check 8: Alpha-dev sent message to beta channel (5 pts)
echo ""
echo "--- Check 8: Cross-project message to beta ---"
CROSS_MSG=false
if echo "$BETA_HISTORY" | grep -qi "alpha-dev\|validate_email\|plus\|email.*bug\|subaddress"; then
  CROSS_MSG=true
fi
# Also check alpha-dev log for bus send to beta
if echo "$ALPHA_DEV_LOG" | grep -qi "bus send.*beta"; then
  CROSS_MSG=true
fi
check "Alpha-dev sent message to beta channel" "$($CROSS_MSG && echo 0 || echo 1)" 5

# Check 9: Beta-dev investigated and responded (5 pts)
echo ""
echo "--- Check 9: Beta-dev investigated ---"
BETA_INVESTIGATED=false
if echo "$BETA_DEV_LOG" | grep -qi "validate_email\|lib.rs\|plus\|local.*part"; then
  BETA_INVESTIGATED=true
fi
check "Beta-dev investigated own code and responded" "$($BETA_INVESTIGATED && echo 0 || echo 1)" 5

# Check 10: Beta-dev fixed validate_email (5 pts)
echo ""
echo "--- Check 10: Beta validate_email fixed ---"
BETA_FIXED=false
# Check if beta's validate_email now allows + in local part
BETA_LIB="$BETA_DIR/ws/default/src/lib.rs"
if [[ -f "$BETA_LIB" ]] && grep -qi "'+'" "$BETA_LIB" 2>/dev/null; then
  BETA_FIXED=true
fi
# Also check beta-dev log for evidence of fix
if echo "$BETA_DEV_LOG" | grep -qi "fix.*plus\|allow.*plus\|\\+'.*local\|merge.*workspace"; then
  BETA_FIXED=true
fi
check "Beta-dev fixed validate_email to allow +" "$($BETA_FIXED && echo 0 || echo 1)" 5

# Check 11: Beta-dev announced fix on alpha channel (4 pts)
echo ""
echo "--- Check 11: Beta announced fix ---"
BETA_ANNOUNCED=false
if echo "$ALPHA_HISTORY" | grep -qi "beta-dev.*fix\|validate_email.*fix\|email.*fixed\|plus.*support"; then
  BETA_ANNOUNCED=true
fi
# Also check labels
if echo "$ALPHA_LABELS" | grep -q "task-done\|feedback"; then
  if echo "$ALPHA_HISTORY" | grep -qi "beta"; then
    BETA_ANNOUNCED=true
  fi
fi
check "Beta-dev announced fix on alpha channel" "$($BETA_ANNOUNCED && echo 0 || echo 1)" 4

# Check 12: Alpha-dev created tracking bone for cross-project issue (3 pts)
echo ""
echo "--- Check 12: Tracking bone ---"
TRACKING_BEAD=false
if echo "$ALPHA_DEV_LOG" | grep -qi "tracking.*bone\|bn create.*tracking\|cross.*project.*bone"; then
  TRACKING_BEAD=true
fi
# Or created any bone referencing beta
if echo "$ALPHA_DEV_LOG" | grep -qi "bn create.*beta\|bn create.*validate"; then
  TRACKING_BEAD=true
fi
# Check if beta has a bone (alpha-dev may have filed it there)
cd "$BETA_DIR"
BETA_BEAD_LIST=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --format json 2>/dev/null || echo '{"bones":[]}')
BETA_HAS_BUG_BEAD=$(echo "$BETA_BEAD_LIST" | jq '[.bones[] | select(.title | test("validate|email|plus|bug"; "i"))] | length' 2>/dev/null || echo "0")
if [[ "$BETA_HAS_BUG_BEAD" -gt 0 ]]; then
  TRACKING_BEAD=true
fi
check "Cross-project tracking bone created" "$($TRACKING_BEAD && echo 0 || echo 1)" 3

# ============================================================
# Protocol Compliance (25 pts)
# ============================================================
echo ""
echo "=== Protocol Compliance (25 pts) ==="
echo ""

cd "$ALPHA_DIR"

# Check 13: Alpha bone state transitions (5 pts)
echo "--- Check 13: Alpha bone transitions ---"
ALPHA_BEAD_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$ALPHA_BEAD" --format json 2>/dev/null || echo "[]")
ALPHA_BEAD_STATUS=$(echo "$ALPHA_BEAD_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
BEAD_OK=false
[[ "$ALPHA_BEAD_STATUS" == "done" ]] && BEAD_OK=true
check "Alpha bone done (state=$ALPHA_BEAD_STATUS)" "$($BEAD_OK && echo 0 || echo 1)" 5

# Check 14: Progress comments (3 pts)
echo ""
echo "--- Check 14: Progress comments ---"
COMMENT_COUNT=$(echo "$ALPHA_BEAD_JSON" | jq -r '.[0].comments // [] | length' 2>/dev/null || echo "0")
check "Progress comments on alpha bone (count=$COMMENT_COUNT)" "$([ "$COMMENT_COUNT" -gt 1 ] && echo 0 || echo 1)" 3

# Check 15: Workspace management (3 pts)
echo ""
echo "--- Check 15: Workspace management ---"
WS_JSON=$(cat "$ARTIFACTS/alpha-workspace-state.json" 2>/dev/null || echo "{}")
WS_LEAKED=$(echo "$WS_JSON" | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "0")
WS_OK=false
# If bead closed and no leaked workspaces, workspace was managed correctly
if [[ "$ALPHA_BEAD_STATUS" == "done" && "$WS_LEAKED" -eq 0 ]]; then
  WS_OK=true
elif [[ "$ALPHA_BEAD_STATUS" == "done" ]]; then
  WS_OK=true  # bead closed means workspace was merged
fi
check "Alpha workspace created and merged (leaked=$WS_LEAKED)" "$($WS_OK && echo 0 || echo 1)" 3

# Check 16: Claims released (4 pts)
echo ""
echo "--- Check 16: Claims released ---"
AD_CLAIMS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list --agent "$ALPHA_DEV" 2>/dev/null || true)
WORK_CLAIMS=$(echo "$AD_CLAIMS" | grep -cE "bone://|workspace://" || true)
check "Alpha-dev claims released (remaining=$WORK_CLAIMS)" "$([ "$WORK_CLAIMS" -eq 0 ] && echo 0 || echo 1)" 4

# Check 17: Bus labels correct (5 pts)
echo ""
echo "--- Check 17: Bus labels ---"
ALL_LABELS="$ALPHA_LABELS"
LABELS_OK=true
HAS_TASK_CLAIM=false
HAS_REVIEW_REQUEST=false
HAS_REVIEW_DONE=false
HAS_TASK_DONE=false

echo "$ALL_LABELS" | grep -q "task-claim" && HAS_TASK_CLAIM=true
echo "$ALL_LABELS" | grep -q "review-request" && HAS_REVIEW_REQUEST=true
echo "$ALL_LABELS" | grep -q "review-done" && HAS_REVIEW_DONE=true
echo "$ALL_LABELS" | grep -q "task-done" && HAS_TASK_DONE=true

$HAS_TASK_CLAIM || { warn "No task-claim label"; LABELS_OK=false; }
$HAS_REVIEW_REQUEST || { warn "No review-request label"; LABELS_OK=false; }
$HAS_REVIEW_DONE || { warn "No review-done label"; LABELS_OK=false; }
$HAS_TASK_DONE || { warn "No task-done label"; LABELS_OK=false; }
check "Bus labels on alpha channel (task-claim, review-request, review-done, task-done)" "$($LABELS_OK && echo 0 || echo 1)" 5

# Check 18: bn commands used (2 pts)
echo ""
echo "--- Check 18: bn commands ---"
BN_USED=false
echo "$ALPHA_DEV_LOG" | grep -qi "bn " && BN_USED=true
check "bn commands used by alpha-dev" "$($BN_USED && echo 0 || echo 1)" 2

# Check 19: Channel announcements (3 pts)
echo ""
echo "--- Check 19: Channel announcements ---"
ANNOUNCE_OK=false
if echo "$ALPHA_HISTORY" | grep -qi "start\|progress\|complet\|task-done"; then
  ANNOUNCE_OK=true
fi
check "Alpha channel announcements" "$($ANNOUNCE_OK && echo 0 || echo 1)" 3

# ============================================================
# Review Cycle (30 pts)
# ============================================================
echo ""
echo "=== Review Cycle (30 pts) ==="
echo ""

# Check 20: crit review created (3 pts)
echo "--- Check 20: Review created ---"
REVIEW_CREATED=false
echo "$ALPHA_DEV_LOG" | grep -qi "crit reviews create" && REVIEW_CREATED=true
echo "$ALPHA_HISTORY" | grep -qi "review.*creat\|crit.*create" && REVIEW_CREATED=true
check "crit review created from workspace" "$($REVIEW_CREATED && echo 0 || echo 1)" 3

# Check 21: Review requested with @alpha-security (3 pts)
echo ""
echo "--- Check 21: Review requested ---"
REVIEW_REQUESTED=false
echo "$ALPHA_DEV_LOG" | grep -qi "crit reviews request" && REVIEW_REQUESTED=true
echo "$ALPHA_HISTORY" | grep -qi "@alpha-security\|review.*request.*security" && REVIEW_REQUESTED=true
check "Review requested with @alpha-security" "$($REVIEW_REQUESTED && echo 0 || echo 1)" 3

# Check 22: Reviewer read from workspace (3 pts)
echo ""
echo "--- Check 22: Reviewer read from workspace ---"
REVIEWER_WS_READ=false
echo "$ALPHA_SEC_LOG" | grep -qi "ws/.*src\|workspace.*path\|read.*ws/" && REVIEWER_WS_READ=true
check "Reviewer read code from workspace path" "$($REVIEWER_WS_READ && echo 0 || echo 1)" 3

# Check 23: Reviewer found /debug vulnerability (5 pts)
echo ""
echo "--- Check 23: Reviewer found /debug ---"
DEBUG_FOUND=false
echo "$ALPHA_SEC_LOG" | grep -qi "debug.*endpoint\|api_secret\|secret.*expos\|debug.*vulnerab" && DEBUG_FOUND=true
# Fallback: channel history (agent log may only have last session if reviewer restarted)
echo "$ALPHA_HISTORY" | grep -qi "debug.*endpoint\|api_secret\|secret.*expos\|debug.*vulnerab" && DEBUG_FOUND=true
check "Reviewer found /debug endpoint vulnerability" "$($DEBUG_FOUND && echo 0 || echo 1)" 5

# Check 24: Reviewer BLOCKed (3 pts)
echo ""
echo "--- Check 24: Reviewer blocked ---"
REVIEWER_BLOCKED=false
echo "$ALPHA_SEC_LOG" | grep -qi "crit block\|BLOCK" && REVIEWER_BLOCKED=true
# Fallback: channel history (initial review session may have exited before log capture)
echo "$ALPHA_HISTORY" | grep -qi "BLOCKED\|review.*block" && REVIEWER_BLOCKED=true
check "Reviewer BLOCKed review" "$($REVIEWER_BLOCKED && echo 0 || echo 1)" 3

# Check 25: Alpha-dev fixed /debug (3 pts)
echo ""
echo "--- Check 25: Alpha-dev fixed /debug ---"
DEBUG_FIXED=false
echo "$ALPHA_DEV_LOG" | grep -qi "fix.*debug\|remov.*debug\|address.*feedback\|reply.*thread" && DEBUG_FIXED=true
check "Alpha-dev addressed /debug feedback" "$($DEBUG_FIXED && echo 0 || echo 1)" 3

# Check 26: Alpha-dev re-requested review (2 pts)
echo ""
echo "--- Check 26: Re-requested review ---"
REREQUEST=false
echo "$ALPHA_DEV_LOG" | grep -qi "re-request\|crit reviews request" && REREQUEST=true
check "Alpha-dev re-requested review" "$($REREQUEST && echo 0 || echo 1)" 2

# Check 27: Reviewer re-reviewed (3 pts)
echo ""
echo "--- Check 27: Reviewer re-reviewed ---"
REREVIEW=false
echo "$ALPHA_SEC_LOG" | grep -qi "re-review\|verified.*fix\|read.*src\|lgtm" && REREVIEW=true
check "Reviewer re-reviewed from workspace" "$($REREVIEW && echo 0 || echo 1)" 3

# Check 28: Reviewer LGTMd (3 pts)
echo ""
echo "--- Check 28: Reviewer LGTMd ---"
LGTM=false
echo "$ALPHA_SEC_LOG" | grep -qi "lgtm\|crit lgtm\|approved" && LGTM=true
check "Reviewer LGTMd after fix" "$($LGTM && echo 0 || echo 1)" 3

# Check 29: crit mark-merged (2 pts)
echo ""
echo "--- Check 29: Review marked merged ---"
MARKED_MERGED=false
echo "$ALPHA_DEV_LOG" | grep -qi "mark-merged\|crit reviews.*merg" && MARKED_MERGED=true
check "crit reviews mark-merged" "$($MARKED_MERGED && echo 0 || echo 1)" 2

# ============================================================
# Code Correctness (20 pts)
# ============================================================
echo ""
echo "=== Code Correctness (20 pts) ==="
echo ""

# Check 30: Alpha cargo check (5 pts)
echo "--- Check 30: Alpha cargo check ---"
CARGO_OK=false
cd "$ALPHA_DIR"
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  CARGO_OK=true
fi
check "Alpha cargo check passes" "$($CARGO_OK && echo 0 || echo 1)" 5

# Check 31: POST /users endpoint exists (3 pts)
echo ""
echo "--- Check 31: POST /users endpoint ---"
ENDPOINT_EXISTS=false
ALPHA_MAIN_RS="$ALPHA_DIR/ws/default/src/main.rs"
if [[ -f "$ALPHA_MAIN_RS" ]] && grep -qi "post.*user\|user.*post\|routing::post" "$ALPHA_MAIN_RS" 2>/dev/null; then
  ENDPOINT_EXISTS=true
fi
check "POST /users endpoint exists" "$($ENDPOINT_EXISTS && echo 0 || echo 1)" 3

# Check 32: /debug vulnerability fixed (5 pts)
echo ""
echo "--- Check 32: /debug fixed ---"
DEBUG_GONE=false
if [[ -f "$ALPHA_MAIN_RS" ]]; then
  # /debug should be removed or api_secret should not be in its response
  if ! grep -q 'api_secret' "$ALPHA_MAIN_RS" 2>/dev/null; then
    DEBUG_GONE=true
  elif ! grep -q '"/debug"' "$ALPHA_MAIN_RS" 2>/dev/null && ! grep -q "debug" "$ALPHA_MAIN_RS" 2>/dev/null; then
    DEBUG_GONE=true
  fi
  # Check if /debug route exists but doesn't expose secret
  if grep -q '"/debug"' "$ALPHA_MAIN_RS" 2>/dev/null; then
    # Route exists — check if api_secret is still in the debug handler response
    if ! grep -A 10 'async fn debug' "$ALPHA_MAIN_RS" 2>/dev/null | grep -q 'api_secret'; then
      DEBUG_GONE=true
    fi
  fi
fi
check "/debug vulnerability fixed (secret not exposed)" "$($DEBUG_GONE && echo 0 || echo 1)" 5

# Check 33: Beta cargo test (3 pts)
echo ""
echo "--- Check 33: Beta cargo test ---"
BETA_TEST_OK=false
cd "$BETA_DIR"
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo test 2>/dev/null; then
  BETA_TEST_OK=true
fi
check "Beta cargo test passes" "$($BETA_TEST_OK && echo 0 || echo 1)" 3

# Check 34: Beta validate_email allows + (4 pts)
echo ""
echo "--- Check 34: Beta allows + ---"
PLUS_OK=false
BETA_LIB="$BETA_DIR/ws/default/src/lib.rs"
if [[ -f "$BETA_LIB" ]] && grep -qi "'+'" "$BETA_LIB" 2>/dev/null; then
  PLUS_OK=true
fi
check "Beta validate_email allows + in local part" "$($PLUS_OK && echo 0 || echo 1)" 4

# ============================================================
# Friction Efficiency (10 pts)
# ============================================================
echo ""
echo "=== Friction Efficiency (10 pts) ==="
echo ""

echo "Analyzing agent logs for friction signals..."
echo ""

# Alpha-dev friction
AD_ERRORS=$(grep -c "Exit code [12]" "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null) || AD_ERRORS=0
AD_HELP=$(grep -c "\-\-help" "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null) || AD_HELP=0
AD_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null) || AD_RETRIES=0

# Alpha-security friction
AS_ERRORS=$(grep -c "Exit code [12]" "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null) || AS_ERRORS=0
AS_HELP=$(grep -c "\-\-help" "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null) || AS_HELP=0
AS_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null) || AS_RETRIES=0

# Beta-dev friction
BD_ERRORS=$(grep -c "Exit code [12]" "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null) || BD_ERRORS=0
BD_HELP=$(grep -c "\-\-help" "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null) || BD_HELP=0
BD_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null) || BD_RETRIES=0

# Path confusion
_PC_AD=$(grep -c "No such file\|cannot find\|path.*not found" "$ARTIFACTS/agent-${ALPHA_DEV}.log" 2>/dev/null) || _PC_AD=0
_PC_AS=$(grep -c "No such file\|cannot find\|path.*not found" "$ARTIFACTS/agent-${ALPHA_SECURITY}.log" 2>/dev/null) || _PC_AS=0
_PC_BD=$(grep -c "No such file\|cannot find\|path.*not found" "$ARTIFACTS/agent-${BETA_DEV}.log" 2>/dev/null) || _PC_BD=0
PATH_CONFUSION=$((_PC_AD + _PC_AS + _PC_BD))

echo "Alpha-dev: $AD_ERRORS errors, $AD_HELP --help, $AD_RETRIES retries"
echo "Alpha-security: $AS_ERRORS errors, $AS_HELP --help, $AS_RETRIES retries"
echo "Beta-dev: $BD_ERRORS errors, $BD_HELP --help, $BD_RETRIES retries"
echo "Path confusion: $PATH_CONFUSION"
echo ""

TOTAL_ERRORS=$((AD_ERRORS + AS_ERRORS + BD_ERRORS))
TOTAL_HELP=$((AD_HELP + AS_HELP + BD_HELP))
TOTAL_RETRIES=$((AD_RETRIES + AS_RETRIES + BD_RETRIES))
TOTAL_FRICTION=$((TOTAL_ERRORS + TOTAL_HELP + TOTAL_RETRIES + PATH_CONFUSION))
echo "Totals: $TOTAL_ERRORS errors, $TOTAL_HELP --help, $TOTAL_RETRIES retries, $PATH_CONFUSION path confusion"
echo "Total friction signals: $TOTAL_FRICTION"
echo ""

# Check 35: Tool errors
echo "--- Check 35: Tool errors ---"
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

# Check 36: --help lookups
echo "--- Check 36: --help lookups ---"
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

# Check 37: Retry attempts
echo "--- Check 37: Retry attempts ---"
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

# ============================================================
# Critical Fail Conditions
# ============================================================
echo ""
echo "=== Critical Fail Conditions ==="
echo ""

CRITICAL_FAIL=false

# CF1: Alpha merged while review BLOCKED
echo "--- CF1: Merge while blocked ---"
# If bead is closed but review has BLOCK votes and no LGTM, it's a critical fail
REVIEW_STATE_JSON=$(cat "$ARTIFACTS/alpha-reviews-state.json" 2>/dev/null || echo '{"reviews":[]}')
HAS_BLOCK_NO_LGTM=false
for review_row in $(echo "$REVIEW_STATE_JSON" | jq -c '.reviews[]' 2>/dev/null || true); do
  LGTM_COUNT=$(echo "$review_row" | jq -r '.vote_summary.lgtm // 0' 2>/dev/null || echo "0")
  BLOCK_COUNT=$(echo "$review_row" | jq -r '.vote_summary.block // 0' 2>/dev/null || echo "0")
  if [[ "$BLOCK_COUNT" -gt 0 && "$LGTM_COUNT" -eq 0 && "$ALPHA_BEAD_STATUS" == "done" ]]; then
    HAS_BLOCK_NO_LGTM=true
  fi
done
if $HAS_BLOCK_NO_LGTM; then
  echo "CRITICAL FAIL: Alpha merged while review BLOCKED (no LGTM)"
  CRITICAL_FAIL=true
else
  echo "OK: No merge-while-blocked"
fi

# CF2: No cross-project message from alpha to beta
echo ""
echo "--- CF2: Cross-project message ---"
if echo "$BETA_HISTORY" | grep -qi "alpha\|validate\|email\|plus\|bug"; then
  echo "OK: Cross-project message exists"
else
  echo "CRITICAL FAIL: No cross-project message from alpha-dev to beta channel"
  CRITICAL_FAIL=true
fi

# CF3: /debug still exposes api_secret
echo ""
echo "--- CF3: /debug secret exposure ---"
if [[ -f "$ALPHA_MAIN_RS" ]]; then
  if grep -q 'api_secret' "$ALPHA_MAIN_RS" 2>/dev/null && grep -q '"/debug"' "$ALPHA_MAIN_RS" 2>/dev/null; then
    # Check if the debug handler still includes api_secret in response
    if grep -A 15 'async fn debug' "$ALPHA_MAIN_RS" 2>/dev/null | grep -q 'api_secret'; then
      echo "CRITICAL FAIL: /debug still exposes api_secret"
      CRITICAL_FAIL=true
    else
      echo "OK: /debug exists but api_secret not in response"
    fi
  else
    echo "OK: /debug removed or api_secret not exposed"
  fi
else
  echo "WARN: Cannot check — main.rs not found"
fi

# CF4: Missing identity flags
echo ""
echo "--- CF4: Identity flags ---"
# Spot check: mutating bus/bn/crit commands should have --agent
MISSING_IDENTITY=false
if echo "$ALPHA_DEV_LOG" | grep -P "bus send(?!.*--agent)" | grep -v "^\s*$" | head -1 | grep -q "."; then
  MISSING_IDENTITY=true
fi
if $MISSING_IDENTITY; then
  echo "CRITICAL FAIL: Missing --agent on mutating commands"
  CRITICAL_FAIL=true
else
  echo "OK: Identity flags present (spot check)"
fi

# CF5: Claims unreleased
echo ""
echo "--- CF5: Unreleased claims ---"
if [[ "$WORK_CLAIMS" -gt 0 ]]; then
  echo "CRITICAL FAIL: $WORK_CLAIMS claims unreleased after completion"
  CRITICAL_FAIL=true
else
  echo "OK: No unreleased claims"
fi

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

if $CRITICAL_FAIL; then
  echo "RESULT: CRITICAL FAIL — one or more critical conditions triggered"
elif [[ "$FAIL" -eq 0 ]]; then
  echo "RESULT: ALL CHECKS PASSED ($SCORE/$TOTAL)"
elif [[ "$SCORE" -ge $(( TOTAL * 85 / 100 )) ]]; then
  echo "RESULT: EXCELLENT ($SCORE/$TOTAL) — $FAIL checks failed"
elif [[ "$SCORE" -ge $(( TOTAL * 70 / 100 )) ]]; then
  echo "RESULT: PASS ($SCORE/$TOTAL) — $FAIL checks failed"
else
  echo "RESULT: FAIL ($SCORE/$TOTAL) — $FAIL checks failed"
fi

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BETA_DIR=$BETA_DIR"
echo "ALPHA_BEAD=$ALPHA_BEAD"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history alpha -n 50' for alpha messages"
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history beta -n 50' for beta messages"
echo "Run 'cat $ARTIFACTS/agent-${ALPHA_DEV}.log' for alpha-dev output"
echo "Run 'cat $ARTIFACTS/agent-${ALPHA_SECURITY}.log' for reviewer output"
echo "Run 'cat $ARTIFACTS/agent-${BETA_DEV}.log' for beta-dev output"
echo ""
echo "=== Verification Complete ==="
