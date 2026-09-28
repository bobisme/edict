#!/usr/bin/env bash
set -euo pipefail

# R6 Parallel Dispatch Eval — Run
# Dev agent dispatches Haiku workers for independent bones in parallel,
# monitors progress, and merges completed work.

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

# Worker model — default to haiku for cost-efficient dispatch
WORKER_MODEL=${WORKER_MODEL:-haiku}

echo "=== R6: Parallel Dispatch ==="
echo "DEV_AGENT=$DEV_AGENT"
echo "EVAL_DIR=$EVAL_DIR"
echo "WORKER_MODEL=$WORKER_MODEL"
echo "BEAD1=$BEAD1  BEAD2=$BEAD2  BEAD3=$BEAD3"

PROMPT="You are lead dev agent \"${DEV_AGENT}\" for project \"r6-eval\".
Use --agent ${DEV_AGENT} on ALL bus, crit, and maw commands.
Set BOTBOX_PROJECT=r6-eval.

Your role is the LEAD DEVELOPER — you coordinate and dispatch, you do NOT implement tasks yourself.

## Instructions

### 1. Triage
Run: bn next
You should see 3 independent bones. Read each with bn show <id>.
These are all small, independent tasks — they can be done in parallel by separate workers.

### 2. Dispatch workers in parallel

For EACH bone, do these steps:
  a. Create a workspace: maw ws create --random
     Note the workspace NAME and absolute PATH from the output.
  b. Generate a worker identity: bus generate-name
  c. Mark the bone doing: bn do <id>
  d. Claim the bone: bus claims stake --agent ${DEV_AGENT} \"bone://r6-eval/<id>\" -m \"dispatched to <worker-name>\"
  e. Announce dispatch: bus send --agent ${DEV_AGENT} r6-eval \"Dispatching <worker-name> for <id>: <title>\" -L mesh -L task-claim

  f. Launch the worker as a BACKGROUND process:

     claude --model ${WORKER_MODEL} -p \"<worker-prompt>\" \\
       --dangerously-skip-permissions --allow-dangerously-skip-permissions \\
       > /tmp/<worker-name>.log 2>&1 &

     The worker prompt MUST include:
     - Worker identity (--agent <worker-name>)
     - Project name (r6-eval)
     - Bone ID and title
     - Workspace name and absolute path
     - Clear instructions (see template below)

IMPORTANT: Dispatch ALL 3 workers BEFORE waiting for any to complete. True parallel dispatch.

### Worker prompt template

\"You are worker agent <worker-name> for project r6-eval.
Use --agent <worker-name> on ALL bus commands.

Your task: bone <id> — <title>
Workspace: <ws-name> at <ws-path>

1. Read the bone: bn show <id>
2. Implement the task. All file operations use absolute path <ws-path>.
   For jj: maw ws jj <ws-name> <args>.
3. Post a progress comment: bn bone comment add <id> 'Progress: <what you did>'
4. Verify: cd <ws-path> && cargo check
5. Describe the change: maw ws jj <ws-name> describe -m '<id>: <summary>'
6. Announce completion: bus send --agent <worker-name> r6-eval 'Worker <worker-name> completed <id>: <title>' -L mesh -L task-done

Do NOT close the bone, merge the workspace, or release claims. The lead dev handles that.\"

### 3. Monitor

After dispatching all workers, monitor for completions:
- Poll: bus inbox --agent ${DEV_AGENT} --channels r6-eval --mark-read
- Also check bone comments: bn show <id> (look for worker progress updates)
- Check if worker processes are still running: jobs or check log files in /tmp/<worker-name>.log
- Wait until all 3 workers have announced completion (or a reasonable timeout ~5 min)

### 4. Merge completed work

For each completed bone:
  a. Verify changes: maw ws jj <ws-name> diff
  b. Merge workspace: maw ws merge <ws-name> --destroy
  c. Close bone: bn done <id> --reason \"Completed by <worker-name>\"
  d. Release claims: bus claims release --agent ${DEV_AGENT} \"bone://r6-eval/<id>\"
  e. Announce: bus send --agent ${DEV_AGENT} r6-eval \"Merged <id>: <title>\" -L mesh -L task-done

### 5. Final verification

Run cargo check from the project root to verify all merged code compiles together.
Announce: bus send --agent ${DEV_AGENT} r6-eval \"All 3 bones dispatched, completed, and merged. cargo check passes.\" -L mesh -L dispatch-complete

## Key rules
- You are the coordinator. Do NOT write code yourself.
- Dispatch ALL workers BEFORE waiting for completions.
- Workers use their own --agent identity. You use --agent ${DEV_AGENT}.
- If a worker appears stuck (no progress after extended waiting), check its log file and bone comments."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== R6 Run Complete ==="
echo "Verify:"
echo "  bn next                               # should be empty"
echo "  bn show $BEAD1 && bn show $BEAD2 && bn show $BEAD3  # all done"
echo "  jj log --no-graph -n 10               # worker commits merged"
echo "  bus history --agent $DEV_AGENT r6-eval # dispatch + merge announcements"
echo "  ls .workspaces/ 2>/dev/null            # should be empty"
echo "  cargo check                            # merged code compiles"
echo ""
echo "Score against rubric (70 points):"
echo "  Dispatch:   30 (parallel launch, workspaces, prompts, model)"
echo "  Monitoring: 15 (polling, detection, tracking)"
echo "  Merge:      15 (verify, merge, close, announce)"
echo "  Protocol:   10 (--agent, labels)"
echo "  Total:      70"
