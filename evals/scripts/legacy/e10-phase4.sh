#!/usr/bin/env bash
set -euo pipefail

# E10 Phase 4: Alpha Resumes + Completes + Requests Review (alpha-dev, Opus)
# Alpha-dev reads beta's fix announcement, verifies tests pass,
# completes implementation, creates crit review, requests alpha-security.

source "${1:?Usage: e10-phase4.sh <path-to-.eval-env>}"

PHASE="phase4"
ARTIFACTS="$EVAL_DIR/artifacts"
mkdir -p "$ARTIFACTS"

cd "$ALPHA_DIR"

# Re-discover workspace if not in env
if [[ -z "${WS:-}" ]]; then
  WS=$(maw ws list --format json 2>/dev/null | jq -r '[.workspaces[] | select(.is_default == false)] | .[0].name // empty')
  if [[ -z "$WS" ]]; then WS="default"; fi
fi
WS_PATH="$ALPHA_DIR/ws/$WS"

echo "=== E10 Phase 4: Alpha Resumes + Review Request ==="
echo "ALPHA_DEV=$ALPHA_DEV"
echo "ALPHA_DIR=$ALPHA_DIR"
echo "BEAD=$BEAD"
echo "WS=$WS"
echo "WS_PATH=$WS_PATH"

PROMPT="You are dev agent \"${ALPHA_DEV}\" for project \"alpha\".
Your project directory is: ${ALPHA_DIR}
This project uses maw v2 (bare repo layout). Source files are in ws/default/.
Use --agent ${ALPHA_DEV} on ALL bus and crit mutation commands.
Set BOTBUS_DATA_DIR=${BOTBUS_DATA_DIR} in your environment for all bus commands.

You previously started work on bone ${BEAD} (POST /users registration) and discovered
that beta's validate_email rejected + characters. You asked beta-dev about it and they
have now fixed it.

Your workspace is: ${WS} (files at ${WS_PATH})

1. CHECK INBOX:
   - bus inbox --agent ${ALPHA_DEV} --channels alpha --mark-read
   - You should see beta-dev's fix announcement

2. VERIFY:
   - Run tests in your workspace: maw exec ${WS} -- cargo test
   - The plus-address test should now pass (beta's fix was merged to main,
     and your Cargo.toml path dependency resolves to beta's main worktree)

3. COMPLETE:
   - Finish any remaining implementation work for POST /users
   - Make sure the endpoint handles both success (201) and validation failure (400)
   - Describe: maw exec ${WS} -- jj describe -m \"feat: add POST /users registration with email validation\"

4. REQUEST REVIEW:
   - Add a progress comment:
     maw exec default -- bn bone comment add ${BEAD} \"Beta fixed validate_email. Tests pass. Implementation complete, requesting review.\"
   - Create a crit review from your workspace:
     maw exec ${WS} -- crit reviews create --agent ${ALPHA_DEV} --title \"feat: POST /users registration\"
   - Note the review ID from the output
   - Request review from alpha-security:
     maw exec ${WS} -- crit reviews request <review-id> --reviewers ${ALPHA_SECURITY} --agent ${ALPHA_DEV}
   - Announce with @mention:
     bus send --agent ${ALPHA_DEV} alpha \"Review requested: <review-id> @${ALPHA_SECURITY}\" -L review-request

5. STOP HERE. Wait for the security review.

Key rules:
- All file operations use the absolute workspace path: ${WS_PATH}/
- Run bn commands via: maw exec default -- bn ...
- Run crit/jj/cargo via: maw exec ${WS} -- <command>
- Use jj (not git) via maw exec"

echo "$PROMPT" > "$ARTIFACTS/$PHASE.prompt.md"

botbox run-agent claude -p "$PROMPT" -m opus -t 900 \
  > "$ARTIFACTS/$PHASE.stdout.log" 2> "$ARTIFACTS/$PHASE.stderr.log" \
  || echo "PHASE $PHASE exited with code $?"

# --- Discover review ID for subsequent phases ---
REVIEW_ID=$(cd "$ALPHA_DIR" && maw exec "$WS" -- crit reviews list --format json 2>/dev/null | jq -r '.[-1].review_id // empty' || true)
if [[ -z "$REVIEW_ID" ]]; then
  # Fallback: grep the agent's stdout log for a review ID (cr-XXXX)
  REVIEW_ID=$(grep -oP 'cr-[a-z0-9]+' "$ARTIFACTS/$PHASE.stdout.log" 2>/dev/null | head -1 || true)
  if [[ -n "$REVIEW_ID" ]]; then
    echo "FALLBACK: REVIEW_ID=$REVIEW_ID recovered from stdout log (crit reviews list failed)"
  fi
fi

if [[ -n "$REVIEW_ID" ]]; then
  echo "export REVIEW_ID=\"$REVIEW_ID\"" >> "$EVAL_DIR/.eval-env"
  echo "Review: $REVIEW_ID"
else
  echo "WARNING: No review ID found after Phase 4"
fi

echo ""
echo "=== Phase 4 Complete ==="
