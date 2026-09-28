#!/usr/bin/env bash
set -euo pipefail

# R7 Phase 2: Execution
# Dev agent works through the subtasks created in Phase 1, following worker-loop.md.
# Picks unblocked subtasks, creates workspaces, implements, finishes, repeats.
# Closes parent bone after all children are done.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R7 Phase 2: Execution ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "PARENT_BEAD=$PARENT_BEAD"
echo "EVAL_DIR=$EVAL_DIR"

PROMPT="You are dev agent \"${DEV_AGENT}\" for project \"r7-eval\".
Use --agent ${DEV_AGENT} on ALL botbus commands. Set BOTBOX_PROJECT=r7-eval.

Read .agents/botbox/worker-loop.md and follow it.

You previously decomposed a large task into subtasks. Now execute them.
Pick the next unblocked subtask, work it, finish it, repeat until done.
Then close the parent bone and announce completion."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 2 Complete ==="
echo "Verify:"
echo "  bn list --format json             # all bones with state"
echo "  bn triage graph                   # all should be done"
echo "  cargo check && cargo test         # code quality"
echo "  maw ws list                       # no leaked workspaces"
echo "  botbus claims --agent $DEV_AGENT  # no active claims"
echo "  botbus history r7-eval --limit 50 # full timeline"
