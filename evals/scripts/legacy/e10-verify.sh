#!/usr/bin/env bash
set -euo pipefail

# E10 Verification Script
# Post-run automated checks across both projects, channel history,
# review state, and claim cleanup. Outputs structured PASS/FAIL results.
#
# Runs all tool commands via maw exec (v2 layout).

source "${1:?Usage: e10-verify.sh <path-to-.eval-env>}"

echo "=== E10 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo ""

PASS=0
FAIL=0
WARN=0

check() {
  local label="$1"
  local result="$2"  # 0 = pass, 1 = fail
  if [[ "$result" -eq 0 ]]; then
    echo "PASS: $label"
    PASS=$((PASS + 1))
  else
    echo "FAIL: $label"
    FAIL=$((FAIL + 1))
  fi
}

warn() {
  local label="$1"
  echo "WARN: $label"
  WARN=$((WARN + 1))
}

# ============================================================
# Alpha State
# ============================================================
echo "--- Alpha State ---"
cd "$ALPHA_DIR"

# Bone should be done
BEAD_STATUS=$(maw exec default -- bn show "$BEAD" --format json 2>/dev/null | jq -r '.[0].state // "unknown"' || echo "unknown")
check "Alpha bone $BEAD is done" "$([ "$BEAD_STATUS" = "done" ] && echo 0 || echo 1)"

# No ready bones (task is done)
READY_COUNT=$(maw exec default -- bn next --format json 2>/dev/null | jq 'length' || echo "0")
if [[ "$READY_COUNT" -gt 0 ]]; then
  warn "Alpha has $READY_COUNT ready bones (may include backlog)"
fi

# Non-default workspaces should be cleaned up
WS_COUNT=$(maw ws list --format json 2>/dev/null | jq '[.workspaces[] | select(.is_default == false)] | length' || echo "0")
check "Alpha workspaces cleaned up" "$([ "$WS_COUNT" -eq 0 ] && echo 0 || echo 1)"

# Work claims should be released
CLAIMS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list --agent "$ALPHA_DEV" 2>/dev/null || true)
# Only count bone:// and workspace:// claims — agent:// claims are managed by hooks, not the agent
WORK_CLAIM_COUNT=$(echo "$CLAIMS" | grep -cE "bone://|workspace://" || true)
check "Alpha-dev work claims released" "$([ "$WORK_CLAIM_COUNT" -eq 0 ] && echo 0 || echo 1)"

# Project should compile (from default workspace)
if maw exec default -- cargo check 2>/dev/null; then
  check "Alpha cargo check passes" 0
else
  check "Alpha cargo check passes" 1
fi

# /debug endpoint should be removed (critical fail condition)
# Check in ws/default/ where source files live in v2
ALPHA_MAIN="$ALPHA_DIR/ws/default/src/main.rs"
if grep -q '"/debug"' "$ALPHA_MAIN" 2>/dev/null || grep -q "route.*debug" "$ALPHA_MAIN" 2>/dev/null; then
  check "CRITICAL: /debug endpoint removed from alpha" 1
else
  check "CRITICAL: /debug endpoint removed from alpha" 0
fi

# api_secret should not be exposed in any handler response
# FIX: Previous check was too loose — matched struct field definitions.
# Now checks specifically for api_secret inside a Json response body (serde_json::json! macro
# or similar serialization), not just a struct field definition.
if [[ -f "$ALPHA_MAIN" ]]; then
  # Check if api_secret appears in any JSON response construction (json! macro, Json(...), etc.)
  # Exclude: struct field definitions (pub field: Type), let bindings, and constructor field assignments
  EXPOSED=false
  if grep -P 'json!\s*\(\s*\{[^}]*api_secret' "$ALPHA_MAIN" 2>/dev/null; then
    EXPOSED=true
  elif grep -P '"api_secret"\s*[:=].*state\.' "$ALPHA_MAIN" 2>/dev/null; then
    EXPOSED=true
  fi
  check "CRITICAL: api_secret not exposed in handlers" "$($EXPOSED && echo 1 || echo 0)"
else
  check "CRITICAL: api_secret not exposed in handlers (file not found)" 1
fi

# Version should be bumped
ALPHA_CARGO="$ALPHA_DIR/ws/default/Cargo.toml"
ALPHA_VERSION=$(grep '^version' "$ALPHA_CARGO" 2>/dev/null | head -1 | grep -oP '"[^"]*"' | tr -d '"' || echo "unknown")
check "Alpha version bumped (expected 0.2.0, got $ALPHA_VERSION)" "$([ "$ALPHA_VERSION" = "0.2.0" ] && echo 0 || echo 1)"

# jj log should show commits
echo ""
echo "Alpha jj log (last 5):"
maw exec default -- jj log --no-graph -n 5 2>/dev/null || echo "(jj log failed)"

echo ""

# ============================================================
# Beta State
# ============================================================
echo "--- Beta State ---"
cd "$BETA_DIR"

# cargo test should pass (including + test)
if maw exec default -- cargo test 2>/dev/null; then
  check "Beta cargo test passes" 0
else
  check "Beta cargo test passes" 1
fi

# Check that + is now accepted in validate_email
BETA_LIB="$BETA_DIR/ws/default/src/lib.rs"
if grep -q '+' "$BETA_LIB" 2>/dev/null; then
  check "Beta validate_email allows + character" 0
else
  check "Beta validate_email allows + character" 1
fi

# jj log
echo ""
echo "Beta jj log (last 5):"
maw exec default -- jj log --no-graph -n 5 2>/dev/null || echo "(jj log failed)"

echo ""

# ============================================================
# Cross-Project Communication
# ============================================================
echo "--- Cross-Project Communication ---"

# Alpha channel should have messages
ALPHA_HISTORY=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history alpha -n 30 2>/dev/null || true)
ALPHA_MSG_COUNT=$(echo "$ALPHA_HISTORY" | grep -c "." || true)
check "Alpha channel has messages" "$([ "$ALPHA_MSG_COUNT" -gt 2 ] && echo 0 || echo 1)"

# Beta channel should have cross-project message from alpha-dev
BETA_HISTORY=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history beta -n 20 2>/dev/null || true)
if echo "$BETA_HISTORY" | grep -qi "alpha-dev\|validate_email\|plus\|subaddress"; then
  check "CRITICAL: Cross-project message from alpha-dev to beta channel" 0
else
  check "CRITICAL: Cross-project message from alpha-dev to beta channel" 1
fi

# Projects registry should have both projects
PROJECTS_HISTORY=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history projects -n 10 2>/dev/null || true)
if echo "$PROJECTS_HISTORY" | grep -q "alpha" && echo "$PROJECTS_HISTORY" | grep -q "beta"; then
  check "Projects registry has both projects" 0
else
  check "Projects registry has both projects" 1
fi

# Check for review-request label
if echo "$ALPHA_HISTORY" | grep -qi "review-request\|review.*request"; then
  check "Review request message on alpha channel" 0
else
  check "Review request message on alpha channel" 1
fi

# Check for task-done label
if echo "$ALPHA_HISTORY" | grep -qi "task-done\|task.*done\|closed\|released\|v0.2.0"; then
  check "Completion announcement on alpha channel" 0
else
  check "Completion announcement on alpha channel" 1
fi

echo ""

# ============================================================
# Review State
# ============================================================
echo "--- Review State ---"
cd "$ALPHA_DIR"

# FIX: Try crit from default workspace first (review should be visible after merge),
# then fall back to stdout log grep for review ID and status.
REVIEW_FOUND=false
REVIEW_MERGED=false
ARTIFACTS="$EVAL_DIR/artifacts"

# Attempt 1: crit reviews list from default workspace
REVIEWS=$(maw exec default -- crit reviews list --format json 2>/dev/null || echo "[]")
if echo "$REVIEWS" | jq -e '.[0]' >/dev/null 2>&1; then
  REVIEW_STATUS=$(echo "$REVIEWS" | jq -r '.[-1].status // "unknown"')
  REVIEW_FOUND=true
  if [[ "$REVIEW_STATUS" = "merged" ]]; then
    REVIEW_MERGED=true
  fi
fi

# Attempt 2: grep phase stdout logs for review evidence
if ! $REVIEW_FOUND; then
  if grep -qP 'cr-[a-z0-9]+' "$ARTIFACTS/phase4.stdout.log" 2>/dev/null; then
    REVIEW_FOUND=true
    echo "  (review ID found via phase4 stdout log fallback)"
  fi
fi
if ! $REVIEW_MERGED; then
  # Check if mark-merged was executed in phase 8 (grep for the command or its output)
  if grep -qi "mark-merged\|status.*merged" "$ARTIFACTS/phase8.stdout.log" 2>/dev/null; then
    REVIEW_MERGED=true
    echo "  (merged status found via phase8 stdout log fallback)"
  fi
fi

check "Review exists" "$($REVIEW_FOUND && echo 0 || echo 1)"
check "Review marked as merged" "$($REVIEW_MERGED && echo 0 || echo 1)"

echo ""

# ============================================================
# Hook State
# ============================================================
echo "--- Hook State ---"

HOOKS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list 2>/dev/null || true)
if echo "$HOOKS" | grep -q "alpha-security"; then
  check "Alpha-security mention hook registered" 0
else
  check "Alpha-security mention hook registered" 1
fi

echo ""

# ============================================================
# Summary
# ============================================================
echo "=== Verification Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "WARN: $WARN"
echo ""

# Critical fail check
CRITICAL_FAILS=0
# Check critical conditions
if grep -q '"/debug"' "$ALPHA_DIR/ws/default/src/main.rs" 2>/dev/null; then
  echo "CRITICAL FAIL: /debug endpoint still present after Phase 6"
  CRITICAL_FAILS=$((CRITICAL_FAILS + 1))
fi
if [[ "$WORK_CLAIM_COUNT" -gt 0 ]]; then
  echo "CRITICAL FAIL: Work claims remain unreleased after Phase 8"
  CRITICAL_FAILS=$((CRITICAL_FAILS + 1))
fi
if ! echo "$BETA_HISTORY" | grep -qi "alpha-dev\|validate_email\|plus\|subaddress"; then
  echo "CRITICAL FAIL: No cross-project peer message from alpha-dev"
  CRITICAL_FAILS=$((CRITICAL_FAILS + 1))
fi

if [[ "$CRITICAL_FAILS" -gt 0 ]]; then
  echo ""
  echo "RESULT: CRITICAL FAIL ($CRITICAL_FAILS critical conditions triggered)"
elif [[ "$FAIL" -eq 0 ]]; then
  echo "RESULT: ALL CHECKS PASSED"
else
  echo "RESULT: $FAIL checks failed (review manually)"
fi

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BETA_DIR=$BETA_DIR"
echo "Run 'bus history <channel>' or 'maw exec default -- bn show <id>' for details"
echo "Phase artifacts in: $EVAL_DIR/artifacts/"
echo ""
echo "=== Verification Complete ==="
