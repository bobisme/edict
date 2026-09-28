#!/usr/bin/env bash
set -euo pipefail

# R4 Phase 5: Dev Agent — Merge + Finish
# Verify LGTM, mark review merged, merge workspace, close bone, release, announce.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R4 Phase 5: Dev Agent — Merge + Finish ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "EVAL_DIR=$EVAL_DIR"

# Find the review ID and workspace name from crit and maw state
REVIEW_ID=$(crit reviews list --format json 2>/dev/null | python3 -c 'import json,sys; reviews=json.load(sys.stdin); print(reviews[0]["review_id"])' 2>/dev/null || echo "UNKNOWN")
WS_NAME=$(maw ws list --format json 2>/dev/null | python3 -c 'import json,sys; ws=json.load(sys.stdin); print(ws[0]["name"])' 2>/dev/null || echo "UNKNOWN")
if [ "$WS_NAME" = "UNKNOWN" ]; then
  WS_NAME=$(ls "${EVAL_DIR}/.workspaces/" 2>/dev/null | head -1 || echo "UNKNOWN")
fi
BEAD_ID=$(bn list --state doing --format json 2>/dev/null | python3 -c 'import json,sys; bones=json.load(sys.stdin); print(bones[0]["id"])' 2>/dev/null || echo "UNKNOWN")

echo "REVIEW_ID=$REVIEW_ID"
echo "WS_NAME=$WS_NAME"
echo "BEAD_ID=$BEAD_ID"

PROMPT="You are dev agent \"${DEV_AGENT}\" for project \"r4-eval\".
Use --agent ${DEV_AGENT} on ALL botbus and crit commands.

The review ${REVIEW_ID} has been approved (LGTM). Complete the merge and finish steps.

Steps:
1. Verify LGTM: crit review ${REVIEW_ID} — confirm approval, no blocks.
2. Mark review as merged: crit reviews merge ${REVIEW_ID} --agent ${DEV_AGENT}
   IMPORTANT: Use 'crit reviews merge', NOT 'crit reviews close' (close does not exist).
3. Merge workspace: maw ws merge ${WS_NAME} --destroy
   IMPORTANT: Do NOT use -f flag. The --destroy flag handles cleanup.
   Run this from the project root (${EVAL_DIR}), not from inside the workspace.
4. Close the bone: bn done ${BEAD_ID} --reason \"Completed\"
   Run from the project root.
5. Release all claims: bus claims release --agent ${DEV_AGENT} --all
6. Announce: botbus send --agent ${DEV_AGENT} r4-eval \"Completed ${BEAD_ID}: merged ${REVIEW_ID}\" -L mesh -L task-done

Key rules:
- Run ALL commands from the project root (${EVAL_DIR})
- Do NOT cd into the workspace
- Use jj (via maw ws jj), not git
- All botbus and crit commands use --agent ${DEV_AGENT}"

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 5 Complete ==="
echo "Verify: review merged in crit? workspace destroyed? bone done? claims released?"
