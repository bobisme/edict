#!/usr/bin/env bash
set -euo pipefail

# E11-L4 Verification Script
# Automated scoring for the mission eval.
# Checks mission recognition, decomposition, worker dispatch,
# monitoring, synthesis, code correctness, and friction.

source "${1:?Usage: e11-l4-verify.sh <path-to-.eval-env>}"

echo "=== E11-L4 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "FUTIL_DEV=$FUTIL_DEV"
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
DEV_LOG=$(cat "$ARTIFACTS/agent-${FUTIL_DEV}.log" 2>/dev/null || echo "")
CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-futil-history.log" 2>/dev/null || echo "")
FINAL_STATUS=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
CHANNEL_JSON=$(cat "$ARTIFACTS/channel-futil-history.json" 2>/dev/null || echo '{"messages":[]}')
CHANNEL_LABELS=$(echo "$CHANNEL_JSON" | jq -r '[.messages[].labels // [] | .[]] | .[]' 2>/dev/null || echo "")

# Extract mission bone from final status
MISSION_BONE=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_BONE=\K[^ ]+' || echo "none")
CHILD_COUNT_FINAL=$(echo "$FINAL_STATUS" | grep -oP 'CHILD_COUNT=\K\d+' || echo "0")
CHILDREN_CLOSED_FINAL=$(echo "$FINAL_STATUS" | grep -oP 'CHILDREN_CLOSED=\K\d+' || echo "0")

cd "$PROJECT_DIR"

# ============================================================
# Critical Fail: Mission never created
# ============================================================
echo "=== Critical Fail Check ==="
echo ""
if [[ "$MISSION_BONE" == "none" || -z "$MISSION_BONE" ]]; then
  echo "CRITICAL FAIL: Mission bone was never created"
  echo ""
  echo "SCORE: 0 / 0 (critical fail)"
  echo "RESULT: CRITICAL FAIL — mission never created"
  exit 0
fi
echo "Mission bone: $MISSION_BONE"
echo ""

# ============================================================
# Mission Recognition (15 pts)
# ============================================================
echo "=== Mission Recognition (15 pts) ==="
echo ""

# Check 1: Bead with mission label (5 pts)
echo "--- Check 1: Mission bone with label ---"
MISSION_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$MISSION_BONE" --format json 2>/dev/null || echo "[]")
HAS_MISSION_LABEL=false
if echo "$MISSION_JSON" | jq -r '.[0].labels // [] | .[]' 2>/dev/null | grep -q "mission"; then
  HAS_MISSION_LABEL=true
fi
check "Mission bone has 'mission' label" "$($HAS_MISSION_LABEL && echo 0 || echo 1)" 5

# Check 2: Structured description (5 pts)
echo ""
echo "--- Check 2: Structured description ---"
MISSION_DESC=$(echo "$MISSION_JSON" | jq -r '.[0].description // ""' 2>/dev/null || echo "")
HAS_OUTCOME=false
if echo "$MISSION_DESC" | grep -qi "outcome\|success.*metric\|constraints\|stop.*crit"; then
  HAS_OUTCOME=true
fi
check "Mission bone has structured description (Outcome/Success/Constraints)" "$($HAS_OUTCOME && echo 0 || echo 1)" 5

# Check 3: Dev-loop identified mission context (5 pts)
echo ""
echo "--- Check 3: Dev-loop identified mission ---"
DEV_MISSION_CTX=false
if echo "$DEV_LOG" | grep -qi "BOTBOX_MISSION\|mission.*${MISSION_BONE}\|Level 4\|mission.*decompos"; then
  DEV_MISSION_CTX=true
fi
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*${MISSION_BONE}\|mission.*creat"; then
  DEV_MISSION_CTX=true
fi
check "Dev-loop identified mission context" "$($DEV_MISSION_CTX && echo 0 || echo 1)" 5

# ============================================================
# Decomposition (25 pts)
# ============================================================
echo ""
echo "=== Decomposition (25 pts) ==="
echo ""

# Get children
CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_BONE" --format json 2>/dev/null || echo '[]')
# Normalize JSON shape
ACTUAL_CHILD_COUNT=$(echo "$CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")

# Check 4: 3+ children (5 pts)
echo "--- Check 4: Child bone count ---"
check "3+ child bones created (actual=$ACTUAL_CHILD_COUNT)" "$([ "$ACTUAL_CHILD_COUNT" -ge 3 ] && echo 0 || echo 1)" 5

# Check 5: mission:<id> labels (5 pts)
echo ""
echo "--- Check 5: Mission labels on children ---"
LABELED_COUNT=0
CHILD_IDS=$(echo "$CHILDREN_JSON" | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null || echo "")
for cid in $CHILD_IDS; do
  CHILD_LABELS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].labels // [] | .[]' 2>/dev/null || echo "")
  if echo "$CHILD_LABELS" | grep -q "mission:"; then
    LABELED_COUNT=$((LABELED_COUNT + 1))
  fi
done
check "Children have mission:<id> labels ($LABELED_COUNT/$ACTUAL_CHILD_COUNT)" "$([ "$LABELED_COUNT" -ge 3 ] && echo 0 || echo 1)" 5

# Check 6: Parent dependencies (5 pts)
echo ""
echo "--- Check 6: Parent dependencies ---"
HAS_PARENT_DEP=false
# Check if children have deps wired (any dep structure)
if echo "$DEV_LOG" | grep -qi "bn triage dep add\|dep.*add"; then
  HAS_PARENT_DEP=true
fi
# Also check bones directly
for cid in $CHILD_IDS; do
  CHILD_DEPS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].dependencies // [] | length' 2>/dev/null || echo "0")
  if [[ "$CHILD_DEPS" -gt 0 ]]; then
    HAS_PARENT_DEP=true
    break
  fi
done
check "Dependencies wired between children or to parent" "$($HAS_PARENT_DEP && echo 0 || echo 1)" 5

# Check 7: Inter-child dependency (5 pts)
echo ""
echo "--- Check 7: Inter-child dependency ---"
HAS_INTER_DEP=false
# Check if any child blocks another child
DEP_ADD_COUNT=$(echo "$DEV_LOG" | grep -ci "bn triage dep add" 2>/dev/null) || DEP_ADD_COUNT=0
if [[ "$DEP_ADD_COUNT" -ge 1 ]]; then
  HAS_INTER_DEP=true
fi
check "Inter-child dependency exists (dep add count=$DEP_ADD_COUNT)" "$($HAS_INTER_DEP && echo 0 || echo 1)" 5

# Check 8: Clear titles (5 pts)
# Requires children to exist — vacuous pass is wrong
echo ""
echo "--- Check 8: Clear child titles ---"
CLEAR_TITLES=false
if [[ "$ACTUAL_CHILD_COUNT" -gt 0 ]]; then
  CLEAR_TITLES=true
  for cid in $CHILD_IDS; do
    CHILD_TITLE=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].title // ""' 2>/dev/null || echo "")
    if [[ ${#CHILD_TITLE} -lt 5 ]]; then
      CLEAR_TITLES=false
      warn "Child $cid has unclear title: '$CHILD_TITLE'"
    fi
  done
fi
check "Child bones have clear titles" "$($CLEAR_TITLES && echo 0 || echo 1)" 5

# ============================================================
# Worker Dispatch (25 pts)
# ============================================================
echo ""
echo "=== Worker Dispatch (25 pts) ==="
echo ""

# Extract worker info from final status
# Worker names can be on separate lines after "WORKER_NAMES:" header
WORKER_NAMES=$(echo "$FINAL_STATUS" | sed -n '/^WORKER_NAMES/,$ p' | tail -n +2 | grep -v '^$' || echo "")
WORKER_COUNT=0
if [[ -n "$WORKER_NAMES" ]]; then
  WORKER_COUNT=$(echo "$WORKER_NAMES" | wc -l)
fi

# Check 9: Workers spawned (5 pts)
# Only count actual hierarchical workers (name contains /), not the dev agent itself
echo "--- Check 9: Workers spawned ---"
WORKERS_SPAWNED=false
if [[ "$WORKER_COUNT" -ge 1 ]]; then
  WORKERS_SPAWNED=true
fi
# Check dev log for vessel spawn with hierarchical name pattern (agent/worker)
if echo "$DEV_LOG" | grep -i "vessel spawn" | grep -q "/"; then
  WORKERS_SPAWNED=true
fi
check "Workers spawned ($WORKER_COUNT discovered)" "$($WORKERS_SPAWNED && echo 0 || echo 1)" 5

# Critical fail: no workers → cap score at 30%
if ! $WORKERS_SPAWNED; then
  echo ""
  echo "CRITICAL: No workers spawned — capping total score at 30%"
  echo ""
fi

# Check 10: 2+ workers (5 pts)
echo ""
echo "--- Check 10: Multiple workers ---"
check "2+ workers spawned ($WORKER_COUNT)" "$([ "$WORKER_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 11: Workspace per worker (5 pts)
# Must create workspaces specifically for workers, not just for the dev agent's own work.
# Look for 2+ workspace creations (one for dev is normal, 2+ means workers got workspaces).
echo ""
echo "--- Check 11: Workspace per worker ---"
WS_CREATE_COUNT=$(echo "$DEV_LOG" | grep -ci "maw ws create" 2>/dev/null) || WS_CREATE_COUNT=0
WORKER_WS=false
if [[ "$WS_CREATE_COUNT" -ge 2 ]]; then
  WORKER_WS=true
fi
check "Workspaces created for workers ($WS_CREATE_COUNT ws creates)" "$($WORKER_WS && echo 0 || echo 1)" 5

# Check 12: Mission env vars (5 pts)
echo ""
echo "--- Check 12: Mission env vars ---"
HAS_MISSION_ENV=false
if echo "$DEV_LOG" | grep -qi "BOTBOX_MISSION\|BOTBOX_SIBLINGS\|BOTBOX_MISSION_OUTCOME"; then
  HAS_MISSION_ENV=true
fi
# Also check channel history and bone comments for mission context
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*context\|mission.*bn-"; then
  HAS_MISSION_ENV=true
fi
# Check if workers received mission context via bone comments
for cid in $CHILD_IDS; do
  CHILD_COMMENTS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn bone comment list "$cid" 2>/dev/null || echo "")
  if echo "$CHILD_COMMENTS" | grep -qi "mission.*context\|BOTBOX_MISSION"; then
    HAS_MISSION_ENV=true
    break
  fi
done
check "Mission env vars set on workers (BOTBOX_MISSION, etc.)" "$($HAS_MISSION_ENV && echo 0 || echo 1)" 5

# Check 13: Claims staked for workers (5 pts)
# Must have 2+ claim stake calls (dev stakes its own, workers need additional claims)
echo ""
echo "--- Check 13: Claims staked for workers ---"
CLAIM_COUNT=$(echo "$DEV_LOG" | grep -ci "bus claims stake" 2>/dev/null) || CLAIM_COUNT=0
HAS_WORKER_CLAIMS=false
if [[ "$CLAIM_COUNT" -ge 3 ]]; then
  # Dev stakes 1-2 for itself (bone + workspace), workers need more
  HAS_WORKER_CLAIMS=true
fi
check "Claims staked for workers ($CLAIM_COUNT total stakes)" "$($HAS_WORKER_CLAIMS && echo 0 || echo 1)" 5

# ============================================================
# Monitoring (15 pts)
# ============================================================
echo ""
echo "=== Monitoring (15 pts) ==="
echo ""

# Check 14: Checkpoint message (5 pts)
echo "--- Check 14: Checkpoint message ---"
HAS_CHECKPOINT=false
if echo "$CHANNEL_HISTORY" | grep -qi "checkpoint\|mission.*done\|active\|progress.*mission"; then
  HAS_CHECKPOINT=true
fi
if echo "$DEV_LOG" | grep -qi "checkpoint"; then
  HAS_CHECKPOINT=true
fi
check "Checkpoint message posted" "$($HAS_CHECKPOINT && echo 0 || echo 1)" 5

# Check 15: Count/status info (5 pts)
echo ""
echo "--- Check 15: Count/status info ---"
HAS_COUNT_INFO=false
if echo "$CHANNEL_HISTORY" | grep -qiE "[0-9]+.*done|[0-9]+.*closed|[0-9]+.*active|[0-9]+/[0-9]+"; then
  HAS_COUNT_INFO=true
fi
if echo "$DEV_LOG" | grep -qiE "children.*closed|children.*status|[0-9]+.*done.*[0-9]+.*total"; then
  HAS_COUNT_INFO=true
fi
check "Count/status info in checkpoint" "$($HAS_COUNT_INFO && echo 0 || echo 1)" 5

# Check 16: Worker completion detected (5 pts)
# Must detect actual worker completion — not just the dev closing its own work.
# Look for patterns specific to monitoring workers: "worker.*exit", "child.*closed", merge of worker workspace.
echo ""
echo "--- Check 16: Worker completion detected ---"
COMPLETION_DETECTED=false
if echo "$DEV_LOG" | grep -qi "worker.*finish\|worker.*exit\|child.*closed\|maw ws merge.*--destroy"; then
  # "maw ws merge --destroy" with 2+ occurrences indicates worker workspace merges
  MERGE_COUNT=$(echo "$DEV_LOG" | grep -ci "maw ws merge" 2>/dev/null) || MERGE_COUNT=0
  if [[ "$MERGE_COUNT" -ge 2 ]]; then
    COMPLETION_DETECTED=true
  fi
fi
if echo "$DEV_LOG" | grep -qi "worker.*complet\|child.*done"; then
  COMPLETION_DETECTED=true
fi
check "Worker completion detected" "$($COMPLETION_DETECTED && echo 0 || echo 1)" 5

# ============================================================
# Synthesis (15 pts)
# ============================================================
echo ""
echo "=== Synthesis (15 pts) ==="
echo ""

# Check 17: All children closed (5 pts)
echo "--- Check 17: All children closed ---"
ALL_CHILDREN_CLOSED=false
CLOSED_COUNT=0
if [[ "$ACTUAL_CHILD_COUNT" -gt 0 ]]; then
  CLOSED_COUNT=$(echo "$CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")
  if [[ "$CLOSED_COUNT" -ge "$ACTUAL_CHILD_COUNT" ]]; then
    ALL_CHILDREN_CLOSED=true
  fi
  echo "  Children closed: $CLOSED_COUNT/$ACTUAL_CHILD_COUNT"
fi
check "All children closed ($CLOSED_COUNT/$ACTUAL_CHILD_COUNT)" "$($ALL_CHILDREN_CLOSED && echo 0 || echo 1)" 5

# Check 18: Mission bone closed (5 pts)
echo ""
echo "--- Check 18: Mission bone closed ---"
MISSION_STATUS=$(echo "$MISSION_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
MISSION_CLOSED=false
[[ "$MISSION_STATUS" == "done" ]] && MISSION_CLOSED=true
check "Mission bone closed (status=$MISSION_STATUS)" "$($MISSION_CLOSED && echo 0 || echo 1)" 5

# Check 19: Synthesis comment (5 pts)
echo ""
echo "--- Check 19: Synthesis comment ---"
HAS_SYNTHESIS=false
MISSION_COMMENTS=$(echo "$MISSION_JSON" | jq -r '.[0].comments // [] | .[].body // .[].content // empty' 2>/dev/null || echo "")
if echo "$MISSION_COMMENTS" | grep -qi "mission.*complete\|synthesis\|children.*closed\|all.*done\|key.*decision\|what.*worked"; then
  HAS_SYNTHESIS=true
fi
# Also check channel for synthesis announcement
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*complete\|all.*children.*done"; then
  HAS_SYNTHESIS=true
fi
check "Synthesis comment on mission bone" "$($HAS_SYNTHESIS && echo 0 || echo 1)" 5

# ============================================================
# Code Correctness (20 pts)
# ============================================================
echo ""
echo "=== Code Correctness (20 pts) ==="
echo ""

# Check 20: cargo check (5 pts)
echo "--- Check 20: cargo check ---"
CARGO_OK=false
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  CARGO_OK=true
fi
check "cargo check passes" "$($CARGO_OK && echo 0 || echo 1)" 5

# Check 21: 2+ subcommands implemented (5 pts)
# Module files exist as stubs — check that todo!() was REMOVED (i.e., actually implemented)
echo ""
echo "--- Check 21: Subcommands implemented ---"
IMPL_COUNT=0
for mod_name in stats search convert; do
  MOD_FILE="$PROJECT_DIR/ws/default/src/${mod_name}.rs"
  if [[ -f "$MOD_FILE" ]] && ! grep -q 'todo!' "$MOD_FILE" 2>/dev/null; then
    IMPL_COUNT=$((IMPL_COUNT + 1))
  fi
done
check "2+ subcommands implemented (found=$IMPL_COUNT)" "$([ "$IMPL_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 22: Shared error module implemented (5 pts)
# error.rs has stubs for validate_file, detect_format, write_output — check they're implemented
echo ""
echo "--- Check 22: Shared error module implemented ---"
ERROR_RS="$PROJECT_DIR/ws/default/src/error.rs"
HELPER_COUNT=0
if [[ -f "$ERROR_RS" ]] && ! grep -q 'todo!' "$ERROR_RS" 2>/dev/null; then
  # Check for actual implementations of the three helpers
  grep -q 'fn validate_file\|fn read_file\|fn load_file' "$ERROR_RS" 2>/dev/null && HELPER_COUNT=$((HELPER_COUNT + 1))
  grep -q 'fn detect_format\|fn guess_format\|fn infer_format' "$ERROR_RS" 2>/dev/null && HELPER_COUNT=$((HELPER_COUNT + 1))
  grep -q 'fn write_output\|fn output_to' "$ERROR_RS" 2>/dev/null && HELPER_COUNT=$((HELPER_COUNT + 1))
fi
check "Shared error module helpers implemented ($HELPER_COUNT/3)" "$([ "$HELPER_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 23: Subcommands work on sample data (5 pts)
echo ""
echo "--- Check 23: Subcommands work on sample data ---"
WORKING_COUNT=0
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo build --quiet 2>/dev/null; then
  # Try stats on sample.txt
  STATS_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- stats data/sample.txt 2>/dev/null || echo "")
  if echo "$STATS_OUT" | grep -qiE "line|word|byte|[0-9]+"; then
    WORKING_COUNT=$((WORKING_COUNT + 1))
  fi
  # Try search
  SEARCH_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- search "Hello" data/sample.txt 2>/dev/null || echo "")
  if echo "$SEARCH_OUT" | grep -qi "Hello"; then
    WORKING_COUNT=$((WORKING_COUNT + 1))
  fi
  # Try convert
  CONVERT_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- convert data/sample.json --format csv 2>/dev/null || echo "")
  if echo "$CONVERT_OUT" | grep -qi "Alice"; then
    WORKING_COUNT=$((WORKING_COUNT + 1))
  fi
fi
check "Subcommands working on sample data ($WORKING_COUNT/3)" "$([ "$WORKING_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 23b: Flags work (5 pts) — bonus for expanded feature set
echo ""
echo "--- Check 23b: Feature flags work ---"
FLAGS_WORKING=0
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo build --quiet 2>/dev/null; then
  # stats --json
  SJ=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- stats --json data/sample.txt 2>/dev/null || echo "")
  echo "$SJ" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null && FLAGS_WORKING=$((FLAGS_WORKING + 1))
  # search -c (count mode)
  SC=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- search -c "the" data/words.txt 2>/dev/null || echo "")
  echo "$SC" | grep -qE "[0-9]+" 2>/dev/null && FLAGS_WORKING=$((FLAGS_WORKING + 1))
  # search -i (case insensitive)
  SI=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- search -i "HELLO" data/sample.txt 2>/dev/null || echo "")
  echo "$SI" | grep -qi "Hello" 2>/dev/null && FLAGS_WORKING=$((FLAGS_WORKING + 1))
  # convert --fields
  CF=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- convert data/sample.json --format csv --fields name,age 2>/dev/null || echo "")
  echo "$CF" | grep -qi "Alice" 2>/dev/null && FLAGS_WORKING=$((FLAGS_WORKING + 1))
fi
TOTAL=$((TOTAL + 5))
if [[ "$FLAGS_WORKING" -ge 3 ]]; then
  echo "PASS (5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$FLAGS_WORKING" -ge 2 ]]; then
  echo "PARTIAL (3/5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$FLAGS_WORKING" -ge 1 ]]; then
  echo "PARTIAL (1/5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): No feature flags work ($FLAGS_WORKING/4)"
  FAIL=$((FAIL + 1))
fi

# ============================================================
# Friction Efficiency (10 pts)
# ============================================================
echo ""
echo "=== Friction Efficiency (10 pts) ==="
echo ""

echo "Analyzing agent logs for friction signals..."
echo ""

# Count tool errors (friction) vs intentional test errors.
# Agents intentionally run the project CLI with bad inputs to verify error handling.
# Those exit code 1s are NOT friction. Real friction = tool_use_error + exit codes from
# tool commands (bus/br/maw/vessel/jj/cargo build/test/check), NOT from running the project binary.
_count_tool_errors() {
  local log="$1"
  local count=0
  # tool_use_error (e.g., file not read yet, sibling errored)
  local tue; tue=$(grep -c "tool_use_error" "$log" 2>/dev/null) || tue=0
  count=$((count + tue))
  # Exit codes: check the preceding command line to classify.
  # Skip if the command runs the project binary (cargo run, cd path && cargo run, ./target/).
  # Commands are on lines like: > Bash {"command":"...",...}
  # Log lines can be truncated, so "cd /long/path && cargo run" may show as just "cd /long/path..."
  local ec; ec=$(awk '
    /Exit code [12][^0-9]/ || /Exit code [12]$/ {
      # Skip intentional project CLI testing (cargo run, cd && cargo run, ./target/)
      if (prev ~ /cargo run/ || prev ~ /\.\/target\// || (prev ~ /^> Bash/ && prev ~ /"command":"cd /)) next
      count++
    }
    { prev = $0 }
    END { print count+0 }
  ' "$log" 2>/dev/null) || ec=0
  count=$((count + ec))
  echo "$count"
}

# Dev + worker friction
DEV_ERRORS=$(_count_tool_errors "$ARTIFACTS/agent-${FUTIL_DEV}.log")
DEV_HELP=$(grep -c "\-\-help" "$ARTIFACTS/agent-${FUTIL_DEV}.log" 2>/dev/null) || DEV_HELP=0
DEV_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${FUTIL_DEV}.log" 2>/dev/null) || DEV_RETRIES=0

WORKER_ERRORS=0
WORKER_HELP=0
WORKER_RETRIES=0
for wlog in "$ARTIFACTS"/agent-${FUTIL_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  WE=$(_count_tool_errors "$wlog")
  WH=$(grep -c "\-\-help" "$wlog" 2>/dev/null) || WH=0
  WR=$(grep -c "retry\|again\|Retrying" "$wlog" 2>/dev/null) || WR=0
  WORKER_ERRORS=$((WORKER_ERRORS + WE))
  WORKER_HELP=$((WORKER_HELP + WH))
  WORKER_RETRIES=$((WORKER_RETRIES + WR))
done

TOTAL_ERRORS=$((DEV_ERRORS + WORKER_ERRORS))
TOTAL_HELP=$((DEV_HELP + WORKER_HELP))
TOTAL_RETRIES=$((DEV_RETRIES + WORKER_RETRIES))

echo "Dev: $DEV_ERRORS errors, $DEV_HELP --help, $DEV_RETRIES retries"
echo "Workers: $WORKER_ERRORS errors, $WORKER_HELP --help, $WORKER_RETRIES retries"
echo "Total: $TOTAL_ERRORS errors, $TOTAL_HELP --help, $TOTAL_RETRIES retries"
echo ""

# Check 24: Tool errors (5 pts)
echo "--- Check 24: Tool errors ---"
TOTAL=$((TOTAL + 5))
if [[ "$TOTAL_ERRORS" -eq 0 ]]; then
  echo "PASS (5 pts): Zero tool errors"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 5 ]]; then
  echo "PARTIAL (3/5 pts): $TOTAL_ERRORS tool errors (threshold: <=5)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 15 ]]; then
  echo "PARTIAL (1/5 pts): $TOTAL_ERRORS tool errors (threshold: <=15)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $TOTAL_ERRORS tool errors"
  FAIL=$((FAIL + 1))
fi
echo ""

# Check 25: --help + retries (5 pts)
echo "--- Check 25: --help and retries ---"
TOTAL=$((TOTAL + 5))
HELP_RETRY_TOTAL=$((TOTAL_HELP + TOTAL_RETRIES))
if [[ "$HELP_RETRY_TOTAL" -eq 0 ]]; then
  echo "PASS (5 pts): Zero --help lookups and retries"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 3 ]]; then
  echo "PARTIAL (3/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=3)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 8 ]]; then
  echo "PARTIAL (1/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=8)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $HELP_RETRY_TOTAL --help/retries"
  FAIL=$((FAIL + 1))
fi
echo ""

# ============================================================
# Critical Fail: No workers spawned → cap at 30%
# ============================================================
if ! $WORKERS_SPAWNED; then
  MAX_SCORE=$(( TOTAL * 30 / 100 ))
  if [[ "$SCORE" -gt "$MAX_SCORE" ]]; then
    echo "CAPPING: No workers spawned — score capped from $SCORE to $MAX_SCORE (30% of $TOTAL)"
    SCORE=$MAX_SCORE
  fi
fi

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
echo "PROJECT_DIR=$PROJECT_DIR"
echo "MISSION_BONE=$MISSION_BONE"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history futil -n 50' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-${FUTIL_DEV}.log' for dev output"
echo "Run 'ls $ARTIFACTS/agent-${FUTIL_DEV}_*.log' for worker logs"
echo ""
echo "=== Verification Complete ==="
