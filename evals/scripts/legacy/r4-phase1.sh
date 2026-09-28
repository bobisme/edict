#!/usr/bin/env bash
set -euo pipefail

# R4 Phase 1: Dev Agent — Triage, Start, Work, Review Request
# The dev agent finds the bone, creates a workspace, implements the endpoint,
# creates a crit review, and requests the reviewer.
# Does NOT close bone or merge workspace — stops after review request.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R4 Phase 1: Dev Agent Work + Review Request ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "REVIEWER=$REVIEWER"
echo "EVAL_DIR=$EVAL_DIR"

PROMPT="You are dev agent \"${DEV_AGENT}\" for project \"r4-eval\".
Use --agent ${DEV_AGENT} on ALL botbus and crit commands. Set BOTBOX_PROJECT=r4-eval.

Execute the TRIAGE → START → WORK → REVIEW REQUEST steps below, then STOP.
Do NOT close the bone. Do NOT merge the workspace. Stop after requesting review.

1. TRIAGE:
   - Check inbox: botbus inbox --agent ${DEV_AGENT} --channels r4-eval --mark-read
   - Check ready bones: bn next
   - Groom the bone (bn show <id>): ensure it has a clear title, description, acceptance criteria. Fix anything missing.
   - Pick one task: bn next — parse for the bone ID.

2. START:
   - bn do <bone-id>
   - bus claims stake --agent ${DEV_AGENT} \"bone://r4-eval/<bone-id>\" -m \"<bone-id>\"
   - maw ws create --random — note workspace name (\$WS) and absolute path (\$WS_PATH)
   - All file operations must use the absolute workspace path. For jj: maw ws jj \$WS <args>.
   - Do NOT cd into the workspace and stay there.
   - bus claims stake --agent ${DEV_AGENT} \"workspace://r4-eval/\$WS\" -m \"<bone-id>\"
   - Announce: botbus send --agent ${DEV_AGENT} r4-eval \"Working on <bone-id>\" -L mesh -L task-claim

3. WORK:
   - Read bone details: bn show <bone-id>
   - Implement a GET /files/:name endpoint using Axum:
     - Read files from ./data directory relative to the binary's working directory
     - Return file contents with 200 OK
     - Return 404 if file not found
     - Return 500 on read errors
   - Add axum and tokio dependencies to Cargo.toml
   - Write src/main.rs with the endpoint
   - Verify it compiles: cd \$WS_PATH && cargo check
   - Describe the change: maw ws jj \$WS describe -m \"feat: add GET /files/:name endpoint\"
   - Add a progress comment: bn bone comment add <bone-id> \"Progress: implemented GET /files/:name endpoint\"

4. REVIEW REQUEST:
   - Create a crit review: crit reviews create --agent ${DEV_AGENT} --title \"feat: add GET /files/:name endpoint\" --description \"Adds file serving endpoint that reads from ./data\"
   - Note the review ID from the output.
   - Request review from ${REVIEWER}: crit reviews request <review-id> --agent ${DEV_AGENT} --reviewers ${REVIEWER}
   - Announce: botbus send --agent ${DEV_AGENT} r4-eval \"Review requested: <review-id> @${REVIEWER}\" -L mesh -L review-request

5. STOP HERE. Do NOT close the bone, do NOT merge the workspace, do NOT run finish steps.
   The review must happen before merge.

Key rules:
- All botbus and crit commands use --agent ${DEV_AGENT}
- All file operations use the absolute workspace path from maw ws create output
- Run bn commands from the project root (${EVAL_DIR}), not from inside the workspace
- Use jj (via maw ws jj), not git"

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 1 Complete ==="
echo "Verify: bone doing? workspace exists? review created? code compiles?"
