#!/usr/bin/env bash
set -euo pipefail

# R7 Phase 1: Decomposition
# Dev agent triages the feature request bone, recognizes it's too large,
# decomposes into subtasks with dependency graph, and stops before coding.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R7 Phase 1: Decomposition ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "PARENT_BEAD=$PARENT_BEAD"
echo "EVAL_DIR=$EVAL_DIR"

PROMPT="You are dev agent \"${DEV_AGENT}\" for project \"r7-eval\".
Use --agent ${DEV_AGENT} on ALL botbus commands. Set BOTBOX_PROJECT=r7-eval.

Read .agents/botbox/worker-loop.md and follow the TRIAGE step only.

Find work, groom the bone, and if the task is too large for one session, break it
into subtasks with bn create + bn triage dep add. Each subtask = one resumable unit of work.
Wire sibling dependencies where order matters. Verify with bn triage graph.

STOP after announcing your decomposition plan. Do NOT create a workspace or write code."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 1 Complete ==="
echo "Verify:"
echo "  bn triage graph                    # dependency graph"
echo "  bn next                           # first unblocked subtask(s)"
echo "  bn show $PARENT_BEAD              # decomposition plan + SQLite decision"
echo "  botbus history r7-eval --limit 10 # planning announcement"
