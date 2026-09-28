#!/usr/bin/env bash
set -euo pipefail

# E12-Proto Verification Script
# Automated scoring for the protocol command integration eval.
# Checks protocol command usage, output correctness, state transitions,
# work quality, cleanup, and friction.

source "${1:?Usage: e12-proto-verify.sh <path-to-.eval-env>}"

echo "=== E12-Proto Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
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
AGENT_LOG=$(cat "$ARTIFACTS/agent-worker.log" 2>/dev/null || echo "")
CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-history.log" 2>/dev/null || echo "")
FINAL_STATUS=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
BONE_FINAL_JSON=$(cat "$ARTIFACTS/bone-final.json" 2>/dev/null || echo "[]")
BONE_COMMENTS=$(cat "$ARTIFACTS/bone-comments.txt" 2>/dev/null || echo "")
CLAIMS_FINAL=$(cat "$ARTIFACTS/claims-final.txt" 2>/dev/null || echo "")
WS_STATE=$(cat "$ARTIFACTS/workspace-state.json" 2>/dev/null || echo "{}")
TEST_OUTPUT=$(cat "$ARTIFACTS/test-output.txt" 2>/dev/null || echo "")

cd "$PROJECT_DIR"

# ============================================================
# Protocol Command Usage (50 pts)
# ============================================================
echo "=== Protocol Command Usage (50 pts) ==="
echo ""

# Check 1: Agent used botbox protocol resume (10 pts)
echo "--- Check 1: protocol resume ---"
USED_RESUME=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol resume\|botbox.*protocol.*resume"; then
  USED_RESUME=true
fi
check "Agent invoked 'botbox protocol resume'" "$($USED_RESUME && echo 0 || echo 1)" 10

# Check 2: Agent used botbox protocol start (10 pts)
echo ""
echo "--- Check 2: protocol start ---"
USED_START=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol start.*$BONE_ID\|botbox.*protocol.*start"; then
  USED_START=true
fi
check "Agent invoked 'botbox protocol start'" "$($USED_START && echo 0 || echo 1)" 10

# Check 3: Agent used botbox protocol review OR manual review steps (10 pts)
# Review is disabled, so either path is valid
echo ""
echo "--- Check 3: protocol review or manual review steps ---"
USED_REVIEW=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol review\|botbox.*protocol.*review"; then
  USED_REVIEW=true
fi
# Also accept manual review steps (crit reviews create, review-request)
if echo "$AGENT_LOG" | grep -qi "crit reviews create\|review.*request\|skip.*review"; then
  USED_REVIEW=true
fi
# With review disabled, the agent might just skip review entirely — that's also valid
if echo "$AGENT_LOG" | grep -qi "review.*disabled\|review.*false\|no.*review"; then
  USED_REVIEW=true
fi
# If the agent went straight to protocol finish, it implicitly handled review (skipped it)
if echo "$AGENT_LOG" | grep -qi "botbox protocol finish\|botbox.*protocol.*finish"; then
  USED_REVIEW=true
fi
check "Agent handled review step (protocol or manual or skip)" "$($USED_REVIEW && echo 0 || echo 1)" 10

# Check 4: Agent used botbox protocol finish (10 pts)
echo ""
echo "--- Check 4: protocol finish ---"
USED_FINISH=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol finish.*$BONE_ID\|botbox.*protocol.*finish"; then
  USED_FINISH=true
fi
check "Agent invoked 'botbox protocol finish'" "$($USED_FINISH && echo 0 || echo 1)" 10

# Check 5: Agent used botbox protocol cleanup (10 pts)
echo ""
echo "--- Check 5: protocol cleanup ---"
USED_CLEANUP=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol cleanup\|botbox.*protocol.*cleanup"; then
  USED_CLEANUP=true
fi
check "Agent invoked 'botbox protocol cleanup'" "$($USED_CLEANUP && echo 0 || echo 1)" 10

# ============================================================
# Protocol Output Correctness (30 pts)
# ============================================================
echo ""
echo "=== Protocol Output Correctness (30 pts) ==="
echo ""

# Check 6: protocol start returned valid status (10 pts)
echo "--- Check 6: protocol start output ---"
START_OK=false
# Look for status= in the protocol start output, or "Fresh" / "Ready" / "Resumable" keywords
if echo "$AGENT_LOG" | grep -qi "status.*Fresh\|status.*Ready\|status.*Resumable\|STATUS: Fresh\|STATUS: Ready"; then
  START_OK=true
fi
# Also check for the structured output lines from protocol commands
if echo "$AGENT_LOG" | grep -qi "WORKSPACE:\|BONE:\|AGENT:"; then
  START_OK=true
fi
# If the agent successfully created workspace and claimed bone, the protocol worked
if echo "$AGENT_LOG" | grep -qi "maw ws create\|bus claims stake.*bone://"; then
  START_OK=true
fi
check "protocol start returned valid status (not error)" "$($START_OK && echo 0 || echo 1)" 10

# Check 7: protocol finish returned valid status (10 pts)
echo ""
echo "--- Check 7: protocol finish output ---"
FINISH_OK=false
if echo "$AGENT_LOG" | grep -qi "status.*Ready\|finish.*Ready\|merge.*workspace\|maw ws merge"; then
  FINISH_OK=true
fi
# Bone being done is strong evidence finish worked
BONE_FINAL_STATE=$(echo "$BONE_FINAL_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
if [[ "$BONE_FINAL_STATE" == "done" ]]; then
  FINISH_OK=true
fi
check "protocol finish returned valid status" "$($FINISH_OK && echo 0 || echo 1)" 10

# Check 8: protocol cleanup returned valid status (10 pts)
echo ""
echo "--- Check 8: protocol cleanup output ---"
CLEANUP_OK=false
if echo "$AGENT_LOG" | grep -qi "cleanup.*Ready\|cleanup.*HasResources\|No cleanup needed\|claims.*release"; then
  CLEANUP_OK=true
fi
# If no claims remain, cleanup worked
if echo "$CLAIMS_FINAL" | grep -qi "no.*claim\|^$" || [[ -z "$(echo "$CLAIMS_FINAL" | grep -i "eval-worker")" ]]; then
  CLEANUP_OK=true
fi
check "protocol cleanup returned valid status" "$($CLEANUP_OK && echo 0 || echo 1)" 10

# ============================================================
# State Transitions (40 pts)
# ============================================================
echo ""
echo "=== State Transitions (40 pts) ==="
echo ""

# Check 9: Bone transitioned to doing (10 pts)
echo "--- Check 9: bone doing transition ---"
WAS_IN_PROGRESS=false
if echo "$AGENT_LOG" | grep -qi "doing\|state.*doing\|bn do"; then
  WAS_IN_PROGRESS=true
fi
if echo "$BONE_COMMENTS" | grep -qi "doing\|Starting\|claimed"; then
  WAS_IN_PROGRESS=true
fi
check "Bone transitioned to doing" "$($WAS_IN_PROGRESS && echo 0 || echo 1)" 10

# Check 10: Bone is DONE at end (10 pts)
echo ""
echo "--- Check 10: bone done ---"
IS_CLOSED=false
if [[ "$BONE_FINAL_STATE" == "done" ]]; then
  IS_CLOSED=true
fi
check "Bone is DONE at end (state=$BONE_FINAL_STATE)" "$($IS_CLOSED && echo 0 || echo 1)" 10

# Check 11: Workspace was created (10 pts)
echo ""
echo "--- Check 11: workspace created ---"
WS_CREATED=false
if echo "$AGENT_LOG" | grep -qi "maw ws create\|workspace.*created\|Workspace.*ready"; then
  WS_CREATED=true
fi
check "Workspace was created during work" "$($WS_CREATED && echo 0 || echo 1)" 10

# Check 12: Workspace handled correctly (10 pts)
# Workers use --no-merge (leads merge for them), so a leftover workspace with
# completed work is EXPECTED. We check that the worker either:
# a) merged the workspace (ws count == 0), or
# b) correctly used --no-merge and left the workspace for the lead
echo ""
echo "--- Check 12: workspace handled correctly ---"
WS_HANDLED=false
NON_DEFAULT_COUNT=$(echo "$WS_STATE" | jq '[.workspaces[]? | select(.is_default == false)] | length' 2>/dev/null || echo "0")
if [[ "$NON_DEFAULT_COUNT" -eq 0 ]]; then
  WS_HANDLED=true
  echo "  Workspace merged/destroyed (0 non-default)"
fi
# Worker used --no-merge correctly (workspace left for lead)
if echo "$AGENT_LOG" | grep -qiE -- "--no.merge|no-merge"; then
  WS_HANDLED=true
  echo "  Worker used --no-merge (workspace left for lead, $NON_DEFAULT_COUNT non-default)"
fi
# Worker attempted merge
if echo "$AGENT_LOG" | grep -qi "maw ws merge.*--destroy\|workspace.*merged\|workspace.*destroyed"; then
  WS_HANDLED=true
fi
# If bone is done and claims are released, the workspace is handled
if [[ "$BONE_FINAL_STATE" == "done" ]]; then
  WS_HANDLED=true
  echo "  Bone done — workspace lifecycle completed"
fi
check "Workspace handled correctly ($NON_DEFAULT_COUNT non-default)" "$($WS_HANDLED && echo 0 || echo 1)" 10

# ============================================================
# Work Quality (30 pts)
# ============================================================
echo ""
echo "=== Work Quality (30 pts) ==="
echo ""

# Check 13: todo!() replaced (10 pts)
# Workers don't merge — check ALL workspaces, not just default
echo "--- Check 13: todo!() replaced ---"
TODO_GONE=false
for greet_file in "$PROJECT_DIR"/ws/*/src/greet.rs; do
  if [[ -f "$greet_file" ]] && ! grep -q 'todo!' "$greet_file" 2>/dev/null; then
    TODO_GONE=true
    echo "  Found implemented greet.rs at: $greet_file"
    break
  fi
done
check "todo!() removed from greet.rs" "$($TODO_GONE && echo 0 || echo 1)" 10

# Check 14: cargo test passes (10 pts)
# Try default first, then any workspace
echo ""
echo "--- Check 14: cargo test ---"
TESTS_PASS=false
if echo "$TEST_OUTPUT" | grep -q "test result: ok"; then
  TESTS_PASS=true
fi
# Try running tests in each workspace (worker may not have merged)
if ! $TESTS_PASS; then
  cd "$PROJECT_DIR"
  for ws_name in $(maw ws list --format json 2>/dev/null | jq -r '.workspaces[].name' 2>/dev/null); do
    WS_TEST=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec "$ws_name" -- cargo test 2>&1 || echo "")
    if echo "$WS_TEST" | grep -q "test result: ok"; then
      TESTS_PASS=true
      echo "  Tests pass in workspace: $ws_name"
      break
    fi
  done
fi
check "cargo test passes" "$($TESTS_PASS && echo 0 || echo 1)" 10

# Check 15: Agent posted progress comment (10 pts)
echo ""
echo "--- Check 15: progress comments ---"
HAS_COMMENT=false
COMMENT_COUNT=$(echo "$BONE_COMMENTS" | grep -c "at 20" || true)
if [[ "$COMMENT_COUNT" -ge 1 ]]; then
  HAS_COMMENT=true
fi
check "Agent posted progress comment ($COMMENT_COUNT comments)" "$($HAS_COMMENT && echo 0 || echo 1)" 10

# ============================================================
# Cleanup (20 pts)
# ============================================================
echo ""
echo "=== Cleanup (20 pts) ==="
echo ""

# Check 16: No active claims for worker (10 pts)
echo "--- Check 16: claims released ---"
WORKER_CLAIMS=false
if echo "$CLAIMS_FINAL" | grep -qi "eval-worker"; then
  WORKER_CLAIMS=true
fi
check "No active claims for worker" "$($WORKER_CLAIMS && echo 1 || echo 0)" 10

# Check 17: Agent sent cleanup/idle announcement (10 pts)
echo ""
echo "--- Check 17: cleanup announcement ---"
ANNOUNCED=false
if echo "$CHANNEL_HISTORY" | grep -qi "idle\|clean\|sign.*off\|done\|complet\|finish"; then
  ANNOUNCED=true
fi
if echo "$AGENT_LOG" | grep -qi "bus send.*idle\|bus send.*done\|bus send.*Finish\|bus statuses clear\|Signing off"; then
  ANNOUNCED=true
fi
check "Agent sent cleanup/idle announcement" "$($ANNOUNCED && echo 0 || echo 1)" 10

# ============================================================
# Friction (negative scoring)
# ============================================================
echo ""
echo "=== Friction Analysis ==="
echo ""

FRICTION_PENALTY=0

# Friction: protocol command errors (fallback to manual)
PROTO_ERRORS=$(echo "$AGENT_LOG" | grep -c "protocol.*exit\|protocol.*error\|protocol.*fail\|fallback\|fall back" || true)
if [[ "$PROTO_ERRORS" -gt 0 ]]; then
  echo "FRICTION (-5): Protocol command errors requiring fallback ($PROTO_ERRORS)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: duplicate bones
DUP_BONES=$(echo "$AGENT_LOG" | grep -c "bn create.*--title" || true)
if [[ "$DUP_BONES" -gt 0 ]]; then
  echo "FRICTION (-5): Agent created new bones ($DUP_BONES creates — bone was pre-made)"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: edited wrong workspace (default instead of agent workspace)
WRONG_WS=false
# Look for edits to ws/default/ paths (the agent should edit ws/<workspace>/ paths)
if echo "$AGENT_LOG" | grep -qi "Edit.*ws/default/src\|Write.*ws/default/src"; then
  # Only flag if the agent also had a workspace (should edit there, not default)
  if echo "$AGENT_LOG" | grep -qi "maw ws create"; then
    echo "FRICTION (-5): Agent edited files in default workspace instead of agent workspace"
    FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
    WRONG_WS=true
  fi
fi

# Friction: looping (same command 3+ times)
REPEATED=$(echo "$AGENT_LOG" | grep -oP '(?<="command":")[^"]+' 2>/dev/null | sort | uniq -c | sort -rn | head -1 | awk '{print $1}')
if [[ -n "$REPEATED" ]] && [[ "$REPEATED" -gt 5 ]]; then
  echo "FRICTION (-5): Agent repeated same command $REPEATED times"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 5))
fi

# Friction: --help lookups
HELP_COUNT=$(echo "$AGENT_LOG" | grep -c -- "--help" || true)
if [[ "$HELP_COUNT" -gt 3 ]]; then
  echo "FRICTION (-5): Agent used --help $HELP_COUNT times (threshold: 3)"
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
echo "=== Protocol Command Scorecard ==="
echo "  resume:  $($USED_RESUME && echo "YES" || echo "NO")"
echo "  start:   $($USED_START && echo "YES" || echo "NO")"
echo "  review:  $($USED_REVIEW && echo "YES" || echo "NO")"
echo "  finish:  $($USED_FINISH && echo "YES" || echo "NO")"
echo "  cleanup: $($USED_CLEANUP && echo "YES" || echo "NO")"

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history greeter -n 50' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-worker.log' for agent output"
echo "Run 'cat $ARTIFACTS/bone-comments.txt' for bone comments"
echo ""
echo "=== Verification Complete ==="
