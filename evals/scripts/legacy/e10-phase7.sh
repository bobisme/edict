#!/usr/bin/env bash
set -euo pipefail

# E10 Phase 7: Re-review + LGTM (alpha-security, Opus)
# Alpha-security re-reviews the code from workspace path,
# verifies /debug is removed, and LGTMs.

source "${1:?Usage: e10-phase7.sh <path-to-.eval-env>}"

PHASE="phase7"
ARTIFACTS="$EVAL_DIR/artifacts"
mkdir -p "$ARTIFACTS"

cd "$ALPHA_DIR"

# Re-discover state if not in env
if [[ -z "${WS:-}" ]]; then
  WS=$(maw ws list --format json 2>/dev/null | jq -r '[.workspaces[] | select(.is_default == false)] | .[0].name // empty')
  if [[ -z "$WS" ]]; then WS="default"; fi
fi
WS_PATH="$ALPHA_DIR/ws/$WS"

if [[ -z "${REVIEW_ID:-}" ]]; then
  REVIEW_ID=$(maw exec "$WS" -- crit reviews list --format json 2>/dev/null | jq -r '.[-1].review_id // empty' || true)
fi
if [[ -z "${REVIEW_ID:-}" ]]; then
  REVIEW_ID=$(grep -oP 'cr-[a-z0-9]+' "$ARTIFACTS/phase4.stdout.log" 2>/dev/null | head -1 || true)
  if [[ -n "$REVIEW_ID" ]]; then
    echo "FALLBACK: REVIEW_ID=$REVIEW_ID recovered from phase4 log (crit reviews list failed)"
  fi
fi

echo "=== E10 Phase 7: Re-review + LGTM ==="
echo "ALPHA_SECURITY=$ALPHA_SECURITY"
echo "REVIEW_ID=$REVIEW_ID"
echo "WS=$WS"
echo "WS_PATH=$WS_PATH"

PROMPT="You are security reviewer agent \"${ALPHA_SECURITY}\" for project \"alpha\".
Your project directory is: ${ALPHA_DIR}
This project uses maw v2 (bare repo layout). Source files are in ws/default/.
Use --agent ${ALPHA_SECURITY} on ALL crit and bus commands.
Set BOTBUS_DATA_DIR=${BOTBUS_DATA_DIR} in your environment for all bus commands.

You previously BLOCKED review ${REVIEW_ID}. The author (alpha-dev) has addressed your feedback.

1. CHECK FOR RE-REVIEW:
   - maw exec default -- crit inbox --agent ${ALPHA_SECURITY} --all-workspaces
   - Or: bus inbox --agent ${ALPHA_SECURITY} --channels alpha --mentions --mark-read

2. RE-REVIEW THE CODE:
   - maw exec ${WS} -- crit review ${REVIEW_ID} --since 5m — see what changed since your last review
   - IMPORTANT: Read the ACTUAL source files from the workspace path to verify the fix:
     Read ${WS_PATH}/src/main.rs
   - Verify: Is the /debug endpoint removed? Is the api_secret no longer exposed via any route?
   - Verify: Is the registration endpoint still correct?
   - Check for any new issues introduced by the fix

3. VOTE:
   - If the fix is satisfactory and no new issues:
     maw exec ${WS} -- crit lgtm ${REVIEW_ID} -m \"Security issue resolved. Registration endpoint looks good.\"
   - If issues remain: comment and keep the block

4. ANNOUNCE:
   - bus send --agent ${ALPHA_SECURITY} alpha \"Review ${REVIEW_ID}: LGTM @${ALPHA_DEV}\" -L review-done

Key rules:
- Read code from the WORKSPACE PATH (${WS_PATH}), not from the project root.
  The workspace contains the latest changes that haven't been merged to main yet.
- Run crit commands via: maw exec ${WS} -- crit ...
- Run crit inbox via: maw exec default -- crit inbox ..."

echo "$PROMPT" > "$ARTIFACTS/$PHASE.prompt.md"

botbox run-agent claude -p "$PROMPT" -m opus -t 600 \
  > "$ARTIFACTS/$PHASE.stdout.log" 2> "$ARTIFACTS/$PHASE.stderr.log" \
  || echo "PHASE $PHASE exited with code $?"

echo ""
echo "=== Phase 7 Complete ==="
