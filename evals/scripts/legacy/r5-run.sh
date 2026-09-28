#!/usr/bin/env bash
set -euo pipefail

# R5 Cross-Project Coordination Eval — Run
# Agent works in r5-app, discovers a bug in r5-utils, and files
# a cross-project issue via report-issue.md.

cd "${APP_DIR:?Set APP_DIR first (source r5-app/.eval-env)}"
source .eval-env

echo "=== R5: Cross-Project Coordination ==="
echo "AGENT=$AGENT"
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "APP_DIR=$APP_DIR"
echo "UTILS_DIR=$UTILS_DIR"
echo "BEAD=$BEAD"

# BOTBUS_DATA_DIR must be exported so the claude session inherits it
export BOTBUS_DATA_DIR

PROMPT="You are worker agent \"${AGENT}\" for project \"r5-app\".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Set BOTBOX_PROJECT=r5-app.

Execute exactly ONE cycle of the worker loop. Complete one task (or determine there is no work),
then STOP. Do not start a second task — the outer loop handles iteration.

0. RESUME CHECK (do this FIRST):
   Run: bus claims --agent ${AGENT} --mine
   If you hold a bone:// claim, you have a doing bone from a previous iteration.
   - Run: bn show <bone-id> to understand what was done before and what remains.
   - Look for workspace info in comments (workspace name and path).
   - If a \"Review requested: <review-id>\" comment exists:
     * Check review status: crit review <review-id>
     * If LGTM (approved): proceed to FINISH (step 7) — merge the review and close the bone.
     * If BLOCKED (changes requested): follow .agents/botbox/review-response.md to fix issues
       in the workspace, re-request review, then STOP this iteration.
     * If PENDING (no votes yet): STOP this iteration. Wait for the reviewer.
   - If no review comment (work was in progress when session ended):
     * Read the workspace code to see what's already done.
     * Complete the remaining work in the EXISTING workspace — do NOT create a new one.
     * After completing: bn bone comment add <id> \"Resumed and completed: <what you finished>\".
     * Then proceed to step 6 (REVIEW REQUEST) or step 7 (FINISH).
   If no active claims: proceed to step 1 (INBOX).

1. INBOX (do this before triaging):
   Run: bus inbox --agent ${AGENT} --channels r5-app --mark-read
   For each message:
   - Task request (-L task-request or asks for work): create a bone with bn create.
   - Status check or question: reply on bus, do NOT create a bone.
   - Feedback (-L feedback): review referenced bones, reply with triage result.
   - Announcements from other agents (\"Working on...\", \"Completed...\", \"online\"): ignore, no action.
   - Duplicate of existing bone: do NOT create another bone, note it covers the request.

2. TRIAGE: Check bn next. If no ready bones and inbox created none, say \"NO_WORK_AVAILABLE\" and stop.
   GROOM each ready bone (bn show <id>): ensure clear title, description with acceptance criteria
   and testing strategy. Fix anything missing, comment what you changed.
   Use bn next to pick exactly one small task. If the task is large, break it down with
   bn create + bn triage dep add, then bn next again. If a bone is claimed
   (bus claims check --agent ${AGENT} \"bone://r5-app/<id>\"), skip it.

3. START: bn do <id>.
   bus claims stake --agent ${AGENT} \"bone://r5-app/<id>\" -m \"<id>\".
   Create workspace: run maw ws create --random. Note the workspace name AND absolute path
   from the output (e.g., name \"frost-castle\", path \"/abs/path/.workspaces/frost-castle\").
   Store the name as WS and the absolute path as WS_PATH.
   IMPORTANT: All file operations (Read, Write, Edit) must use the absolute WS_PATH.
   For bash commands: cd \$WS_PATH && <command>. For jj commands: maw ws jj \$WS <args>.
   Do NOT cd into the workspace and stay there — the workspace is destroyed during finish.
   bus claims stake --agent ${AGENT} \"workspace://r5-app/\$WS\" -m \"<id>\".
   bn bone comment add <id> \"Started in workspace \$WS (\$WS_PATH)\".
   Announce: bus send --agent ${AGENT} r5-app \"Working on <id>: <title>\" -L mesh -L task-claim.

4. WORK: bn show <id>, then implement the task in the workspace.
   Add at least one progress comment: bn bone comment add <id> \"Progress: ...\".

5. STUCK CHECK: If same approach tried twice, info missing, or tool fails repeatedly — you are
   stuck. bn bone comment add <id> \"Blocked: <details>\".
   bus send --agent ${AGENT} r5-app \"Stuck on <id>: <reason>\" -L mesh -L task-blocked.
   Release: bus claims release --agent ${AGENT} \"bone://r5-app/<id>\".
   Stop this cycle.

6. REVIEW REQUEST:
   Describe the change: maw ws jj \$WS describe -m \"<id>: <summary>\".
   Create review: crit reviews create --agent ${AGENT} --title \"<title>\" --description \"<summary>\".
   Add bone comment: bn bone comment add <id> \"Review requested: <review-id>, workspace: \$WS (\$WS_PATH)\".
   Announce: bus send --agent ${AGENT} r5-app \"Review requested: <review-id> for <id>: <title>\" -L mesh -L review-request.
   Do NOT close the bone. Do NOT merge the workspace. Do NOT release claims.
   STOP this iteration. The reviewer will process the review.

7. FINISH (only reached after LGTM from step 0, or if no review needed):
   IMPORTANT: Run ALL finish commands from the project root, not from inside the workspace.
   If your shell is cd'd into .workspaces/, cd back to the project root first.
   bn bone comment add <id> \"Completed by ${AGENT}\".
   bn done <id> --reason \"Completed\".
   maw ws merge \$WS --destroy (if conflict, preserve and announce).
   bus claims release --agent ${AGENT} --all.
   bus send --agent ${AGENT} r5-app \"Completed <id>: <title>\" -L mesh -L task-done.

Key rules:
- Exactly one small task per cycle.
- Always finish or release before stopping.
- If claim denied, pick something else.
- All bus and crit commands use --agent ${AGENT}.
- All file operations use the absolute workspace path from maw ws create output. Do NOT cd into the workspace and stay there.
- Run bn commands from the project root, NOT from .workspaces/WS/.
- If a tool behaves unexpectedly, report it: bus send --agent ${AGENT} r5-app \"Tool issue: <details>\" -L mesh -L tool-issue.
- STOP after completing one task or determining no work. Do not loop."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== R5 Run Complete ==="
echo "Verify:"
echo "  cd $APP_DIR && bn next                      # should be empty (bone done)"
echo "  cd $APP_DIR && cargo check                 # compiles"
echo "  cd $UTILS_DIR && bn next                   # should show bug bone filed by agent"
echo "  BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history r5-utils  # feedback message"
echo "  BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history r5-app    # task-claim + task-done"
echo "  ls $APP_DIR/.workspaces/ 2>/dev/null       # should be empty"
echo ""
echo "Score against rubric (70 points):"
echo "  Issue Discovery:    15 (reads r5-utils, finds bug, decides to file)"
echo "  Project Discovery:  15 (queries #projects, parses, navigates)"
echo "  Issue Filing:       20 (bone in r5-utils, clear description, repro info)"
echo "  Bus Announcement:   10 (message on r5-utils, -L feedback, @r5-utils-dev)"
echo "  Own Task:           10 (implements endpoint, correct validation)"
echo "  Total:              70"
