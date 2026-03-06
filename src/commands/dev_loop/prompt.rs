use super::journal::LastIteration;
use super::{LoopContext, SiblingLead};

/// Build the dev-loop prompt for Claude.
///
/// This is the main prompt that tells the lead agent what to do each iteration.
/// It includes status context, sibling awareness, and the full instruction set.
pub fn build(
    ctx: &LoopContext,
    last_iteration: Option<&LastIteration>,
    sibling_leads: &[SiblingLead],
    status_snapshot: Option<&str>,
) -> String {
    let agent = &ctx.agent;
    let project = &ctx.project;
    let review_str = if ctx.review_enabled { "true" } else { "false" };
    let worker_model = if ctx.worker_model.is_empty() {
        "default"
    } else {
        &ctx.worker_model
    };

    let missions_enabled = ctx.missions_enabled;
    let max_workers = ctx.missions_config.as_ref().map_or(4, |m| m.max_workers);
    let max_children = ctx.missions_config.as_ref().map_or(12, |m| m.max_children);
    let checkpoint_interval = ctx
        .missions_config
        .as_ref()
        .map_or(30, |m| m.checkpoint_interval_sec);
    let merge_timeout = ctx
        .multi_lead_config
        .as_ref()
        .map_or(60, |m| m.merge_timeout_sec);

    let push_main_step = if ctx.push_main {
        "\n  14. Push to GitHub: maw push (if fails, announce issue).".to_string()
    } else {
        String::new()
    };

    let review_instructions = format!("REVIEW is {review_str}");

    let previous_context = last_iteration
        .map(|li| {
            format!(
                "\n\n## PREVIOUS ITERATION ({}, may be stale)\n\n{}\n",
                li.age, li.content
            )
        })
        .unwrap_or_default();

    let status_section = status_snapshot
        .map(|s| format!("\n## CURRENT STATUS (pre-gathered — no need to re-fetch)\n\n{s}\n"))
        .unwrap_or_default();

    let sibling_section = if !sibling_leads.is_empty() {
        let leads_list: String = sibling_leads
            .iter()
            .map(|s| {
                let memo = if s.memo.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", s.memo)
                };
                format!("- {}{memo}", s.name)
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"
## SIBLING LEADS (multi-lead mode active)

Other lead agents are currently active on this project. Coordinate through claims — do NOT duplicate work.

Active leads:
{leads_list}

**Bone claim rule**: Before starting work on ANY bone, check if it is already claimed:
  bus claims list --format json
  Look for `bone://{project}/<id>` — if claimed by another agent, SKIP that bone and pick the next one.
  Only work on bones you can successfully claim.
"#
        )
    } else {
        String::new()
    };

    let edict_mission_env = std::env::var("EDICT_MISSION").ok();

    let mission_triage = if missions_enabled {
        let mission_focus = edict_mission_env
            .as_deref()
            .map(|m| format!("EDICT_MISSION=\"{m}\" — prioritize this mission's children."))
            .unwrap_or_default();
        format!(
            r#"
### Mission-Aware Triage

Check for active missions (bones with label "mission" that are doing):
  maw exec default -- bn list -l mission --state doing --json
{mission_focus}
For each active mission:
  1. List children: maw exec default -- bn list -l "mission:<mission-id>" --json
  2. Count status: N open, M doing, K done, J blocked
  3. If any children are ready (open, unblocked): include them in the dispatch plan
  4. If all children are done: close the mission bone (see step 5c "Closing a Mission")
  5. If children are blocked: investigate — can you unblock them? Reassign?
"#
        )
    } else {
        String::new()
    };

    let mission_level4_disabled = if missions_enabled {
        String::new()
    } else {
        " (DISABLED — missionsEnabled is false)".to_string()
    };

    let mission_level4_signals = if missions_enabled {
        "\n**Level 4 signals:** Task mentions multiple components, description reads like a spec/PRD, human explicitly requested coordinated work (EDICT_MISSION env), or bones share a common feature/goal."
    } else {
        ""
    };

    let mission_triage_option = if missions_enabled {
        "\n- Large task needing decomposition: create a mission (follow step 5c below). Mission children MUST be dispatched to workers — solo sequential work defeats the purpose."
    } else {
        ""
    };

    let mission_section_5c = if missions_enabled {
        let mission_env_note = edict_mission_env
            .as_deref()
            .map(|m| format!("\nEDICT_MISSION is set to \"{m}\" — focus on this mission.\n"))
            .unwrap_or_default();

        format!(
            r#"
## 5c. MISSION (Level 4 — large task decomposition)

Use when: a large coherent task needs decomposition into related bones with shared context.
{mission_env_note}
### Creating a Mission

1. Create the mission bone (if not already created by !mission handler):
   maw exec default -- bn create \
     --title "<mission title>" --labels mission --kind task --urgency default \
     --description "Outcome: <what done looks like>\nSuccess metric: <how to verify>\nConstraints: <scope/budget/forbidden>\nStop criteria: <when to stop>"
2. Plan decomposition: break the mission into {max_children} or fewer child bones.
   Consider dependencies between children — which can run in parallel, which are sequential.
3. Create child bones:
   For each child:
   maw exec default -- bn create \
     --title "<child title>" --parent <mission-id> \
     --labels "mission:<mission-id>" --kind task --urgency default
4. Wire dependencies between children if needed:
   maw exec default -- bn dep add <blocked-child> <blocker-child>
5. Post plan to channel:
   bus send --agent {agent} {project} "Mission <mission-id>: <title> — created N child bones" -L task-claim

### Dispatch Mission Workers

IMPORTANT: You MUST dispatch workers for independent children. Do NOT implement them yourself sequentially.
The whole point of missions is parallel execution — doing children sequentially defeats the purpose and wastes time.
Use `vessel spawn` for mission workers — NOT the Task tool. See step 5b for why.

For independent children (unblocked), dispatch workers (max {max_workers} concurrent):
- Follow the same dispatch pattern as step 5b — INCLUDING claim staking for EACH worker:
  bus claims stake --agent {agent} "bone://{project}/<child-id>" -m "dispatched to <worker-name>"
  bus claims stake --agent {agent} "workspace://{project}/$WS" -m "<child-id>"
- Add mission labels and sibling context env vars:
    --label "mission:<mission-id>" \
    --env "EDICT_MISSION=<mission-id>" \
    --env "EDICT_MISSION_OUTCOME=<outcome from mission bone description>" \
    --env "EDICT_SIBLINGS=<sibling-id> (<title>) [owner:<owner>, status:<status>]\n..." \
    --env "EDICT_FILE_HINTS=<sibling-id>: likely edits <files>\n..." \

Build the sibling context BEFORE dispatching:
1. List all children: maw exec default -- bn list -l "mission:<mission-id>" --json
2. For each child: extract id, title, owner, status
3. Format EDICT_SIBLINGS as one line per child: "<id> (<title>) [owner:<owner>, status:<status>]"
4. Estimate file ownership hints from bone titles/descriptions (advisory, not enforced)
5. Extract the Outcome line from the mission bone description for EDICT_MISSION_OUTCOME

- Include mission context in each worker's bone comment:
  maw exec default -- bn bone comment add <child-id> \
    "Mission context: <mission-id> — <outcome>. Siblings: <sibling-ids>."

### Checkpoint Loop (step 17)

After dispatching workers, enter a checkpoint loop. Run checkpoints every {checkpoint_interval} seconds.

Each checkpoint:
1. Count children by status:
   maw exec default -- bn list --all -l "mission:<mission-id>" --json
   Tally: N open, M doing, K done, J blocked
2. Check alive workers:
   vessel list --format json
   Cross-reference with dispatched worker names ({agent}/<suffix>)
3. Check for completions (cursor-based — track last-seen message ID to avoid rescanning):
   bus history {project} -n 20 -L task-done --since <last-checkpoint-time>
   Look for "Completed <bone-id>" messages from workers
4. Post checkpoint to channel (REQUIRED — crash recovery depends on this):
   bus send --agent {agent} {project} "Mission <mission-id> checkpoint: K/$TOTAL done, J blocked, M active" -L feedback
   If this session crashes, the next iteration uses these messages to reconstruct mission state.
5. Detect failures:
   If a worker is not in vessel list but its bone is still doing → crash recovery (see step 6)
6. Decide:
   - All children closed → exit checkpoint loop, proceed to Mission Close (step 18)
   - Some blocked, none doing → investigate blockers or rescope
   - Workers still alive → continue checkpoint loop

Exit the checkpoint loop when: all children are closed, OR no workers alive and all remaining bones are blocked.

### Mission Close and Synthesis (step 18)

When all children are closed:
1. Verify: maw exec default -- bn list -l "mission:<mission-id>" — all should be closed
2. Write mission log as a bone comment (synthesis of what happened):
   maw exec default -- bn bone comment add <mission-id> \
     "Mission complete.\n\nChildren: N total, all closed.\nKey decisions: <what changed during execution>\nWhat worked: <patterns that succeeded>\nWhat to avoid: <patterns that failed>\nKey artifacts: <files/modules created or modified>"
3. Close the mission: maw exec default -- bn done <mission-id> --reason "All children completed"
4. Announce: bus send --agent {agent} {project} "Mission <mission-id> complete: <title> — N children, all done" -L task-done
"#
        )
    } else {
        String::new()
    };

    let multi_lead_rules = if ctx.multi_lead_enabled {
        "\n- MULTI-LEAD: Other leads may be active. Always check bone claims before picking work. Use bus claims stake to claim bones atomically — if it fails, another lead got there first. Skip to the next bone.".to_string()
    } else {
        String::new()
    };

    let mission_rules = if missions_enabled {
        let mission_focus = edict_mission_env
            .as_deref()
            .map(|m| format!(" Focus on mission: {m}"))
            .unwrap_or_default();
        format!(
            "\n- MISSIONS: Enabled. Max {max_workers} concurrent workers, max {max_children} children per mission. Checkpoint every {checkpoint_interval}s.{mission_focus}\n- COORDINATION: Watch for coord:interface, coord:blocker, coord:handoff labels on bus messages from workers. React to coord:blocker by unblocking or reassigning."
        )
    } else {
        String::new()
    };

    // Worker dispatch command (built into edict binary)
    let worker_cmd = "edict run worker-loop";
    let project_dir = &ctx.project_dir;
    let worker_timeout = ctx.worker_timeout;

    // Build extra --env flags from config [env] section for vessel spawn template
    let spawn_env_flags: String = ctx
        .spawn_env
        .iter()
        .map(|(k, v)| format!("    --env \"{k}={v}\" \\"))
        .collect::<Vec<_>>()
        .join("\n");

    let worker_memory_limit_flag = ctx
        .worker_memory_limit
        .as_deref()
        .map(|limit| format!("    --memory-limit {limit} \\\n"))
        .unwrap_or_default();

    let check_command = ctx.check_command.as_deref().unwrap_or("just check");

    format!(
        r#"You are lead dev agent "{agent}" for project "{project}".

IMPORTANT: Use --agent {agent} on ALL bus and seal commands. bn resolves agent identity from $AGENT/$BOTBUS_AGENT env automatically. Set EDICT_PROJECT={project}. {review_instructions}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread bus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.

COMMAND PATTERN — maw exec: All bn commands run in the default workspace. All seal commands run in their workspace.
  bn:   maw exec default -- bn <args>
  seal: maw exec $WS -- seal <args>
  git:  maw exec $WS -- git <args>
  other: maw exec $WS -- <command>           (cargo test, etc.)
Inside `maw exec <ws>`, CWD is already `ws/<ws>/`. Use `maw exec default -- ls src/`, NOT `maw exec default -- ls ws/default/src/`
For file reads/edits outside maw exec, use the full absolute path: `ws/<ws>/src/...`
VERSION CONTROL: This project uses Git + maw. Do NOT run jj commands.
{previous_context}{status_section}{sibling_section}Execute exactly ONE dev cycle. Triage inbox, assess ready bones, either work on one yourself
or dispatch multiple workers in parallel, monitor progress, merge results. Then STOP.

At the end of your work, output:
1. A summary for the next iteration: <iteration-summary>Brief summary of what you did: bones worked on, workers dispatched, reviews processed, etc.</iteration-summary>
2. Completion signal:
   - <promise>COMPLETE</promise> if you completed work or determined no work available
   - <promise>END_OF_STORY</promise> if iteration done but more work remains

## 1. UNFINISHED WORK CHECK (do this FIRST — crash recovery)

Check CURRENT STATUS above for UNFINISHED BEADS. If none listed, skip to step 2.

If any doing bones are owned by you, you have unfinished work from a previous session that was interrupted.

For EACH unfinished bone:
1. Read the bone and its comments: maw exec default -- bn show <id> and maw exec default -- bn comments <id>
2. Check if you still hold claims: bus claims list --agent {agent} --mine
3. Determine state:
   - If "Review created: <review-id>" comment exists:
     * Find the review: maw exec $WS -- seal review <review-id>
     * Check review status: maw exec $WS -- seal review <review-id>
     * If LGTM (approved): Proceed to merge/finish (step 7 — use "Already reviewed and approved" path)
     * If BLOCKED (changes requested): fix the issues, then re-request review:
       1. Read threads: maw exec $WS -- seal review <review-id> (threads show inline with comments)
       2. For each unresolved thread with reviewer feedback:
          - Fix the code in the workspace (use absolute WS_PATH for file edits)
          - Reply: maw exec $WS -- seal reply <thread-id> --agent {agent} "Fixed: <what you did>"
          - Resolve: maw exec $WS -- seal threads resolve <thread-id> --agent {agent}
       3. Commit changes: maw exec $WS -- git add -A && maw exec $WS -- git commit -m "<id>: <summary> (addressed review feedback)"
       4. Re-request: maw exec $WS -- seal reviews request <review-id> --reviewers {project}-security --agent {agent}
       5. Announce: bus send --agent {agent} {project} "Review updated: <review-id> — addressed feedback @{project}-security" -L review-response
       STOP this iteration — wait for re-review
     * If PENDING (no votes yet): STOP this iteration — wait for reviewer
     * If review not found: DO NOT merge or create a new review. The reviewer may still be starting up (hooks have latency). STOP this iteration and wait. Only create a new review if the workspace was destroyed AND 3+ iterations have passed since the review comment.
   - If workspace comment exists but no review comment (work was in progress when session died):
     * Extract workspace name from comments
     * Verify workspace still exists: maw ws list
     * If workspace exists: Resume work in that workspace, complete the task, then proceed to review/finish
     * If workspace was destroyed: Re-create workspace and resume from scratch (check comments for what was done)
   - If no workspace comment (bone was just started):
     * Re-create workspace and start fresh

After handling all unfinished bones, proceed to step 2 (RESUME CHECK).

## 2. RESUME CHECK (check for active claims)

Try protocol command: edict protocol resume --agent {agent}
Read the output carefully. If status is Resumable or HasResources, follow the suggested commands.
If it fails (exit 1 = command unavailable), fall back to manual resume check:
  Check CURRENT STATUS above for ACTIVE CLAIMS. If none listed, skip to step 3.

  If you hold any claims not covered by unfinished bones in step 1:
  - bone:// claim with review comment: Check seal review status. If LGTM, proceed to merge/finish.
  - bone:// claim without review: Complete the work, then review or finish.
  - workspace:// claims: These are dispatched workers. Skip to step 7 (MONITOR).

  If no additional claims: proceed to step 2.5 (ORPHAN CLEANUP).

## 2.5. ORPHAN CLEANUP (detect and release stale claims)

Try protocol command: edict protocol cleanup --agent {agent}
Read the output carefully. If status is HasResources, run the suggested cleanup commands.
If it fails (exit 1 = command unavailable), fall back to manual cleanup:
  Check for orphaned claims from completed or failed work:
  1. List all your claims: bus claims list --agent {agent} --mine --format json
  2. For each bone:// claim:
     - Extract bone ID from the URI (e.g., "bone://{project}/bd-abc" → "bd-abc")
     - Check bone status: maw exec default -- bn show <id> --json
     - If bone is done or blocked: the claim is orphaned
     - Release it: bus claims release --agent {agent} "bone://{project}/<id>"
     - If there's a matching workspace:// claim, release that too:
       bus claims release --agent {agent} "workspace://{project}/<ws>"
  3. For each workspace:// claim without a matching bone:// claim:
     - Extract workspace name from the URI
     - Check if workspace exists: maw ws list --format json
     - If workspace doesn't exist: release the claim
       bus claims release --agent {agent} "workspace://{project}/<ws>"

This cleanup prevents orphaned claims from blocking other agents.

## 3. INBOX

Check CURRENT STATUS above for INBOX messages. If none, skip to step 4.
To process and mark read: bus inbox --agent {agent} --channels {project} --mark-read

Process each message:
- Task requests (-L task-request): create bones with maw exec default -- bn create
- Feedback (-L feedback): if it contains a bug report, feature request, or actionable work — create a bone. Evaluate critically: is this a real issue? Is it well-scoped? Set priority accordingly. Then acknowledge on bus.
- Status/questions: reply on bus
- Announcements ("Working on...", "Completed...", "online"): ignore, no action
- Duplicate requests: note existing bone, don't create another

## 4. TRIAGE

Run: maw exec default -- bn triage
This gives you scored top picks, blockers, quick wins, and project health in one command.
Use `maw exec default -- bn next N` to get top N triaged bones for dispatch (e.g., `bn next 4` for 4 workers).
If no actionable bones and inbox created none:
  bus send --agent {agent} {project} "No ready bones found — nothing to work on. Use 'bn triage' to check backlog or send a task request." -L agent-idle
  output <promise>COMPLETE</promise> and stop.
{mission_triage}
GROOM each ready bone:
- maw exec default -- bn show <id> — ensure clear title, description, acceptance criteria, priority, and risk label
- Evaluate as lead dev: is this worth doing now? Is the approach sound? Reprioritize, close as wontfix, or ask for clarification if needed.
- RISK ASSESSMENT: If the bone lacks a risk label, assign one based on: blast radius, data sensitivity, reversibility, dependency uncertainty.
  - risk:low — typo fixes, doc updates, config tweaks (maw exec default -- bn bone tag <id> risk:low)
  - risk:medium — standard features/bugs (default, no label needed)
  - risk:high — security-sensitive, data integrity, user-visible behavior changes
  - risk:critical — irreversible actions, migrations, regulated changes
  Risk can be escalated upward by any agent. Downgrades require lead approval with justification comment.
- Comment what you changed: maw exec default -- bn bone comment add <id> "..."
- CLAIM CHECK: Before working on or dispatching a bone, verify it is not already claimed by another agent:
  bus claims list --format json — look for bone://{project}/<id>. If claimed by another agent, skip it.

## EXECUTION LEVEL DECISION

After grooming, decide the execution level for this iteration:

| Level | Name | When to use |
|-------|------|-------------|
| 2 | Sequential | 1 small clear bone, or tightly coupled bones (same files, must be done in order) |
| 3 | Parallel dispatch | 2+ independent bones unrelated to each other. Different bugs, unrelated features. |
| 4 | Mission | Large task needing decomposition into related bones with shared context{mission_level4_disabled} |

**Level 3 vs 4:** Level 3 dispatches workers for *pre-existing independent bones*. Level 4 *creates the bones as part of planning* under a mission envelope with shared outcome, constraints, and sibling awareness.
{mission_level4_signals}
Assess bone count:
- 0 ready bones (but dispatched workers pending): just monitor, skip to step 7.
- 1 ready bone: do it yourself sequentially (follow steps 5a below).
- 2+ independent ready bones: dispatch workers in parallel (follow steps 5b below). Do NOT work on them yourself sequentially — parallel dispatch is REQUIRED.{mission_triage_option}

## 5a. SEQUENTIAL (1 bone — do it yourself)

START: Try protocol command: edict protocol start <bone-id> --agent {agent}
Read the output carefully. If status is Ready, run the suggested commands.
If it fails (exit 1 = command unavailable), fall back to manual start:
  1. maw exec default -- bn do <id>
  2. bus claims stake --agent {agent} "bone://{project}/<id>" -m "<id>"
  3. maw ws create --random — note workspace NAME and absolute PATH
  4. bus claims stake --agent {agent} "workspace://{project}/$WS" -m "<id>"
  5. maw exec default -- bn bone comment add <id> "Started in workspace $WS ($WS_PATH)"
  6. bus statuses set --agent {agent} "Working: <id>" --ttl 30m
  7. Announce: bus send --agent {agent} {project} "Working on <id>: <title>" -L task-claim

WORK:
8. Implement the task. All file operations use absolute WS_PATH.
   For commands in workspace: maw exec $WS -- <command>. Do NOT cd into workspace and stay there.
9. maw exec default -- bn bone comment add <id> "Progress: ..."
10. Commit: maw exec $WS -- git add -A && maw exec $WS -- git commit -m "<id>: <summary>"

REVIEW (risk-aware):
Check the bone's risk label (maw exec default -- bn show <id>). No risk label = risk:medium.

RISK:LOW (evals, docs, tests, config) — Self-review and merge directly:
  No security review needed regardless of REVIEW setting.
  Add self-review comment: maw exec default -- bn bone comment add <id> "Self-review (risk:low): <what you verified>"
  Proceed directly to merge/finish below.

RISK:MEDIUM — Standard review (if REVIEW is true):
  Try protocol command: edict protocol review <bone-id> --agent {agent}
  Read the output carefully. If status is Ready, run the suggested commands.
  If it fails (exit 1 = command unavailable), fall back to manual review:
    CHECK for existing review: maw exec default -- bn comments <id> | grep "Review created:"
    Create review with reviewer (if none exists): maw exec $WS -- seal reviews create --agent {agent} --title "<id>: <title>" --description "<summary>" --reviewers {project}-security
    IMMEDIATELY record: maw exec default -- bn bone comment add <id> "Review created: <review-id> in workspace $WS"
    Spawn reviewer via @mention: bus send --agent {agent} {project} "Review requested: <review-id> for <id> @{project}-security" -L review-request
  STOP this iteration — wait for reviewer.

RISK:HIGH — Security review + failure-mode checklist:
  Same as risk:medium, but add to review description: "risk:high — failure-mode checklist required."
  MUST request security reviewer. STOP.

RISK:CRITICAL — Security review + human approval:
  Same as risk:high, but also post: bus send --agent {agent} {project} "risk:critical review for <id>: requires human approval before merge" -L review-request
  STOP.

If REVIEW is false:
  Merge: maw ws merge $WS --destroy --message "feat: <bone-title>" (use conventional commit prefix; produces linear squashed history and auto-moves main)
  maw exec default -- bn done <id> --reason="Completed"
  bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done
  bus claims release --agent {agent} --all
  (bn is event-sourced — no sync needed){push_main_step}

## 5b. PARALLEL DISPATCH (2+ bones)

For EACH independent ready bone, assess and dispatch:

### Model Selection
Read each bone (maw exec default -- bn show <id>) and select a tier based on complexity:
- **fast**: Small scope, clear criteria. E.g., add endpoint, fix typo, update config, simple test.
- **balanced**: Multiple files, moderate complexity. E.g., refactor module, add feature with tests, wire up integration.
- **strong**: Deep debugging, architecture, complex algorithms. E.g., fix race condition, redesign data flow.

Default from config: **{worker_model}**. Override per-bone based on your complexity assessment.

IMPORTANT: Always pass the tier name (fast, balanced, strong) as `--model`, NOT a specific provider/model string.
The worker resolves tier names to a provider pool at runtime for cross-provider load balancing.

### For each bone being dispatched:
1. maw ws create --random — note NAME and PATH
2. bus generate-name — get a worker identity
3. maw exec default -- bn do <id>
4. bus claims stake --agent {agent} "bone://{project}/<id>" -m "dispatched to <worker-name>"
5. bus claims stake --agent {agent} "workspace://{project}/$WS" -m "<id>"
6. maw exec default -- bn bone comment add <id> "Dispatched worker <worker-name> (model: <model>) in workspace $WS ($WS_PATH)"
7. bus statuses set --agent {agent} "Dispatch: <id>" --ttl 5m
8. bus send --agent {agent} {project} "Dispatching <worker-name> for <id>: <title>" -L task-claim

### Spawning Workers

IMPORTANT: You MUST use `vessel spawn` to create workers. Do NOT use Claude Code's built-in Task tool for worker dispatch.
Why: vessel workers are independently observable (`vessel tail`, `vessel list`), survive your session crashing,
have independent timeouts, participate in botbus coordination (claims, messages, status), and respect maxWorkers limits.
The Task tool creates in-process subagents that bypass all of this infrastructure — no crash recovery, no observability, no coordination.

For each dispatched bone, spawn a worker via vessel with hierarchical naming:

  vessel spawn --name "{agent}/<worker-suffix>" \
    --label worker --label "bone:<id>" \
    --env-inherit BOTBUS_CHANNEL,BOTBUS_DATA_DIR,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT \
    --env "AGENT={agent}/<worker-suffix>" \
    --env "EDICT_BONE=<id>" \
    --env "EDICT_WORKSPACE=$WS" \
    --env "BOTBUS_CHANNEL={project}" \
    --env "EDICT_PROJECT={project}" \
{spawn_env_flags}
{worker_memory_limit_flag}    --timeout <model-timeout> \
    --cwd {project_dir}/ws/$WS \
    -- {worker_cmd} --model <selected-model> --agent {agent}/<worker-suffix>

Set --timeout to {worker_timeout} (from config agents.worker.timeout).

The hierarchical name ({agent}/<suffix>) lets you find all your workers via `vessel list`.
The EDICT_BONE and EDICT_WORKSPACE env vars tell the worker-loop to skip triage and go straight to the assigned work.

COLLISION GUARD: NEVER dispatch a worker for a bone you are currently working on or have already started fixing yourself this iteration. If you took over a failed worker's bone, do NOT also dispatch a new worker for it.

After dispatching all workers, skip to step 6 (MONITOR).
{mission_section_5c}
## 6. MONITOR (if workers are dispatched)

IMPORTANT: Do NOT end the iteration early just to poll again. Ending the iteration and starting a new one
burns a full Claude API call. Instead, WAIT for workers to finish using one of these approaches:

**Preferred: vessel wait --exited** — blocks until any dispatched worker exits (success or crash):

  vessel wait --exited <worker-1> <worker-2> <worker-3> -t 300

This detects BOTH clean exits and silent crashes. Zero tokens consumed while waiting.
When it returns, check which workers exited and whether they succeeded or died.

**Alternative: sleep** — if you want to check periodically:

  sleep 120

Then check for completions. This is fine — sleep does NOT consume tokens.

**Do NOT use `bus wait -L task-done`** — workers that crash or hang never post a task-done message,
so the lead hangs until timeout without detecting the failure.

**Do NOT** output END_OF_STORY while workers are still running unless you've done a dead-worker check
and confirmed all workers are alive. Each unnecessary iteration wastes tokens.

After waiting:

Check worker results:
- vessel list — which workers are still alive vs exited
- bus inbox --agent {agent} --channels {project} -n 20 — completion messages
- Check workspace status: maw ws list

For each completed worker:
- Read their progress comments: maw exec default -- bn comments <id>
- Verify the work looks reasonable (spot check key files)

### Crash Recovery (dead worker detection)

Check which workers are still alive: vessel list --format json
Cross-reference with your dispatched bones (check your bone:// claims).

For each dispatched bone where the worker is NOT in vessel list but the bone is still doing:
1. Check bone comments for a "RETRY:1" marker (from a previous crash recovery attempt).
2. If NO retry marker — first failure, reassign once:
   - maw exec default -- bn bone comment add <id> "Worker <worker-name> died. RETRY:1 — reassigning."
   - Check if workspace still exists (maw ws list). If destroyed, create a new one.
   - Re-dispatch following step 5b (new worker name, same or new workspace).
3. If "RETRY:1" marker already exists — second failure, block the bone:
   - maw exec default -- bn bone comment add <id> "Worker died again after retry. Blocking bone."
   - maw exec default -- bn bone tag <id> blocked
   - bus send --agent {agent} {project} "Bone <id> blocked: worker died twice" -L task-blocked
   - If workspace still exists: maw ws destroy <ws> (don't merge broken work)
   - bus claims release --agent {agent} "bone://{project}/<id>"

## 7. FINISH (merge completed work)

For each completed bone with a workspace, ALWAYS try protocol merge first:

  edict protocol merge <workspace> --message "feat: <bone-title>" --agent {agent}

Get the bone title from: maw exec default -- bn show <id>
Use the appropriate conventional commit prefix: feat: for features, fix: for bugs, chore: for maintenance.

This checks bone status, review gate, and conflicts in one step. Read the output:
  - Ready → proceed with Merge Protocol below, then follow the post-merge steps from the output
    (mark review merged, sync bones, announce).
  - NeedsReview → review required before merging (see NeedsReview handling below).
  - Blocked → read diagnostics for recovery steps (conflicts, bone not closed, review blocked).
  - Unavailable (exit 1) → fall back to manual merge paths at the bottom of this section.

After protocol merge reports Ready, close the bone:
  maw exec default -- bn done <id> --reason="Completed"
  bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done

### Merge Protocol (used by all paths that call "maw ws merge")

Every merge into default MUST follow this protocol to prevent concurrent merge conflicts:

  a0. COMMIT WORKER FILES (critical — workers may have uncommitted changes):
      Workers may edit files without committing. Ensure changes are committed before merge:
        maw exec $WS -- git add -A && maw exec $WS -- git commit -m "<id>: worker changes" --allow-empty
      If you skip this step, maw ws merge may miss uncommitted worker changes.

  a. PREFLIGHT CHECK (outside mutex — early conflict detection):
     maw ws merge $WS --check
     If conflicts are detected, resolve them before acquiring the mutex.

  b. ACQUIRE MERGE MUTEX:
     bus claims stake --agent {agent} "workspace://{project}/default" --ttl 120 -m "merging $WS for <id>"
     If the claim fails (held by another agent): retry with backoff+jitter.
     Retry delays: 2s, 4s, 8s, 15s — each with +-30% random jitter.
     Between retries, check: bus history {project} -L coord:merge -n 1 --since "2 minutes ago"
     If a new coord:merge appeared since your last attempt, retry immediately (the lock may be free).
     If still held after {merge_timeout}s total: post to bus and skip this merge for now.

  c. CONFLICT CHECK (under mutex — catches merges that landed during wait):
     maw ws merge $WS --check
     If conflicts: resolve them before proceeding. Use this workflow:
       1. Identify the bone: maw exec default -- bn show <id> — read what the worker was trying to do
       2. See what changed in main recently: maw exec default -- git log --oneline -10
       3. Read the conflicted files in the workspace (ws/$WS/<path>) — understand both sides
       4. Resolve by keeping BOTH sides' intent: the worker's new code AND main's recent additions.
          For registry files (match arms, mod declarations, use statements), this usually means keeping all entries.
       5. After resolving: maw exec $WS -- git add -A && maw exec $WS -- git commit -m "<id>: <summary> (conflict resolved)"

  d. MERGE:
     maw ws merge $WS --destroy --message "feat: <bone-title>"
     (Use a conventional commit prefix: feat: for features, fix: for bugs, chore: for maintenance, etc.
      Replace <bone-title> with the actual bone title from `bn show <id>`.)

  d2. RELEASE WORKER CLAIMS (for dispatched workers only):
      For workspace that was dispatched to a worker (check bone comments for "Dispatched worker"):
        bus claims release --agent {agent} "bone://{project}/<id>"
        bus claims release --agent {agent} "workspace://{project}/$WS"
      For work you did yourself (no "Dispatched worker" comment): skip this step.

  d3. POST-MERGE CHECK:
      After merging, verify compilation in the default workspace:
        maw exec default -- {check_command}
      If it fails, the merge introduced a semantic conflict — two workers' code compiles alone but
      not together (e.g., disagreeing type signatures, duplicate symbol names, missing imports).
      Fix it immediately:
        - Read the error output carefully — compiler errors point to the exact incompatibility
        - Check the bone for context: maw exec default -- bn show <id> — what was the worker's intent?
        - Common patterns: mismatched function signatures (reconcile the API), duplicate definitions
          (keep one, update callers), missing imports (add them)
        - Make targeted fixes in the default workspace (use maw exec default -- to edit)
        - Re-run the check until it passes
        - Announce: bus send --agent {agent} {project} "Post-merge fix: <what broke and how you fixed it>" -L coord:merge

  e. ANNOUNCE:
     bus send --agent {agent} {project} "Merged $WS (<id>): <summary>" -L coord:merge

  f. SYNC:
     (bn is event-sourced — no sync needed){push_main_step}

  g. RELEASE MUTEX (always, even on failure — use try/finally):
     bus claims release --agent {agent} "workspace://{project}/default"

### NeedsReview handling (protocol merge returned NeedsReview):

  CHECK for existing review: maw exec default -- bn comments <id> | grep "Review created:"
  Create review (if none): maw exec $WS -- seal reviews create --agent {agent} --title "<id>: <title>" --description "<summary>" --reviewers {project}-security
  Record: maw exec default -- bn bone comment add <id> "Review created: <review-id> in workspace <ws-name>"
  Announce: bus send --agent {agent} {project} "Review requested: <review-id> for <id> @{project}-security" -L review-request
  STOP — wait for reviewer
  For risk:high add failure-mode checklist to review description, for risk:critical add human approval request.

### Manual fallback (only if protocol merge is unavailable):

  Try protocol command: edict protocol finish <bone-id> --agent {agent}
  It will tell you the review status and exact commands to run.
  If it fails (exit 1 = command unavailable), fall back to the manual paths below.

  Already reviewed and approved (LGTM):
    maw exec default -- seal reviews mark-merged <review-id> --agent {agent}
    Run MERGE PROTOCOL above for $WS
    maw exec default -- bn done <id> --reason="Completed"
    bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done

  Not yet reviewed — RISK:LOW (evals, docs, tests, config):
    Self-review and merge directly.
    maw exec default -- bn bone comment add <id> "Self-review (risk:low): <what you verified>"
    Run MERGE PROTOCOL above for $WS
    maw exec default -- bn done <id> --reason="Completed"
    bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done

  Not yet reviewed — RISK:MEDIUM/HIGH/CRITICAL (REVIEW is true):
    CHECK for existing review: maw exec default -- bn comments <id> | grep "Review created:"
    Create review (if none): maw exec $WS -- seal reviews create --agent {agent} --title "<id>: <title>" --description "<summary>" --reviewers {project}-security
    Record: maw exec default -- bn bone comment add <id> "Review created: <review-id> in workspace <ws-name>"
    Announce: bus send --agent {agent} {project} "Review requested: <review-id> for <id> @{project}-security" -L review-request
    STOP — wait for reviewer. For risk:high add failure-mode checklist, for risk:critical add human approval request.

  If REVIEW is false (regardless of risk):
    Run MERGE PROTOCOL above for $WS
    maw exec default -- bn done <id>
    bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done

After finishing all ready work:
  bus claims release --agent {agent} --all

## 7.5. END-OF-ITERATION CLEANUP

Run cleanup to release orphaned resources before signaling completion:
  edict protocol cleanup --agent {agent}
If it fails (exit 1 = command unavailable), skip — the startup cleanup (step 2.5) will catch it next iteration.

## 8. RELEASE CHECK (before signaling COMPLETE)

Before outputting COMPLETE, check if a release is needed:

0. ACQUIRE RELEASE MUTEX (prevents multiple leads releasing simultaneously):
   bus claims stake --agent {agent} "release://{project}" --ttl 120 -m "checking release"
   If the claim fails (another lead is already releasing): skip the release check entirely.
   The other lead will handle it. Proceed directly to the output signal.

1. Check for unreleased commits: maw exec default -- git log --oneline $(git describe --tags --abbrev=0 2>/dev/null || echo HEAD~20)..HEAD
2. If any commits start with "feat:" or "fix:" (user-visible changes), a release is needed:
   - Bump version in Cargo.toml/package.json (semantic versioning)
   - Update changelog if one exists
   - Release: maw release vX.Y.Z (this tags, pushes, and updates bookmarks)
   - Announce: bus send --agent {agent} {project} "<project> vX.Y.Z released - <summary>" -L release
3. If only "chore:", "docs:", "refactor:" commits, no release needed.
4. RELEASE MUTEX: bus claims release --agent {agent} "release://{project}"

Output: <promise>END_OF_STORY</promise> if more bones remain, else <promise>COMPLETE</promise>

Key rules:
- Triage first, then decide: sequential vs parallel
- Monitor dispatched workers, merge when ready
- All bus/seal commands use --agent {agent}
- All bn commands: maw exec default -- bn ...
- All seal/git commands in a workspace: maw exec $WS -- seal/git ...
- For parallel dispatch, note limitations of this prompt-based approach
- RISK LABELS: Always assess risk during grooming. risk:low (evals, docs, tests, config) skips security review entirely — self-review and merge directly. risk:medium gets standard review (when REVIEW is true). risk:high requires failure-mode checklist. risk:critical requires human approval.{mission_rules}{multi_lead_rules}
- Output completion signal at end"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> LoopContext {
        LoopContext {
            agent: "test-dev".to_string(),
            project: "testproject".to_string(),
            model: "opus".to_string(),
            worker_model: "fast".to_string(),
            worker_timeout: 900,
            review_enabled: true,
            push_main: false,
            check_command: Some("cargo clippy && cargo test".to_string()),
            missions_enabled: true,
            missions_config: None,
            multi_lead_enabled: false,
            multi_lead_config: None,
            project_dir: "/home/test/project".to_string(),
            spawn_env: Default::default(),
            worker_memory_limit: None,
        }
    }

    #[test]
    fn prompt_contains_all_protocol_commands() {
        let ctx = test_ctx();
        let prompt = build(&ctx, None, &[], None);

        // All 5 protocol commands must be referenced in the dev-loop prompt
        assert!(
            prompt.contains("edict protocol resume"),
            "dev-loop prompt must reference 'edict protocol resume'"
        );
        assert!(
            prompt.contains("edict protocol start"),
            "dev-loop prompt must reference 'edict protocol start'"
        );
        assert!(
            prompt.contains("edict protocol review"),
            "dev-loop prompt must reference 'edict protocol review'"
        );
        assert!(
            prompt.contains("edict protocol finish"),
            "dev-loop prompt must reference 'edict protocol finish'"
        );
        assert!(
            prompt.contains("edict protocol cleanup"),
            "dev-loop prompt must reference 'edict protocol cleanup'"
        );
    }

    #[test]
    fn prompt_contains_protocol_fallback_wording() {
        let ctx = test_ctx();
        let prompt = build(&ctx, None, &[], None);

        // Verify fallback wording is present for protocol transitions
        // This prevents silent regressions where protocol fallback guidance is removed
        assert!(
            prompt.contains("If it fails (exit 1 = command unavailable), fall back"),
            "dev-loop prompt must contain protocol fallback wording for unavailable commands"
        );

        // Verify transition hooks are explicitly documented
        assert!(
            prompt.contains("Try protocol command:"),
            "dev-loop prompt must guide agents to try protocol commands first"
        );

        // Verify at least one complete fallback example path is present
        // (e.g., for resume, cleanup, start, review, finish transitions)
        let fallback_patterns = [
            ("protocol resume", "resume check"),
            ("protocol cleanup", "cleanup"),
            ("protocol start", "start"),
            ("protocol review", "review request"),
            ("protocol finish", "finish"),
        ];

        for (protocol_cmd, step_name) in fallback_patterns.iter() {
            assert!(
                prompt.contains(protocol_cmd),
                "dev-loop prompt must reference '{}' in {} step",
                protocol_cmd,
                step_name
            );
        }
    }
}
