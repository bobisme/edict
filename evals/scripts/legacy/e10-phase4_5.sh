#!/usr/bin/env bash
set -euo pipefail

# E10 Phase 4.5: Hook Verification (automated, no agent)
# Validates that botbox init correctly registered hooks,
# particularly the mention hook for alpha-security.

source "${1:?Usage: e10-phase4_5.sh <path-to-.eval-env>}"

PHASE="phase4_5"
ARTIFACTS="$EVAL_DIR/artifacts"
mkdir -p "$ARTIFACTS"

echo "=== E10 Phase 4.5: Hook Verification ==="

# Use JSON format for reliable field access (TOON splits fields across lines)
HOOKS_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list --format json 2>&1)
echo "$HOOKS_JSON" > "$ARTIFACTS/$PHASE.hooks.json"

PASS=0
FAIL=0

# Extract the alpha-security mention hook (condition.agent == "alpha-security")
SECURITY_HOOK=$(echo "$HOOKS_JSON" | jq '[.[] | select(.condition.agent == "alpha-security")] | .[0] // empty')

# Check alpha-security mention hook exists
if [[ -n "$SECURITY_HOOK" && "$SECURITY_HOOK" != "null" ]]; then
  echo "PASS: alpha-security mention hook found"
  PASS=$((PASS + 1))
else
  echo "FAIL: alpha-security mention hook missing"
  FAIL=$((FAIL + 1))
  # Can't check further if hook doesn't exist
  echo ""
  echo "Results: $PASS passed, $FAIL failed"
  echo ""
  echo "=== Phase 4.5 Complete ==="
  [[ $FAIL -eq 0 ]]
fi

# Check hook command includes --pass-env
HOOK_CMD=$(echo "$SECURITY_HOOK" | jq -r '.command | join(" ")')
if echo "$HOOK_CMD" | grep -q "pass-env"; then
  echo "PASS: --pass-env present in hook command"
  PASS=$((PASS + 1))
else
  echo "FAIL: --pass-env missing from hook command"
  FAIL=$((FAIL + 1))
fi

# Check hook command includes vessel spawn
if echo "$HOOK_CMD" | grep -q "vessel.*spawn"; then
  echo "PASS: vessel spawn present in hook command"
  PASS=$((PASS + 1))
else
  echo "FAIL: vessel spawn missing from hook command"
  FAIL=$((FAIL + 1))
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"

{
  echo "phase: $PHASE"
  echo "pass: $PASS"
  echo "fail: $FAIL"
  echo "hook_cmd: $HOOK_CMD"
} > "$ARTIFACTS/$PHASE.results.txt"

echo ""
echo "=== Phase 4.5 Complete ==="

# Fail if any checks failed
[[ $FAIL -eq 0 ]]
