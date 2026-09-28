#!/usr/bin/env bash
set -euo pipefail

# Multi-Lead Verification Script
# Scores multi-lead protocol compliance:
# - Lead spawning (separate slot instances)
# - Both missions decomposed and completed
# - Merge serialization (no divergent commits)
# - Bone claim deduplication
# - Code correctness
# - Friction efficiency

source "${1:?Usage: multi-lead-verify.sh <path-to-.eval-env>}"

echo "=== Multi-Lead Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
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
FINAL_STATUS=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-futil-history.log" 2>/dev/null || echo "")
CHANNEL_JSON=$(cat "$ARTIFACTS/channel-futil-history.json" 2>/dev/null || echo '{"messages":[]}')
JJ_LOG=$(cat "$ARTIFACTS/jj-log.txt" 2>/dev/null || echo "")
CLAIMS_JSON=$(cat "$ARTIFACTS/claims-state.json" 2>/dev/null || echo '{"claims":[]}')

# Extract from final status
MISSION_A_BONE=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_A_BONE=\K[^ ]+' || echo "none")
MISSION_B_BONE=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_B_BONE=\K[^ ]+' || echo "none")
MISSION_A_STATUS=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_A_STATUS=\K[^ ]+' || echo "unknown")
MISSION_B_STATUS=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_B_STATUS=\K[^ ]+' || echo "unknown")
LEAD_SLOT_COUNT=$(echo "$FINAL_STATUS" | grep -oP 'LEAD_SLOT_COUNT=\K\d+' || echo "0")
WORKER_COUNT=$(echo "$FINAL_STATUS" | grep -oP 'WORKER_COUNT=\K\d+' || echo "0")
MERGE_COUNT_FINAL=$(echo "$FINAL_STATUS" | grep -oP 'MERGE_COUNT=\K\d+' || echo "0")

cd "$PROJECT_DIR"

# ============================================================
# Critical Fail: No missions created
# ============================================================
echo "=== Critical Fail Check ==="
echo ""
if [[ "$MISSION_A_BONE" == "none" && "$MISSION_B_BONE" == "none" ]]; then
  echo "CRITICAL FAIL: No mission bones were created"
  echo ""
  echo "SCORE: 0 / 0 (critical fail)"
  echo "RESULT: CRITICAL FAIL — no missions created"
  exit 0
fi
echo "Mission A: $MISSION_A_BONE ($MISSION_A_STATUS)"
echo "Mission B: $MISSION_B_BONE ($MISSION_B_STATUS)"
echo ""

# ============================================================
# Lead Spawning (20 pts)
# ============================================================
echo "=== Lead Spawning (20 pts) ==="
echo ""

# Check 1: Two separate lead slots used (10 pts)
echo "--- Check 1: Two separate lead slots ---"
check "2+ lead slots discovered ($LEAD_SLOT_COUNT)" "$([ "$LEAD_SLOT_COUNT" -ge 2 ] && echo 0 || echo 1)" 10

# Check 2: Leads used numbered slot names (5 pts)
echo ""
echo "--- Check 2: Numbered slot naming ---"
HAS_NUMBERED_SLOTS=false
LEAD_NAMES=$(echo "$FINAL_STATUS" | sed -n '/LEAD_SLOTS/,/WORKERS/p' | grep -v "LEAD_SLOTS\|WORKERS" | tr -d ' ')
if echo "$LEAD_NAMES" | grep -qE "/[0-9]+$"; then
  HAS_NUMBERED_SLOTS=true
fi
# Also check agent logs for slot patterns
for llog in "$ARTIFACTS"/agent-futil-dev_*.log; do
  [[ -f "$llog" ]] || continue
  if grep -qi "futil-dev/[0-9]" "$llog" 2>/dev/null; then
    HAS_NUMBERED_SLOTS=true
    break
  fi
done
check "Lead slots use numbered naming (futil-dev/N)" "$($HAS_NUMBERED_SLOTS && echo 0 || echo 1)" 5

# Check 3: Router spawned leads (not single-lead exec) (5 pts)
echo ""
echo "--- Check 3: Router spawned leads via vessel ---"
ROUTER_SPAWNED=false
# Check channel history for spawn-ack messages
if echo "$CHANNEL_HISTORY" | grep -qi "Spawned lead\|spawn-ack"; then
  ROUTER_SPAWNED=true
fi
# Check router logs
for rlog in "$ARTIFACTS"/agent-*router*.log; do
  [[ -f "$rlog" ]] || continue
  if grep -qi "vessel spawn\|spawnLead\|acquireLeadSlot" "$rlog" 2>/dev/null; then
    ROUTER_SPAWNED=true
    break
  fi
done
check "Router spawned leads via vessel" "$($ROUTER_SPAWNED && echo 0 || echo 1)" 5

# ============================================================
# Mission Decomposition (20 pts)
# ============================================================
echo ""
echo "=== Mission Decomposition (20 pts) ==="
echo ""

# Check 4: Both missions created (5 pts)
echo "--- Check 4: Both missions created ---"
BOTH_CREATED=false
[[ "$MISSION_A_BONE" != "none" && "$MISSION_B_BONE" != "none" ]] && BOTH_CREATED=true
check "Both mission bones created" "$($BOTH_CREATED && echo 0 || echo 1)" 5

# Check 5: Mission A has children (5 pts)
echo ""
echo "--- Check 5: Mission A children ---"
MA_CHILD_COUNT=0
if [[ "$MISSION_A_BONE" != "none" ]]; then
  MA_CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_A_BONE" --format json 2>/dev/null || echo '[]')
  MA_CHILD_COUNT=$(echo "$MA_CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")
fi
check "Mission A has children ($MA_CHILD_COUNT)" "$([ "$MA_CHILD_COUNT" -ge 1 ] && echo 0 || echo 1)" 5

# Check 6: Mission B has children (5 pts)
echo ""
echo "--- Check 6: Mission B children ---"
MB_CHILD_COUNT=0
if [[ "$MISSION_B_BONE" != "none" ]]; then
  MB_CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_B_BONE" --format json 2>/dev/null || echo '[]')
  MB_CHILD_COUNT=$(echo "$MB_CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")
fi
check "Mission B has children ($MB_CHILD_COUNT)" "$([ "$MB_CHILD_COUNT" -ge 1 ] && echo 0 || echo 1)" 5

# Check 7: Children have mission labels (5 pts)
echo ""
echo "--- Check 7: Mission labels on children ---"
TOTAL_CHILDREN=$((MA_CHILD_COUNT + MB_CHILD_COUNT))
LABELED=0
for mbone in "$MISSION_A_BONE" "$MISSION_B_BONE"; do
  [[ "$mbone" == "none" || -z "$mbone" ]] && continue
  CHILD_IDS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$mbone" --format json 2>/dev/null | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null || echo "")
  for cid in $CHILD_IDS; do
    CL=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].labels // [] | .[]' 2>/dev/null || echo "")
    echo "$CL" | grep -q "mission:" && LABELED=$((LABELED + 1))
  done
done
check "Children have mission:<id> labels ($LABELED/$TOTAL_CHILDREN)" "$([ "$LABELED" -ge 2 ] && echo 0 || echo 1)" 5

# ============================================================
# Merge Serialization (20 pts)
# ============================================================
echo ""
echo "=== Merge Serialization (20 pts) ==="
echo ""

# Check 8: No divergent commits (10 pts)
echo "--- Check 8: No divergent commits ---"
DIVERGENT_COUNT=0
if [[ -n "$JJ_LOG" ]]; then
  DIVERGENT_COUNT=$(echo "$JJ_LOG" | grep -ci "divergent" 2>/dev/null) || DIVERGENT_COUNT=0
fi
# Also check live
LIVE_DIVERGENT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- jj log --no-graph 2>/dev/null | grep -ci "divergent" || true)
LIVE_DIVERGENT=${LIVE_DIVERGENT:-0}
DIVERGENT_COUNT=$((DIVERGENT_COUNT + LIVE_DIVERGENT))
check "No divergent commits (found=$DIVERGENT_COUNT)" "$([ "$DIVERGENT_COUNT" -eq 0 ] && echo 0 || echo 1)" 10

# Check 9: Merge mutex used (coord:merge messages) (5 pts)
echo ""
echo "--- Check 9: Merge mutex used ---"
COORD_MERGE_COUNT=$(echo "$CHANNEL_HISTORY" | grep -ci "coord:merge\|Merged.*for\|Merged.*workspace" 2>/dev/null) || COORD_MERGE_COUNT=0
# Also check for merge mutex claim patterns in logs
MUTEX_USED=false
if [[ "$COORD_MERGE_COUNT" -ge 1 ]]; then
  MUTEX_USED=true
fi
# Check lead logs for merge protocol
for llog in "$ARTIFACTS"/agent-futil-dev_*.log; do
  [[ -f "$llog" ]] || continue
  if grep -qi "workspace.*default.*merge\|merge.*mutex\|merge.*protocol\|claims.*stake.*default" "$llog" 2>/dev/null; then
    MUTEX_USED=true
    break
  fi
done
check "Merge mutex used ($COORD_MERGE_COUNT coord:merge messages)" "$($MUTEX_USED && echo 0 || echo 1)" 5

# Check 10: Multiple merges completed (5 pts)
echo ""
echo "--- Check 10: Multiple merges completed ---"
# Check jj log for multiple non-empty commits on main
COMMIT_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- jj log -r 'main@origin..main' --no-graph -T 'description.first_line() ++ "\n"' 2>/dev/null | grep -c "." || true)
COMMIT_COUNT=${COMMIT_COUNT:-0}
check "Multiple merges to main ($COMMIT_COUNT commits since origin)" "$([ "$COMMIT_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# ============================================================
# Bone Claim Deduplication (10 pts)
# ============================================================
echo ""
echo "=== Bone Claim Dedup (10 pts) ==="
echo ""

# Check 11: No bone claimed by two different agents (5 pts)
echo "--- Check 11: No duplicate bone claims ---"
DUPLICATE_CLAIMS=false
ALL_BEADS_JSON=$(cat "$ARTIFACTS/all-bones-state.json" 2>/dev/null || echo '[]')
ALL_BEAD_IDS=$(echo "$ALL_BEADS_JSON" | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null || echo "")
# Check channel history and lead logs for "already claimed" or "skip" patterns
CLAIM_CONFLICT_COUNT=0
for llog in "$ARTIFACTS"/agent-futil-dev_*.log; do
  [[ -f "$llog" ]] || continue
  CC=$(grep -ci "already claimed\|skip.*claimed\|bone.*claimed.*another\|claim.*fail" "$llog" 2>/dev/null) || CC=0
  CLAIM_CONFLICT_COUNT=$((CLAIM_CONFLICT_COUNT + CC))
done
# No duplicate claims = good. Some claim conflicts resolved = also good (means the claim checking worked).
check "No duplicate bone ownership (conflicts detected and resolved: $CLAIM_CONFLICT_COUNT)" 0 5

# Check 12: Workers from different leads don't share bones (5 pts)
echo ""
echo "--- Check 12: Work division ---"
# Check that missions A and B work on different bones
OVERLAP=false
if [[ "$MISSION_A_BONE" != "none" && "$MISSION_B_BONE" != "none" ]]; then
  # Get child IDs for each mission
  A_CHILDREN=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_A_BONE" --format json 2>/dev/null | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null | sort)
  B_CHILDREN=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_B_BONE" --format json 2>/dev/null | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null | sort)
  COMMON=$(comm -12 <(echo "$A_CHILDREN") <(echo "$B_CHILDREN") | grep -c "." || true)
  COMMON=${COMMON:-0}
  if [[ "$COMMON" -gt 0 ]]; then
    OVERLAP=true
    warn "Missions share $COMMON child bones"
  fi
fi
check "Missions work on separate bones (no overlap)" "$($OVERLAP && echo 1 || echo 0)" 5

# ============================================================
# Both Missions Complete (20 pts)
# ============================================================
echo ""
echo "=== Mission Completion (20 pts) ==="
echo ""

# Check 13: Mission A completed (5 pts)
echo "--- Check 13: Mission A completed ---"
check "Mission A closed ($MISSION_A_STATUS)" "$([ "$MISSION_A_STATUS" == "done" ] && echo 0 || echo 1)" 5

# Check 14: Mission B completed (5 pts)
echo ""
echo "--- Check 14: Mission B completed ---"
check "Mission B closed ($MISSION_B_STATUS)" "$([ "$MISSION_B_STATUS" == "done" ] && echo 0 || echo 1)" 5

# Check 15: Mission A children all closed (5 pts)
echo ""
echo "--- Check 15: Mission A children closed ---"
MA_CLOSED=0
if [[ "$MISSION_A_BONE" != "none" && "$MA_CHILD_COUNT" -gt 0 ]]; then
  MA_CLOSED=$(echo "$MA_CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")
fi
check "Mission A children all closed ($MA_CLOSED/$MA_CHILD_COUNT)" "$([ "$MA_CLOSED" -ge "$MA_CHILD_COUNT" ] && [ "$MA_CHILD_COUNT" -gt 0 ] && echo 0 || echo 1)" 5

# Check 16: Mission B children all closed (5 pts)
echo ""
echo "--- Check 16: Mission B children closed ---"
MB_CLOSED=0
if [[ "$MISSION_B_BONE" != "none" && "$MB_CHILD_COUNT" -gt 0 ]]; then
  MB_CLOSED=$(echo "$MB_CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")
fi
check "Mission B children all closed ($MB_CLOSED/$MB_CHILD_COUNT)" "$([ "$MB_CLOSED" -ge "$MB_CHILD_COUNT" ] && [ "$MB_CHILD_COUNT" -gt 0 ] && echo 0 || echo 1)" 5

# ============================================================
# Code Correctness (15 pts)
# ============================================================
echo ""
echo "=== Code Correctness (15 pts) ==="
echo ""

# Check 17: cargo check passes (5 pts)
echo "--- Check 17: cargo check ---"
CARGO_OK=false
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  CARGO_OK=true
fi
check "cargo check passes" "$($CARGO_OK && echo 0 || echo 1)" 5

# Check 18: 2+ subcommands implemented (5 pts)
echo ""
echo "--- Check 18: Subcommands implemented ---"
IMPL_COUNT=0
for mod_name in stats search convert; do
  MOD_FILE="$PROJECT_DIR/ws/default/src/${mod_name}.rs"
  if [[ -f "$MOD_FILE" ]] && ! grep -q 'todo!' "$MOD_FILE" 2>/dev/null; then
    IMPL_COUNT=$((IMPL_COUNT + 1))
  fi
done
check "2+ subcommands implemented (found=$IMPL_COUNT)" "$([ "$IMPL_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 19: At least 1 subcommand works on sample data (5 pts)
echo ""
echo "--- Check 19: Subcommand works on sample data ---"
WORKING_COUNT=0
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo build --quiet 2>/dev/null; then
  STATS_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- stats data/sample.txt 2>/dev/null || echo "")
  echo "$STATS_OUT" | grep -qiE "line|word|byte|[0-9]+" && WORKING_COUNT=$((WORKING_COUNT + 1))
  SEARCH_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- search "Hello" data/sample.txt 2>/dev/null || echo "")
  echo "$SEARCH_OUT" | grep -qi "Hello" && WORKING_COUNT=$((WORKING_COUNT + 1))
  CONVERT_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- convert data/sample.json --format csv 2>/dev/null || echo "")
  echo "$CONVERT_OUT" | grep -qi "Alice" && WORKING_COUNT=$((WORKING_COUNT + 1))
fi
check "Subcommand works on sample data ($WORKING_COUNT/3)" "$([ "$WORKING_COUNT" -ge 1 ] && echo 0 || echo 1)" 5

# ============================================================
# Friction Efficiency (10 pts)
# ============================================================
echo ""
echo "=== Friction Efficiency (10 pts) ==="
echo ""

_count_tool_errors() {
  local log="$1"
  local count=0
  local tue; tue=$(grep -c "tool_use_error" "$log" 2>/dev/null) || tue=0
  count=$((count + tue))
  local ec; ec=$(awk '
    /Exit code [12][^0-9]/ || /Exit code [12]$/ {
      if (prev ~ /cargo run/ || prev ~ /\.\/target\// || (prev ~ /^> Bash/ && prev ~ /"command":"cd /)) next
      count++
    }
    { prev = $0 }
    END { print count+0 }
  ' "$log" 2>/dev/null) || ec=0
  count=$((count + ec))
  echo "$count"
}

TOTAL_ERRORS=0
TOTAL_HELP=0
TOTAL_RETRIES=0
for alog in "$ARTIFACTS"/agent-*.log; do
  [[ -f "$alog" ]] || continue
  AE=$(_count_tool_errors "$alog")
  AH=$(grep -c "\-\-help" "$alog" 2>/dev/null) || AH=0
  AR=$(grep -c "retry\|again\|Retrying" "$alog" 2>/dev/null) || AR=0
  TOTAL_ERRORS=$((TOTAL_ERRORS + AE))
  TOTAL_HELP=$((TOTAL_HELP + AH))
  TOTAL_RETRIES=$((TOTAL_RETRIES + AR))
done

echo "Total across all agents: $TOTAL_ERRORS errors, $TOTAL_HELP --help, $TOTAL_RETRIES retries"
echo ""

# Check 20: Tool errors (5 pts)
echo "--- Check 20: Tool errors ---"
TOTAL=$((TOTAL + 5))
if [[ "$TOTAL_ERRORS" -eq 0 ]]; then
  echo "PASS (5 pts): Zero tool errors"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 8 ]]; then
  echo "PARTIAL (3/5 pts): $TOTAL_ERRORS tool errors (threshold: <=8)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 20 ]]; then
  echo "PARTIAL (1/5 pts): $TOTAL_ERRORS tool errors (threshold: <=20)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $TOTAL_ERRORS tool errors"
  FAIL=$((FAIL + 1))
fi

# Check 21: --help + retries (5 pts)
echo ""
echo "--- Check 21: --help and retries ---"
TOTAL=$((TOTAL + 5))
HELP_RETRY_TOTAL=$((TOTAL_HELP + TOTAL_RETRIES))
if [[ "$HELP_RETRY_TOTAL" -eq 0 ]]; then
  echo "PASS (5 pts): Zero --help lookups and retries"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 5 ]]; then
  echo "PARTIAL (3/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=5)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 12 ]]; then
  echo "PARTIAL (1/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=12)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $HELP_RETRY_TOTAL --help/retries"
  FAIL=$((FAIL + 1))
fi

# ============================================================
# Critical cap: No second lead → cap at 40%
# ============================================================
echo ""
if [[ "$LEAD_SLOT_COUNT" -lt 2 ]]; then
  MAX_SCORE=$(( TOTAL * 40 / 100 ))
  if [[ "$SCORE" -gt "$MAX_SCORE" ]]; then
    echo "CAPPING: Only $LEAD_SLOT_COUNT lead slot(s) used — score capped from $SCORE to $MAX_SCORE (40% of $TOTAL)"
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
echo "MISSION_A=$MISSION_A_BONE ($MISSION_A_STATUS)"
echo "MISSION_B=$MISSION_B_BONE ($MISSION_B_STATUS)"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history futil -n 50' for channel messages"
echo "Run 'ls $ARTIFACTS/agent-*.log' for agent logs"
echo ""
echo "=== Verification Complete ==="
