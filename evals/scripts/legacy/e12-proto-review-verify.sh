#!/usr/bin/env bash
set -euo pipefail

# E12-Proto-Review Verification Script
# Includes all base E12-proto checks PLUS review-specific checks.
# Validates protocol review, crit review creation, review gate, and LGTM flow.

source "${1:?Usage: e12-proto-review-verify.sh <path-to-.eval-env>}"

echo "=== E12-Proto-Review Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
echo "REVIEW_ID=${REVIEW_ID:-not set}"
echo "REVIEW_WS=${REVIEW_WS:-not set}"
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
FINAL_STATUS_FILE=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
BONE_FINAL_JSON=$(cat "$ARTIFACTS/bone-final.json" 2>/dev/null || echo "[]")
BONE_COMMENTS=$(cat "$ARTIFACTS/bone-comments.txt" 2>/dev/null || echo "")
CLAIMS_FINAL=$(cat "$ARTIFACTS/claims-final.txt" 2>/dev/null || echo "")
WS_STATE=$(cat "$ARTIFACTS/workspace-state.json" 2>/dev/null || echo "{}")
TEST_OUTPUT=$(cat "$ARTIFACTS/test-output.txt" 2>/dev/null || echo "")
REVIEW_JSON=$(cat "$ARTIFACTS/review-final.json" 2>/dev/null || echo "{}")

cd "$PROJECT_DIR"

# ============================================================
# Protocol Command Usage (50 pts) — same as base E12-proto
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

# Check 3: Agent used botbox protocol review (10 pts)
# With review ENABLED, the agent MUST use protocol review
echo ""
echo "--- Check 3: protocol review ---"
USED_REVIEW=false
if echo "$AGENT_LOG" | grep -qi "botbox protocol review\|botbox.*protocol.*review"; then
  USED_REVIEW=true
fi
check "Agent invoked 'botbox protocol review'" "$($USED_REVIEW && echo 0 || echo 1)" 10

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
# Review Flow (60 pts) — NEW for review eval
# ============================================================
echo ""
echo "=== Review Flow (60 pts) ==="
echo ""

# Check 6: crit review was created (10 pts)
echo "--- Check 6: crit review created ---"
REVIEW_CREATED=false
if echo "$AGENT_LOG" | grep -qi "crit reviews create\|review.*created\|Created review"; then
  REVIEW_CREATED=true
fi
# Also check if we found a review ID during the run
if [[ -n "${REVIEW_ID:-}" ]]; then
  REVIEW_CREATED=true
fi
check "crit review was created" "$($REVIEW_CREATED && echo 0 || echo 1)" 10

# Check 7: review request sent on bus with @mention (10 pts)
echo ""
echo "--- Check 7: review request on bus ---"
REVIEW_REQUESTED=false
if echo "$CHANNEL_HISTORY" | grep -qi "review.*request\|@greeter-security"; then
  REVIEW_REQUESTED=true
fi
if echo "$AGENT_LOG" | grep -qi "bus send.*review.*@greeter-security\|bus send.*@greeter-security.*review\|review-request"; then
  REVIEW_REQUESTED=true
fi
check "Review request sent on bus with @mention" "$($REVIEW_REQUESTED && echo 0 || echo 1)" 10

# Check 8: review was LGTM'd (eval confirms it applied) (10 pts)
echo ""
echo "--- Check 8: review LGTM applied ---"
REVIEW_LGTM=false
LGTM_DONE_VAL=$(echo "$FINAL_STATUS_FILE" | grep -oP 'REVIEW_LGTM_DONE=\K.*' || echo "false")
if [[ "$LGTM_DONE_VAL" == "true" ]]; then
  REVIEW_LGTM=true
fi
check "Review was LGTM'd by eval script" "$($REVIEW_LGTM && echo 0 || echo 1)" 10

# Check 9: protocol finish saw LGTM (returned Ready, not NeedsReview/Blocked) (10 pts)
echo ""
echo "--- Check 9: protocol finish review gate ---"
FINISH_SAW_LGTM=false
# If protocol finish returned Ready status (not NeedsReview or Blocked)
if echo "$AGENT_LOG" | grep -qi "protocol finish" && echo "$AGENT_LOG" | grep -qi "status.*Ready\|finish.*Ready"; then
  FINISH_SAW_LGTM=true
fi
# Strong signal: bone was done after LGTM (finish worked)
BONE_FINAL_STATE=$(echo "$BONE_FINAL_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
if [[ "$BONE_FINAL_STATE" == "done" ]] && [[ "$LGTM_DONE_VAL" == "true" ]]; then
  FINISH_SAW_LGTM=true
fi
check "protocol finish passed review gate (LGTM found)" "$($FINISH_SAW_LGTM && echo 0 || echo 1)" 10

# Check 10: worker stopped after review request (waited for LGTM) (10 pts)
# Evidence: protocol review output tells worker to stop, or there's a pause between
# review request and finish
echo ""
echo "--- Check 10: worker paused for review ---"
WORKER_PAUSED=false
# Look for evidence of stopping/waiting: "stop", "waiting", "pause", multiple iterations
if echo "$AGENT_LOG" | grep -qi "stop.*wait\|waiting.*review\|review.*pending\|NeedsReview\|iteration.*2\|Iteration 2\|resume.*in.progress"; then
  WORKER_PAUSED=true
fi
# If the worker used protocol resume to find the doing bone (2nd iteration), it paused
if echo "$AGENT_LOG" | grep -qi "protocol resume.*Resumable\|Resuming\|Found in-progress"; then
  WORKER_PAUSED=true
fi
# If both review request AND finish happened, AND LGTM was applied between them, there was a pause
if [[ "$REVIEW_LGTM" == "true" ]] && [[ "$BONE_FINAL_STATE" == "done" ]]; then
  WORKER_PAUSED=true
fi
check "Worker paused/waited for review approval" "$($WORKER_PAUSED && echo 0 || echo 1)" 10

# Check 11: crit reviews mark-merged called or in finish steps (10 pts)
echo ""
echo "--- Check 11: mark-merged ---"
MARK_MERGED=false
if echo "$AGENT_LOG" | grep -qi "crit reviews mark-merged\|mark.merged\|reviews mark-merged"; then
  MARK_MERGED=true
fi
# protocol finish outputs mark-merged as a step even with --no-merge
if echo "$AGENT_LOG" | grep -qi "crit.*mark.*merged"; then
  MARK_MERGED=true
fi
check "crit reviews mark-merged called or in output" "$($MARK_MERGED && echo 0 || echo 1)" 10

# ============================================================
# State Transitions (30 pts)
# ============================================================
echo ""
echo "=== State Transitions (30 pts) ==="
echo ""

# Check 12: Bone transitioned to doing (10 pts)
echo "--- Check 12: bone doing transition ---"
WAS_IN_PROGRESS=false
if echo "$AGENT_LOG" | grep -qi "doing\|state.*doing\|bn do"; then
  WAS_IN_PROGRESS=true
fi
if echo "$BONE_COMMENTS" | grep -qi "doing\|Starting\|claimed"; then
  WAS_IN_PROGRESS=true
fi
check "Bone transitioned to doing" "$($WAS_IN_PROGRESS && echo 0 || echo 1)" 10

# Check 13: Bone is DONE at end (10 pts)
echo ""
echo "--- Check 13: bone done ---"
IS_CLOSED=false
if [[ "$BONE_FINAL_STATE" == "done" ]]; then
  IS_CLOSED=true
fi
check "Bone is DONE at end (state=$BONE_FINAL_STATE)" "$($IS_CLOSED && echo 0 || echo 1)" 10

# Check 14: Workspace was created (10 pts)
echo ""
echo "--- Check 14: workspace created ---"
WS_CREATED=false
if echo "$AGENT_LOG" | grep -qi "maw ws create\|workspace.*created\|Workspace.*ready"; then
  WS_CREATED=true
fi
check "Workspace was created during work" "$($WS_CREATED && echo 0 || echo 1)" 10

# ============================================================
# Work Quality (30 pts)
# ============================================================
echo ""
echo "=== Work Quality (30 pts) ==="
echo ""

# Check 15: todo!() replaced (10 pts)
echo "--- Check 15: todo!() replaced ---"
TODO_GONE=false
for greet_file in "$PROJECT_DIR"/ws/*/src/greet.rs; do
  if [[ -f "$greet_file" ]] && ! grep -q 'todo!' "$greet_file" 2>/dev/null; then
    TODO_GONE=true
    echo "  Found implemented greet.rs at: $greet_file"
    break
  fi
done
check "todo!() removed from greet.rs" "$($TODO_GONE && echo 0 || echo 1)" 10

# Check 16: cargo test passes (10 pts)
echo ""
echo "--- Check 16: cargo test ---"
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

# Check 17: Agent posted progress comment (10 pts)
echo ""
echo "--- Check 17: progress comments ---"
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

# Check 18: No active claims for worker (10 pts)
echo "--- Check 18: claims released ---"
WORKER_CLAIMS=false
if echo "$CLAIMS_FINAL" | grep -qi "eval-worker"; then
  WORKER_CLAIMS=true
fi
check "No active claims for worker" "$($WORKER_CLAIMS && echo 1 || echo 0)" 10

# Check 19: Agent sent cleanup/idle announcement (10 pts)
echo ""
echo "--- Check 19: cleanup announcement ---"
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
if echo "$AGENT_LOG" | grep -qi "Edit.*ws/default/src\|Write.*ws/default/src"; then
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

# Friction: review-specific — agent tried to self-review
if echo "$AGENT_LOG" | grep -qi "crit lgtm.*eval-worker\|self.*review\|approve.*own"; then
  echo "FRICTION (-10): Agent attempted to self-review"
  FRICTION_PENALTY=$((FRICTION_PENALTY + 10))
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
echo "=== Review Scorecard ==="
echo "  crit review created:  $($REVIEW_CREATED && echo "YES" || echo "NO")"
echo "  review requested:     $($REVIEW_REQUESTED && echo "YES" || echo "NO")"
echo "  LGTM applied:         $($REVIEW_LGTM && echo "YES" || echo "NO")"
echo "  finish saw LGTM:      $($FINISH_SAW_LGTM && echo "YES" || echo "NO")"
echo "  worker paused:        $($WORKER_PAUSED && echo "YES" || echo "NO")"
echo "  mark-merged:          $($MARK_MERGED && echo "YES" || echo "NO")"

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "BONE_ID=$BONE_ID"
echo "REVIEW_ID=${REVIEW_ID:-not found}"
echo "REVIEW_WS=${REVIEW_WS:-not found}"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history greeter -n 50' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-worker.log' for agent output"
echo "Run 'cat $ARTIFACTS/bone-comments.txt' for bone comments"
echo "Run 'cat $ARTIFACTS/review-final.json' for review details"
echo ""
echo "=== Verification Complete ==="
