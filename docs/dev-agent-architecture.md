# Dev Agent Architecture

The target architecture for a project-level dev agent (e.g., `terseid-dev`). This agent owns a project's development lifecycle — triaging work, executing tasks, coordinating parallel workers, and managing code review.

## Roles

| Agent | Role | Model | Lifecycle |
|-------|------|-------|-----------|
| `<project>-dev` | Lead developer. Triages, grooms, dispatches, reviews, merges. | Opus or Sonnet | Long-running loop |
| `<random-name>` | Worker. Claims one bead, implements, finishes. | Haiku (routine), Sonnet (moderate), Opus (complex) | Spawned per-task, exits when done |
| `security-reviewer` | Reviews code for security issues via seal. | Opus | Spawned on demand via vessel |

### Model Selection

- **Opus**: Planning, design, architecture decisions, code review, complex implementation. The lead dev agent uses Opus for triage/grooming and any task requiring judgment across the codebase.
- **Sonnet**: General implementation, moderate complexity tasks, lead dev fallback when Opus budget is a concern.
- **Haiku**: Routine, well-specified tasks with clear acceptance criteria. Best for pre-groomed beads where the work is straightforward (94% eval score on v2.1).

## Script Selection

| Script | Role | When to use |
|--------|------|-------------|
| `scripts/agent-loop.sh` | Worker. One task at a time, sequential. | Spawned by dev-loop for individual tasks, or standalone for simple projects with a single work queue. |
| `scripts/reviewer-loop.sh` | Reviewer. One review at a time. | Spawned by dev-loop or standalone when code reviews are pending. |
| `scripts/dev-loop.sh` | Lead dev. Triages, dispatches workers, monitors, merges. | The orchestrator for projects with multiple ready beads that benefit from parallel execution. |

**Start here:** If your project has a handful of beads and one agent is enough, use `agent-loop.sh`. When you have multiple independent beads and want parallel dispatch with model selection, use `dev-loop.sh`.

## Main Loop

`<project>-dev` runs in a loop similar to `scripts/agent-loop.sh`. Each iteration:

### 1. Inbox

Check `rite inbox --agent $AGENT --channels $PROJECT --mark-read`. Handle each message by type:
- **Task requests**: Create beads or merge into existing.
- **Status checks**: Reply on rite.
- **Review responses**: Handle reviewer comments (see Review Lifecycle below).
- **Worker announcements**: Track progress, note completions.
- **Feedback**: Triage referenced beads, reply.

### 2. Triage

Check `br ready`. Groom each bead (title, description, acceptance criteria, testing strategy, priority). Use `bv --robot-next` to decide what to work on.

### 3. Dispatch

Decide between sequential and parallel execution:

**Sequential** (default): The dev agent does the work itself — claim, start, work, finish. This is the flow validated by the agent-loop evals (Sonnet 99%, Haiku 94%).

**Parallel** (when multiple independent beads are ready): Spin up worker agents for each bead:

```bash
# For each independent bead:
vessel spawn --name <random-name> -- \
  claude -p "You are worker <name> for <project>. Complete bead <id>. ..." \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions
```

Each worker:
- Claims the bead and workspace via rite
- Implements the task in its workspace
- Runs `br close`, `maw ws merge --destroy`, releases claims
- Announces completion on `#<project>`

The dev agent doesn't wait — it continues its loop. On subsequent iterations it sees worker completions in rite and bead closures in `br ready`.

### 4. Review Lifecycle

After work is complete (either by the dev agent or a worker), if review is enabled:

**Request review:**
1. Create a seal review: `seal reviews create --title "..." --change <jj-change-id>`
2. Request reviewer: `seal reviews request <review-id> --reviewers security-reviewer`
3. Announce on rite: `rite send --agent $AGENT $PROJECT "Review requested: <review-id> @security-reviewer" -L mesh -L review-request`

**Ensure reviewer is running:**
1. Check if reviewer is active: `rite claims check --agent $AGENT "agent://security-reviewer"`
2. If not running, spawn it: `vessel spawn --name security-reviewer -- <reviewer-script>`

**Wait for review:**
The dev agent doesn't block. It continues its loop. Options:
- Sleep briefly and check next iteration (simplest)
- Use `rite wait --agent $AGENT -L review-done -t 120` for event-driven notification
- Check `seal reviews list --agent $AGENT --status=open --format=json` each iteration for review status

**Handle review response:**
On the next iteration where a review response is visible:

1. Read review: `seal review <review-id>`
2. For each thread/comment:
   - **Fix**: Make the code change in a workspace, commit, comment "Fixed in <change>"
   - **Address**: Reply explaining why the current approach is correct (won't-fix with rationale)
   - **Defer**: Create a bead for future work, comment "Filed <bead-id> for follow-up"
3. Re-request review: `rite send --agent $AGENT $PROJECT "Re-review requested: <review-id> @security-reviewer" -L mesh -L review-request`
4. Repeat until LGTM or all blockers resolved.

**Merge:**
1. Verify approval: `seal review <review-id>` — confirm LGTM, no blocks
2. Merge workspace: `maw ws merge $WS --destroy`
3. Close bead, release claims, sync, announce

### 5. Cleanup

Same as the current agent-loop finish:
- `br comments add <id> "Completed by $AGENT"`
- `br close <id> --reason="Completed" --suggest-next`
- `rite claims release --agent $AGENT --all`
- `br sync --flush-only`
- `rite send --agent $AGENT $PROJECT "Completed <id>" -L mesh -L task-done`

## Coordination Model

Agents coordinate through two channels:

**rite** — real-time messaging. Announcements, mentions, review requests. Agents check inbox each iteration. Labels (`-L review-request`, `-L task-done`, `-L review-done`) enable filtering.

**beads + seal** — persistent state. Bead status (open/in_progress/closed), seal reviews (pending/approved/blocked), comments and threads. This is the source of truth; rite messages are notifications.

Claims (`rite claims stake`) prevent conflicts:
- `agent://<name>` — agent lease (one instance at a time)
- `bead://<project>/<id>` — bead ownership
- `workspace://<project>/<ws>` — workspace ownership

## Eval Coverage

What's validated and what remains (as of multi-agent run 1, 2026-02-01):

| Capability | Eval Status | Best Score |
|-----------|-------------|------------|
| Worker loop (sequential) | ✅ 10 runs | Sonnet 99%, Haiku 94% |
| Inbox triage | ✅ 5 runs | Sonnet 99% (v3) |
| Grooming | ✅ Observed in all runs | Consistent |
| `has_work()` gating | ✅ Validated | br sync fix confirmed |
| Review request (create + announce) | ✅ R4 + multi-agent run 1 | 4/5 (request flag issue) |
| Review loop (reviewer agent) | ✅ R1 × 3, R3, R8 × 3 | Opus 100% (R1), 75% (R8v2) |
| Review response handling (fix/address/defer) | ✅ R2, R3 | Opus 100% (R2) |
| Multi-iteration coordination | ✅ Multi-agent run 1 | 80% (4 beads, 48 min) |
| Planning / epic decomposition | ✅ R7 | Opus 80% (76/95) |
| Adversarial review (subtle bugs) | ✅ R8 × 3 | Opus 75%, Sonnet 63% (FAIL) |
| Parallel dispatch | ✅ R6 × 1 | Opus 99% (69/70) |
| Cross-project coordination | ✅ R5 × 1 | Opus 100% (70/70) |
| Crash recovery | ✅ R9 × 1 | Opus 99% (69/70) |

## Eval Status

All planned evals (R1-R9) have been run. See `evals/rubrics.md` for full rubrics and `evals/results/README.md` for all run reports.
