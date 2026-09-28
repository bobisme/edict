#!/usr/bin/env bash
set -euo pipefail

# R8 Adversarial Review Eval — Reviewer Run
# v3 prompt: references .agents/botbox/review-loop.md (cross-file + boundary checks).
# Same shape as R1: single reviewer, score against 65-point rubric.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R8: Adversarial Review ==="
echo "REVIEWER=$REVIEWER"
echo "EVAL_DIR=$EVAL_DIR"
echo "REVIEW_ID=$REVIEW_ID"

PROMPT="You are security reviewer agent \"${REVIEWER}\" for project \"r8-eval\".
Use --agent ${REVIEWER} on ALL crit and botbus commands.

Read the review workflow doc at .agents/botbox/review-loop.md and follow it.
Start by checking botbus inbox and crit inbox, then review each pending review.

Focus on security and correctness. Ground your findings in evidence — compiler
output, documentation, or source code — not assumptions about API behavior."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== R8 Run Complete ==="
echo "Verify:"
echo "  crit review $REVIEW_ID"
echo "  crit threads list $REVIEW_ID"
echo "  botbus history r8-eval --limit 10"
echo ""
echo "Score against v2 rubric (65 points):"
echo "  Bug 1 (race condition): found + fix + severity = 12"
echo "  Bug 2 (TOCTOU delete): found + fix + severity = 12"
echo "  Bug 3 (pagination underflow): found + fix = 6"
echo "  Blocking decision: 5"
echo "  Quality feedback (2 issues): 10"
echo "  Cross-file reasoning (download vs delete): 5"
echo "  FP resistance (2 traps, HIGH+ only): 5"
echo "  Protocol (crit + botbus): 10"
echo "  Total: 65"
