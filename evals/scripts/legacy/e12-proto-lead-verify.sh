#!/usr/bin/env bash
set -euo pipefail

# E12-Proto-Lead Verification Script
# Checks whether the dev-loop lead correctly merged the pre-completed workspace.
# Primary goal: validate `botbox protocol merge` usage.

source "${1:?Usage: e12-proto-lead-verify.sh <path-to-.eval-env>}"

echo "=== E12-Proto-Lead Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
echo "WORKER_WS=$WORKER_WS"
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
AGENT_LOG=$(cat "$ARTIFACTS/agent-lead.log" 2>/dev/null || echo "")
CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-history.log" 2>/dev/null || echo "")
FINAL_STATUS_FILE=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
BONE_FINAL_JSON=$(cat "$ARTIFACTS/bone-final.json" 2>/dev/null || echo "[]")
CLAIMS_FINAL=$(cat "$ARTIFACTS/claims-final.txt" 2>/dev/null || echo "")
WS_STATE=$(cat "$ARTIFACTS/workspace-state.json" 2>/dev/null || echo "{}")
TEST_OUTPUT=$(cat "$ARTIFACTS/test-output.txt" 2>/dev/null || echo "")

cd "$PROJECT_DIR"

# ============================================================
# Protocol Merge Usage (40 pts) — PRIMARY GOAL
# ============================================================
echo "=== Protocol Merge Usage (40 pts) ==="
echo ""

# Check 1: Lead used botbox protocol merge (20 pts) — the key check
echo "--- Check 1: protocol merge invoked ---"
USED_PROTO_MERGE=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol merge\|botbox.*protocol.*merge"; then
  USED_PROTO_MERGE=true
fi
check "Lead invoked 'botbox protocol merge'" "$($USED_PROTO_MERGE && echo 0 || echo 1)" 20

# Check 2: protocol merge targeted correct workspace (10 pts)
echo ""
echo "--- Check 2: protocol merge targeted $WORKER_WS ---"
MERGE_CORRECT_WS=false
if echo "$AGENT_LOG" | grep -qi "protocol merge.*$WORKER_WS\|protocol merge.*storm-reef"; then
  MERGE_CORRECT_WS=true
fi
# Also accept if protocol merge was called and workspace was merged (even if WS name not in exact command)
if [[ "$USED_PROTO_MERGE" == "true" ]]; then
  NON_DEFAULT_AFTER=$(echo "$WS_STATE" | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "1")
  if [[ "$NON_DEFAULT_AFTER" -eq 0 ]]; then
    MERGE_CORRECT_WS=true
  fi
fi
check "protocol merge targeted correct workspace" "$($MERGE_CORRECT_WS && echo 0 || echo 1)" 10

# Check 3: Lead read task-done from inbox (10 pts)
echo ""
echo "--- Check 3: inbox read ---"
READ_INBOX=false
if echo "$AGENT_LOG" | grep -qi "bus inbox\|task-done\|Completed.*$BONE_ID"; then
  READ_INBOX=true
fi
check "Lead read inbox (task-done message)" "$($READ_INBOX && echo 0 || echo 1)" 10

# ============================================================
# Merge Execution (50 pts)
# ============================================================
echo ""
echo "=== Merge Execution (50 pts) ==="
echo ""

# Check 4: Workspace was merged/destroyed (15 pts)
echo "--- Check 4: workspace merged ---"
WS_MERGED=false
NON_DEFAULT_AFTER=$(echo "$WS_STATE" | jq '[.workspaces[] | select(.is_default == false)] | length' 2>/dev/null || echo "1")
WS_STILL_EXISTS=$(echo "$WS_STATE" | jq -r ".workspaces[] | select(.name == \"$WORKER_WS\") | .name" 2>/dev/null || echo "")
if [[ -z "$WS_STILL_EXISTS" ]]; then
  WS_MERGED=true
fi
check "Workspace $WORKER_WS merged ($NON_DEFAULT_AFTER non-default remaining)" "$($WS_MERGED && echo 0 || echo 1)" 15

# Check 5: maw ws merge --destroy was called (10 pts)
echo ""
echo "--- Check 5: maw ws merge --destroy ---"
MERGE_CMD_USED=false
if echo "$AGENT_LOG" | grep -qi "maw ws merge.*--destroy\|ws merge.*destroy"; then
  MERGE_CMD_USED=true
fi
check "maw ws merge --destroy was called" "$($MERGE_CMD_USED && echo 0 || echo 1)" 10

# Check 6: Tests pass in default workspace after merge (10 pts)
echo ""
echo "--- Check 6: tests pass after merge ---"
TESTS_PASS=false
if echo "$TEST_OUTPUT" | grep -q "test result: ok"; then
  TESTS_PASS=true
fi
check "cargo test passes in default after merge" "$($TESTS_PASS && echo 0 || echo 1)" 10

# Check 7: todo!() replaced in default workspace (5 pts)
echo ""
echo "--- Check 7: code merged to default ---"
CODE_MERGED=false
DEFAULT_GREET="$PROJECT_DIR/ws/default/src/greet.rs"
if [[ -f "$DEFAULT_GREET" ]] && ! grep -q 'todo!' "$DEFAULT_GREET" 2>/dev/null; then
  CODE_MERGED=true
fi
check "todo!() removed from default/src/greet.rs" "$($CODE_MERGED && echo 0 || echo 1)" 5

# Check 8: bones sync not needed (event-sourced) — check maw push instead (10 pts)
echo ""
echo "--- Check 8: maw push ---"
BR_SYNCED=false
if echo "$AGENT_LOG" | grep -qi "maw push\|push.*main"; then
  BR_SYNCED=true
fi
check "maw push was run" "$($BR_SYNCED && echo 0 || echo 1)" 10

# ============================================================
# Announcements & Cleanup (30 pts)
# ============================================================
echo ""
echo "=== Announcements & Cleanup (30 pts) ==="
echo ""

# Check 9: Merge announced on channel (10 pts)
echo ""
echo "--- Check 9: merge announcement ---"
MERGE_ANNOUNCED=false
if echo "$CHANNEL_HISTORY" | grep -qi "merge\|Merged"; then
  MERGE_ANNOUNCED=true
fi
if echo "$AGENT_LOG" | grep -qi "bus send.*merge\|bus send.*Merged"; then
  MERGE_ANNOUNCED=true
fi
check "Merge announced on channel" "$($MERGE_ANNOUNCED && echo 0 || echo 1)" 10

# Check 10: Lead attempted to release worker claims OR claims are gone (10 pts)
# Note: Worker claims are staked by a different agent (worker). The lead may not be
# able to release them directly (agent identity mismatch). We accept:
# a) Claims are actually gone (released somehow)
# b) Lead attempted to release them (good faith effort)
echo ""
echo "--- Check 10: worker claims handled ---"
CLAIMS_HANDLED=false
# Claims are gone
WORKER_CLAIMS_REMAIN=$(echo "$CLAIMS_FINAL" | grep -ci "eval-worker\|$WORKER_WS" || true)
if [[ "$WORKER_CLAIMS_REMAIN" -eq 0 ]]; then
  CLAIMS_HANDLED=true
fi
# Lead tried to release them (even if it failed due to agent identity)
if echo "$AGENT_LOG" | grep -qi "claims release.*bone\|claims release.*workspace\|claims release.*$BONE_ID\|claims release.*$WORKER_WS"; then
  CLAIMS_HANDLED=true
fi
check "Worker claims handled (released or attempted)" "$($CLAIMS_HANDLED && echo 0 || echo 1)" 10

# Check 11: Lead checked workspace list (10 pts)
echo ""
echo "--- Check 11: workspace discovery ---"
CHECKED_WS_LIST=false
if echo "$AGENT_LOG" | grep -qi "maw ws list\|ws list\|workspace"; then
  CHECKED_WS_LIST=true
fi
check "Lead checked workspace list" "$($CHECKED_WS_LIST && echo 0 || echo 1)" 10

# ============================================================
# Friction (negative scoring)
# ============================================================
echo ""
echo "=== Friction Analysis ==="
echo ""

FRICTION_PENALTY=0

# Friction: protocol merge errors (fallback to manual)
PROTO_ERRORS=$(echo "$AGENT_LOG" | grep -c "protocol merge.*exit\|protocol merge.*error\|protocol merge.*fail" || true)
if [[ "$PROTO_ERRORS" -gt 0 ]]; then
  echo "FRICTION (-5): Protocol merge command errors ($PROTO_ERRORS)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: created new bones (nothing to create here)
DUP_BONES=$(echo "$AGENT_LOG" | grep -c "bn create.*--title" || true)
if [[ "$DUP_BONES" -gt 0 ]]; then
  echo "FRICTION (-5): Lead created new bones ($DUP_BONES creates — nothing needed)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: tried to destroy default workspace
if echo "$AGENT_LOG" | grep -qi "ws merge default\|ws destroy default"; then
  echo "FRICTION (-10): Lead tried to merge/destroy default workspace"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 10))
fi

# Friction: dispatched unnecessary workers
if echo "$AGENT_LOG" | grep -qi "vessel spawn.*worker"; then
  echo "FRICTION (-5): Lead dispatched workers (nothing to work on)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: --help lookups
HELP_COUNT=$(echo "$AGENT_LOG" | grep -c -- "--help" || true)
if [[ "$HELP_COUNT" -gt 3 ]]; then
  echo "FRICTION (-5): Lead used --help $HELP_COUNT times (threshold: 3)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

if [[ "$FRICTION_PENALTY" -eq 0 ]]; then
  echo "No friction detected."
fi

# Apply friction
SCORE=$((SCORE - FRICTION_PENALTY))
if [[ "$SCORE" -lt 0 ]]; then
  SCORE=0
fi

echo ""
echo "Friction penalty: -${FRICTION_PENALTY} pts"

# ============================================================
# Summary
# ============================================================
echo ""
echo "=== Verification Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "WARN: $WARN"
echo "FRICTION: -$FRICTION_PENALTY"
echo "SCORE: $SCORE / $TOTAL"
echo ""

if [[ "$FAIL" -eq 0 && "$FRICTION_PENALTY" -eq 0 ]]; then
  echo "RESULT: PERFECT ($SCORE/$TOTAL)"
elif [[ "$FAIL" -eq 0 ]]; then
  echo "RESULT: ALL CHECKS PASSED ($SCORE/$TOTAL, -$FRICTION_PENALTY friction)"
elif [[ "$SCORE" -ge $(( TOTAL * 85 / 100 )) ]]; then
  echo "RESULT: EXCELLENT ($SCORE/$TOTAL) — $FAIL checks failed"
elif [[ "$SCORE" -ge $(( TOTAL * 70 / 100 )) ]]; then
  echo "RESULT: PASS ($SCORE/$TOTAL) — $FAIL checks failed"
else
  echo "RESULT: FAIL ($SCORE/$TOTAL) — $FAIL checks failed"
fi

echo ""
echo "=== Lead Merge Scorecard ==="
echo "  protocol merge invoked:  $($USED_PROTO_MERGE && echo "YES" || echo "NO")"
echo "  correct workspace:       $($MERGE_CORRECT_WS && echo "YES" || echo "NO")"
echo "  inbox read:              $($READ_INBOX && echo "YES" || echo "NO")"
echo "  workspace merged:        $($WS_MERGED && echo "YES" || echo "NO")"
echo "  merge --destroy called:  $($MERGE_CMD_USED && echo "YES" || echo "NO")"
echo "  tests pass:              $($TESTS_PASS && echo "YES" || echo "NO")"
echo "  code in default:         $($CODE_MERGED && echo "YES" || echo "NO")"
echo "  maw push:                $($BR_SYNCED && echo "YES" || echo "NO")"
echo "  merge announced:         $($MERGE_ANNOUNCED && echo "YES" || echo "NO")"
echo "  claims handled:          $($CLAIMS_HANDLED && echo "YES" || echo "NO")"
echo "  workspace discovery:     $($CHECKED_WS_LIST && echo "YES" || echo "NO")"

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
echo "WORKER_WS=$WORKER_WS"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history greeter -n 50' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-lead.log' for agent output"
echo ""
echo "=== Verification Complete ==="
