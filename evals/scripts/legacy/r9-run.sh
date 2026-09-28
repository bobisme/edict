#!/usr/bin/env bash
set -euo pipefail

# R9 Crash Recovery Eval — Run
# Launches an agent with the SAME identity as the crashed session.
# The agent must detect the crash state, resume from subtask 3,
# and complete all remaining work without duplicating subtasks 1-2.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R9: Crash Recovery ==="
echo "AGENT=$AGENT"
echo "EVAL_DIR=$EVAL_DIR"
echo "PARENT_BEAD=$PARENT_BEAD"
echo "S3=$S3 (in_progress, crash state)"
echo "S4=$S4  S5=$S5 (ready, blocked)"
echo "WS=$WS  WS_PATH=$WS_PATH"

PROMPT="You are worker agent \"${AGENT}\" for project \"r9-eval\".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Set BOTBOX_PROJECT=r9-eval.

You are resuming after a previous session ended unexpectedly. Your previous session
may have left work in progress — claims, workspaces, partial implementations.
You must figure out what happened and continue without redoing completed work.

Execute the worker loop starting with RESUME CHECK, then continue through all
remaining work until everything is done.

## 0. RESUME CHECK (do this FIRST)

Run: bus claims --agent ${AGENT} --mine
If you hold bone:// claims, you have in-progress work from a previous session.
For each bone:// claim:
  - Run: bn show <bone-id> to understand the task and see comments
  - Check if a workspace exists: look at comments for workspace info
  - Read the workspace code to see what's already implemented
  - Determine what remains to be done

Do NOT start from scratch. Continue from where the previous session left off.

## 1. COMPLETE IN-PROGRESS WORK

For the doing bone:
- Check what's already in the workspace (read source files)
- Read the bone comments to understand what was done and what remains
- Complete the remaining work in the EXISTING workspace (do not create a new one)
- All file operations use the absolute workspace path from bone comments
- For jj commands: maw ws jj <ws-name> <args>
- Post progress: bn bone comment add <id> \"Progress: <what you did>\"
- Describe: maw ws jj <ws-name> describe -m \"<id>: <summary>\"
- Merge: maw ws merge <ws-name> --destroy
- Close: bn done <id> --reason \"Completed\"
- Release: bus claims release --agent ${AGENT} \"bone://r9-eval/<id>\"
- Announce: bus send --agent ${AGENT} r9-eval \"Completed <id>: <title>\" -L mesh -L task-done

## 2. CHECK FOR REMAINING WORK

After completing the doing bone:
- Run: bn next
- Check the parent bone and dependency tree: bn show <parent>, bn triage graph
- If there are more ready subtasks, work through them in dependency order

For each remaining subtask:
1. bn do <id>
2. bus claims stake --agent ${AGENT} \"bone://r9-eval/<id>\" -m \"<id>\"
3. maw ws create --random — note workspace NAME and absolute PATH
4. bus claims stake --agent ${AGENT} \"workspace://r9-eval/\$WS\" -m \"<id>\"
5. Announce: bus send --agent ${AGENT} r9-eval \"Working on <id>: <title>\" -L mesh -L task-claim
6. Implement in the workspace. All file operations use absolute WS_PATH.
   For jj: maw ws jj \$WS <args>. Do NOT cd into workspace and stay there.
7. bn bone comment add <id> \"Progress: ...\"
8. Verify: cd \$WS_PATH && cargo check
9. Describe: maw ws jj \$WS describe -m \"<id>: <summary>\"
10. Merge: maw ws merge \$WS --destroy
11. bn done <id> --reason \"Completed\"
12. bus claims release --agent ${AGENT} \"bone://r9-eval/<id>\"
13. bus send --agent ${AGENT} r9-eval \"Completed <id>: <title>\" -L mesh -L task-done

## 3. CLOSE PARENT

After all subtasks are done:
- bn done <parent-id> --reason \"All subtasks completed\"
- bus send --agent ${AGENT} r9-eval \"All subtasks complete, parent closed\" -L mesh -L task-done

## Key Rules

- RESUME CHECK is mandatory — you MUST check claims and bone comments first.
- Do NOT redo work that's already completed (check bone state and existing code).
- Do NOT create a new workspace for a bone that already has one.
- Use the existing workspace path from bone comments for in-progress work.
- All bus and crit commands use --agent ${AGENT}.
- Post progress comments on every subtask you work on.
- Run cargo check in each workspace before merging.
- Work through ALL remaining subtasks, not just one."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== R9 Run Complete ==="
echo "Verify:"
echo "  bn next                               # should be empty"
echo "  bn show $PARENT_BEAD                  # done"
echo "  bn show $S3 && bn show $S4 && bn show $S5  # all done"
echo "  jj log --no-graph -n 15               # subtask commits merged"
echo "  bus history --agent $AGENT r9-eval    # resume + completion announcements"
echo "  ls .workspaces/ 2>/dev/null           # should be empty (all destroyed)"
echo "  cargo check                           # merged code compiles"
echo ""
echo "Score against rubric (70 points):"
echo "  State Detection:     20 (claims check, read comments, identify completed/in-progress)"
echo "  No Duplication:      15 (didn't redo S1/S2, used existing workspace for S3)"
echo "  Recovery Execution:  25 (completed S3/S4/S5, cargo check passes)"
echo "  Protocol Compliance: 10 (--agent, progress comments, announcements, parent closed)"
echo "  Total:               70"
