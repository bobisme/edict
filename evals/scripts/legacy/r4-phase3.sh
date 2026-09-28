#!/usr/bin/env bash
set -euo pipefail

# R4 Phase 3: Dev Agent — Handle Reviewer Feedback
# The dev agent reads reviewer comments, fixes issues in the workspace,
# replies on threads, and re-requests review.
# Only run this if Phase 2 resulted in a BLOCK.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R4 Phase 3: Dev Agent — Handle Feedback ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo "EVAL_DIR=$EVAL_DIR"

# Find the review ID and workspace name from crit and maw state
REVIEW_ID=$(crit reviews list --format json 2>/dev/null | python3 -c 'import json,sys; reviews=json.load(sys.stdin); print(reviews[0]["review_id"])' 2>/dev/null || echo "UNKNOWN")
WS_NAME=$(maw ws list --format json 2>/dev/null | python3 -c 'import json,sys; ws=json.load(sys.stdin); print(ws[0]["name"])' 2>/dev/null || echo "UNKNOWN")
if [ "$WS_NAME" = "UNKNOWN" ]; then
  WS_NAME=$(ls "${EVAL_DIR}/.workspaces/" 2>/dev/null | head -1 || echo "UNKNOWN")
fi

echo "REVIEW_ID=$REVIEW_ID"
echo "WS_NAME=$WS_NAME"

PROMPT="You are dev agent \"${DEV_AGENT}\" for project \"r4-eval\".
Use --agent ${DEV_AGENT} on ALL crit and botbus commands.

Your code review ${REVIEW_ID} has been BLOCKED by reviewer ${REVIEWER}. Handle the feedback.

Workflow:
1. Check botbus inbox: botbus inbox --agent ${DEV_AGENT} --channels r4-eval --mark-read
2. Read the review: crit review ${REVIEW_ID}
3. Read all threads: crit threads list ${REVIEW_ID}
4. For each thread, read the comment and decide:
   - CRITICAL/HIGH severity → MUST FIX before merge. Fix the code in the workspace, then reply:
     crit reply <thread-id> --agent ${DEV_AGENT} \"Fixed: <description of fix>\"
   - MEDIUM severity → SHOULD FIX. Fix the code or explain why not, then reply on the thread.
   - LOW/INFO severity → OPTIONAL. Fix if trivial, otherwise acknowledge or defer.
5. After handling all comments:
   a. Verify fixes compile: cd \$WS_PATH && cargo check (where \$WS_PATH is the absolute path to .workspaces/${WS_NAME})
   b. Describe the change: maw ws jj ${WS_NAME} describe -m \"fix: address review feedback on ${REVIEW_ID}\"
   c. Re-request review: crit reviews request ${REVIEW_ID} --agent ${DEV_AGENT} --reviewers ${REVIEWER}
   d. Announce (include workspace path so the reviewer can find the fixed code):
      botbus send --agent ${DEV_AGENT} r4-eval \"Review feedback addressed: ${REVIEW_ID}, fixes in workspace ${WS_NAME} (${EVAL_DIR}/.workspaces/${WS_NAME})\" -L mesh -L review-response

The workspace is ${WS_NAME}. Use absolute paths for file operations.
Run bn commands from the project root (${EVAL_DIR}), not from inside the workspace.
Read the full source files before making changes. Verify your fixes compile with cargo check.
Use jj (via maw ws jj ${WS_NAME}), not git."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 3 Complete ==="
echo "Verify: fixes compile? threads have replies? review re-requested?"
