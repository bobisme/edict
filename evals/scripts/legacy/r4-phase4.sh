#!/usr/bin/env bash
set -euo pipefail

# R4 Phase 4: Reviewer — Re-review
# The reviewer verifies that fixes were actually applied in the code (not just replies),
# runs clippy, and LGTMs or re-blocks.
# Only run this if Phase 2 was a BLOCK and Phase 3 fixed the issues.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R4 Phase 4: Reviewer — Re-review ==="
echo "REVIEWER=$REVIEWER"
echo "EVAL_DIR=$EVAL_DIR"

# Find the review ID from crit state
REVIEW_ID=$(crit reviews list --format json 2>/dev/null | python3 -c 'import json,sys; reviews=json.load(sys.stdin); print(reviews[0]["review_id"])' 2>/dev/null || echo "UNKNOWN")
# Find workspace name (non-default workspace)
jj workspace update-stale 2>/dev/null || true
WS_NAME=$(maw ws list 2>/dev/null | grep -v "^Workspaces" | grep -v "^\*" | grep -v "^$" | head -1 | sed 's/^ *//' | cut -d: -f1)
if [ -z "$WS_NAME" ]; then
  # Fallback: find workspace directory
  WS_NAME=$(ls "${EVAL_DIR}/.workspaces/" 2>/dev/null | head -1 || echo "UNKNOWN")
fi

echo "REVIEW_ID=$REVIEW_ID"
echo "WS_NAME=$WS_NAME"

PROMPT="You are security reviewer agent \"${REVIEWER}\" for project \"r4-eval\".
Use --agent ${REVIEWER} on ALL crit and botbus commands.

You previously BLOCKED review ${REVIEW_ID}. The author has addressed your feedback.

IMPORTANT: The author's fixes are in the workspace, not the main branch.
The workspace code is at: ${EVAL_DIR}/.workspaces/${WS_NAME}/
Read files from the WORKSPACE path to see the fixed code.

Re-review workflow:
1. botbus inbox --agent ${REVIEWER} --channels r4-eval --mark-read
2. crit inbox --agent ${REVIEWER}
3. crit review ${REVIEW_ID} — read all threads and author replies
4. Read the CURRENT source from the WORKSPACE: cat ${EVAL_DIR}/.workspaces/${WS_NAME}/src/main.rs
   This is where the fixes were made. Do NOT read from the project root src/main.rs (that's the pre-fix version).
5. Run cargo clippy in the workspace: cd ${EVAL_DIR}/.workspaces/${WS_NAME} && cargo clippy 2>&1
6. If all issues resolved:
   crit lgtm ${REVIEW_ID} --agent ${REVIEWER} --reason \"All issues resolved: <summary>\"
   botbus send --agent ${REVIEWER} r4-eval \"Re-review: ${REVIEW_ID} — LGTM\" -L mesh -L review-done
7. If issues remain: reply on thread explaining what's still wrong, keep block

Be thorough. Read actual code from the workspace, don't just trust replies."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 4 Complete ==="
echo "Check: LGTM or still blocked?"
