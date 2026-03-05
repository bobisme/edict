# Edict

Edict is a setup and sync tool for multi-agent workflows. It bootstraps projects with workflow docs, scripts, and hooks that enable multiple AI coding agents to collaborate on the same codebase — triaging work, claiming tasks, reviewing each other's code, and communicating via channels.

Edict is NOT a runtime. It copies files and regenerates config; the actual coordination happens through the companion tools below.

## Ecosystem

Edict orchestrates these companion projects (all ours):

| Project | Binary | Purpose |
|---------|--------|---------|
| **botbus** | `bus` | Channel-based messaging, claims (advisory locks), agent coordination |
| **maw** | `maw` | Multi-agent workspaces — isolated Git worktrees for concurrent edits |
| **botcrit** | `crit` | Distributed code review — threads, votes, LGTM/block workflow |
| **botty** | `botty` | PTY-based agent runtime — spawn, manage, and communicate with agents |

External (not ours, but used heavily):
- **bones** (`bn`) — Unified issue tracker (replaces beads, beads-view, beads-tui)

## How the Whole System Works End-to-End

Understanding the full chain from "message arrives" to "agent does work" is critical for debugging and development.

### The Agent Spawn Chain

```
1. Message lands on a botbus channel (e.g., `bus send myproject "New task" -L task-request`)
2. botbus checks registered hooks (`bus hooks list`) for matching conditions
3. Matching hook fires → runs its command (typically `botty spawn ...`)
4. botty spawn creates a PTY session, runs `edict run <subcommand>`
5. The agent loop iterates: triage → start → work → review → finish
6. Agent communicates back via `bus send`, updates bones via `bn`, manages workspace via `maw`
```

### Hook Types That Trigger Agents

Registered during `edict init` (and updated via migrations):

| Hook Type | Trigger | Spawns | Example |
|-----------|---------|--------|---------|
| **Router** (claim-based) | Any message on project channel, when no agent claimed | `edict run responder` | `bus hooks add --channel myproject --claim "agent://myproject-dev" ...` |
| **Reviewer** (mention-based) | `@myproject-security` mention | Reviewer agent | `bus hooks add --channel myproject --mention "myproject-security" ...` |

The router hook spawns `edict run responder` which routes messages based on `!` prefixes:
- `!dev [msg]` — create bone + spawn dev-loop
- `!bead [desc]` — create bone (with dedup via `bn search`)
- `!q [question]` — answer with sonnet
- `!qq [question]` — answer with haiku
- `!bigq [question]` — answer with opus
- `!q(model) [question]` — answer with explicit model
- No prefix — smart triage via haiku (chat → reply, question → conversation mode, work → bone + dev-loop)

Also accepts old-style `q:` / `qq:` / `big q:` / `q(model):` prefixes for backwards compatibility.

Hook commands use `botty spawn` with `--env-inherit` to forward environment variables (BOTBUS_CHANNEL, BOTBUS_MESSAGE_ID, BOTBUS_AGENT) to the spawned agent.

### Observing Agents in Action

```bash
botty list                    # See running agents
botty tail <name>             # Stream real-time agent output (primary debugging tool)
botty tail <name> --last 100  # See last 100 lines
botty kill <name>             # Stop a misbehaving agent
botty send <name> "message"   # Send input to agent's PTY

bus history <channel> -n 20   # See recent channel messages
bus statuses list             # See agent presence/status
bus claims list               # See all active claims
bus inbox --all               # See unread messages across all channels
```

`botty tail` is the primary way to see what an agent is doing, whether it's stuck, and what tools it's calling. This is how you evaluate the effectiveness of the entire tool suite.

## Companion Tools Deep Dive

### botbus (`bus`) — Messaging and Coordination

SQLite-backed channel messaging system. Default output is `text` format (concise, token-efficient). Use `--format json` when you need structured data for parsing.

**Core commands:**
- `bus send [--agent $AGENT] <channel> "message" [-L label]` — Post message to channel. Labels categorize messages (task-request, review-request, task-done, feedback, etc.)
- `bus inbox [--channels <ch>] [--mentions] [--mark-read]` — Check unread messages. `--mentions` checks all channels for @agent mentions. `--count-only` for just the count.
- `bus history <channel> [-n count] [--from agent] [--since time]` — Browse message history. Channel can also be passed as `-c/--channel <ch>`. `bus history projects` shows the project registry.
- `bus search <query> [-c channel]` — Full-text search (FTS5 syntax)
- `bus wait [-c channel] [--mention] [-L label] [-t timeout]` — Block until matching message arrives. Used by the responder for follow-up conversations.
- `bus watch [-c channel] [--all]` — Stream messages in real-time

**Claims (advisory locks):**
- `bus claims stake --agent $AGENT "<uri>" [-m memo] [--ttl duration]` — Claim a resource
- `bus claims release --agent $AGENT [--all | "<uri>"]` — Release claims
- `bus claims list [--mine] [--agent $AGENT]` — List active claims
- Claim URI patterns: `bone://project/id`, `workspace://project/ws`, `agent://name`, `respond://name`

**Hooks (event triggers):**
- `bus hooks add --channel <ch> --cwd <dir> [--claim uri] [--mention name] [--ttl secs] <command>` — Register hook. `--cwd` is mandatory.
- `bus hooks list` — List registered hooks with their conditions
- `bus hooks remove <id>` — Remove a hook
- Hook matching: `--claim` fires when claim is available; `--mention` fires on @name in message

**Other:**
- `bus statuses set/clear/list` — Agent presence and status messages
- `bus generate-name` — Generate random agent names (used by dev-loop for worker dispatch)
- `bus whoami [--agent $AGENT]` — Show/verify agent identity

### maw — Multi-Agent Workspaces

Creates isolated Git worktrees so multiple agents can edit files concurrently without conflicts.

**Core commands:**
- `maw ws create <name> [--random]` — Create workspace. Returns workspace name. Workspace files live at `ws/<name>/`. `--random` generates a random name.
- `maw ws list [--format json]` — List all workspaces with their status
- `maw ws merge <name> --destroy` — Squash-merge workspace into main and delete it. `--destroy` is required. **Never use on `default`.**
- `maw ws destroy <name>` — Delete workspace without merging. **Never use on `default`.**
- `maw exec <name> -- <command>` — Run any command inside a workspace (e.g., `maw exec myws -- cargo test`)
- `maw ws status` — Comprehensive view of all workspaces, conflicts, and unmerged work
- `maw init` — Initialize maw in a project
- `maw push` — Push changes to remote
- `maw doctor` — Validate maw configuration

**Critical rules:**
- **Never merge or destroy the default workspace.** It is the main working copy — other workspaces merge INTO it.
- Use `maw exec <ws> -- <command>` to run commands in workspace context (bn, crit, cargo, etc.)
- Use `maw exec default -- bn ...` for bones commands (always in default workspace)
- Use `maw exec <ws> -- crit ...` for review commands (always in the review's workspace)
- Workspace files are at `ws/<name>/` — use absolute paths for file operations
- Never `cd` into a workspace directory and stay there — it breaks cleanup when the workspace is destroyed
- Do not create git branches manually — `maw ws create` handles branching for you.

### botcrit (`crit`) — Code Review

Distributed code review system. Reviews are tied to workspace diffs, with file-line-based comment threads and LGTM/BLOCK voting.

**Review lifecycle:**
```bash
maw exec $WS -- crit reviews create --agent $AGENT --title "..." --reviewers <name>  # Create review + assign reviewer
maw exec $WS -- crit reviews request <id> --reviewers <name> --agent $AGENT  # Re-assign reviewer (after fixes)
maw exec $WS -- crit review <id> [--format json] [--since time]  # Show full review with threads
maw exec $WS -- crit comment --file <path> --line <n> <review-id> "msg"  # Add line comment
maw exec $WS -- crit reply <thread-id> "message"                 # Reply to existing thread
maw exec $WS -- crit lgtm <review-id> [-m "message"]             # Approve
maw exec $WS -- crit block <review-id> --reason "..."            # Block (request changes)
maw exec default -- crit reviews mark-merged <review-id>          # Mark as merged after workspace merge
maw exec $WS -- crit inbox --agent $AGENT                        # Show reviews/threads needing attention
```

**Key details:**
- Always run crit commands via `maw exec <ws> --` in the workspace context
- Reviewers iterate workspaces via `maw ws list` + `maw exec $WS -- crit inbox` per workspace
- Agent identity via `--agent` flag or `CRIT_AGENT`/`BOTBUS_AGENT` env vars
- `--user` flag switches to human identity ($USER) for manual reviews

### botty — Agent Runtime

PTY-based agent spawner and manager. Runs Claude Code sessions in managed PTY processes.

**Core commands:**
- `botty spawn [--pass-env] [--model model] [--timeout secs] <name> <command...>` — Spawn agent. `--pass-env` forwards BOTBUS_* env vars to the spawned process.
- `botty list [--format json]` — List running agents with PIDs and uptime
- `botty tail <name> [--last n] [--follow]` — Stream agent output. **Primary debugging tool.**
- `botty kill <name>` — Terminate agent
- `botty send <name> "message"` — Send text to agent's PTY stdin

### bones (`bn`) — Issue Tracking

Unified issue tracker. Bones are stored in `.bones/`. Event-sourced, no sync needed.

**Core commands:**
- `bn create --title "..." [--description "..."] [--kind task|bug|goal]`
- `bn next` — Next bone to work on (replaces `br ready` and `bv --robot-next`)
- `bn show <id>` — Full bone details with comments and dependencies
- `bn do <id>` — Start work on a bone (sets state to doing)
- `bn done <id> [--reason "..."]` — Close a bone (sets state to done)
- `bn bone comment add <id> "message"` — Add comment
- `bn triage dep add <blocker> --blocks <blocked>` — Add dependency
- `bn triage graph` — Show dependency graph
- `bn bone tag <id> <tag>` — Add tag
- `bn triage` — Triage output with scores and recommendations
- `bn search <query>` — Full-text search

Identity resolved from `$AGENT`/`$BOTBUS_AGENT` env. No `--actor`/`--author` flags needed.

## Agent Subcommands

All agent loops are built into the `edict` binary as subcommands under `edict run`. They are invoked by botbus hooks via `botty spawn`.

### `edict run dev-loop` — Lead Dev Agent

Triages work, dispatches parallel workers, monitors progress, merges completed work.

**Config:** `.edict.toml` → `agents.dev.{model, timeout, maxLoops, pause}`

**Per iteration:**
1. Read inbox, create bones from task requests
2. Check next bones and in-progress work
3. For N >= 2 ready bones: dispatch Haiku workers in parallel via `botty spawn`
4. For single bone or when solo: work directly
5. Monitor worker progress, merge completed workspaces
6. Check for releases (feat/fix commits → version bump + tag)

**Dispatch pattern:** Creates workspace per worker, generates random worker name via `bus generate-name`, stakes claims, comments bone with worker/workspace info, spawns via `botty spawn`.

### `edict run worker-loop` — Worker Agent

Sequential: one bone per iteration. Triage → start → work → review → finish.

**Config:** `.edict.toml` → `agents.worker.{model, timeout}`

**Per iteration:**
1. Resume check (crash recovery via bone comments)
2. Triage: inbox → create bones → `bn next` → pick one
3. Start: claim bone, create workspace, announce
4. Work: implement in workspace using absolute paths
5. Stuck check: 2 failed attempts = post and move on
6. Review: `crit reviews create`, request reviewer, STOP and wait
7. Finish: close bone, merge workspace (`maw ws merge --destroy`), release claims
8. Release check: unreleased feat/fix → bump version

### `edict run reviewer-loop` — Reviewer Agent

Processes reviews, votes LGTM or BLOCK, leaves severity-tagged comments.

**Config:** `.edict.toml` → `agents.reviewer.{model, timeout, maxLoops, pause}`

**Role detection:** Agent name suffix determines role (e.g., `myproject-security` → loads `reviewer-security.md` prompt). Falls back to generic `reviewer.md`.

**Per iteration:**
1. Iterate workspaces via `maw ws list`, check `maw exec $WS -- crit inbox` per workspace
2. Read review diff and source files from workspace (`ws/$WS/...`)
3. Comment with severity: CRITICAL, HIGH, MEDIUM, LOW, INFO
4. Vote: BLOCK if CRITICAL/HIGH issues, LGTM otherwise
5. Post summary to project channel

**Journal:** Maintains `.agents/edict/review-loop-<role>.txt` with iteration summaries.

### `edict run responder` — Universal Message Router

THE single entrypoint for all project channel messages. Routes based on `!` prefixes, maintains conversation context across turns, and can escalate to dev-loop mid-conversation.

**Commands:** `!dev` → dev-loop, `!bead` → create bone, `!q`/`!qq`/`!bigq`/`!q(model)` → question answering, no prefix → haiku triage (chat/question/work)

**Flow:** Fetch message → route by prefix → dispatch to handler. Question mode enters a conversation loop with transcript buffer. Triage classifies bare messages and routes accordingly. Mid-conversation escalation creates a bone with conversation context and spawns dev-loop.

**Config:** `.edict.toml` → `agents.responder.{model, timeout, wait_timeout, max_conversations}`

### `edict run triage` — Token-Efficient Triage

Wraps `bn triage` output into scannable output: top picks, blockers, quick wins, health metrics.

### `edict run iteration-start` — Combined Status

Aggregates inbox, ready bones, pending reviews, active claims into a single status snapshot at iteration start.

## Subcommand Eligibility

Subcommands require specific companion tools to be enabled in `.edict.toml`:

| Subcommand | Requires |
|------------|----------|
| `worker-loop`, `dev-loop` | bones + maw + crit + botbus |
| `reviewer-loop` | crit + botbus |
| `responder` | botbus |
| `triage` | bones |
| `iteration-start` | bones + crit + botbus |

## Claude Code Hooks

Hooks are registered in `.claude/settings.json` as `edict hooks run <name>` commands:

| Hook | Event | Requires | Purpose |
|------|-------|----------|---------|
| `init-agent` | SessionStart | botbus | Display agent identity and project channel |
| `check-jj` | SessionStart | maw | Display workspace tips and maw usage reminders |
| `check-bus-inbox` | PostToolUse | botbus | Check for unread bus messages, inject reminder with previews |
| `claim-agent` | SessionStart, PostToolUse, SessionEnd | botbus | Stake/refresh/release agent claim for session duration |

## How edict sync Works

`edict sync` keeps projects up to date with latest docs, scripts, conventions, and hooks. It manages:

- **Workflow docs** (`.agents/edict/*.md`) — copied from bundled source
- **AGENTS.md managed section** — regenerated from templates (between `<!-- edict:managed-start/end -->` markers)
- **Claude Code hooks** — registered in `.claude/settings.json` as `edict hooks run` commands
- **Prompts** (`.agents/edict/prompts/*.md`) — reviewer prompt templates
- **Design docs** (`.agents/edict/design/*.md`) — copied based on project type
- **Config migrations** (`.edict.toml`) — runs pending migrations

Each component is version-tracked via SHA-256 content hashes stored in marker files (`.version`, `.hooks-version`, `.prompts-version`, `.design-docs-version`). Sync detects staleness by comparing installed hash vs current bundled hash.

### Migrations

**Botbus hooks** (registered via `bus hooks add`) and other runtime changes are managed through **migrations**, not direct sync logic.

Migrations are defined in `src/commands/sync.rs`. Each has an ID (semantic version), title, and migration function.

Migrations run automatically during `edict sync` when the config version is behind. **When adding new botbus hook types or changing runtime behavior, add a migration.**

### Init vs Sync

**`edict init`** does everything: interactive config, creates `.agents/edict/`, copies all files, generates AGENTS.md + `.edict.toml`, initializes external tools (`bn init`, `maw init`, `crit init`), registers botbus hooks, seeds initial bones, creates .gitignore.

**`edict sync`** is incremental: checks staleness, runs pending migrations, updates only changed components, preserves user edits outside managed markers. `--check` mode exits non-zero without changing anything (CI use).

## .edict.toml Config

```json
{
  "version": "1.0.6",
  "project": {
    "name": "myproject",
    "type": ["cli"],
    "defaultAgent": "myproject-dev",
    "channel": "myproject",
    "installCommand": "just install"
  },
  "tools": { "bones": true, "maw": true, "crit": true, "botbus": true, "botty": true },
  "review": { "enabled": true, "reviewers": ["security"] },
  "pushMain": false,
  "agents": {
    "dev": { "model": "opus", "maxLoops": 20, "pause": 2, "timeout": 900,
      "missions": { "enabled": true, "maxWorkers": 4, "maxChildren": 12, "checkpointIntervalSec": 30 }
    },
    "worker": { "model": "haiku", "timeout": 600 },
    "reviewer": { "model": "opus", "maxLoops": 20, "pause": 2, "timeout": 600 },
    "responder": { "model": "sonnet", "timeout": 300, "wait_timeout": 300, "max_conversations": 10 }
  }
}
```

Mission config is read from `agents.dev.missions`. `enabled` defaults to true. `maxWorkers` limits concurrent worker agents per mission, `maxChildren` caps the number of child bones, and `checkpointIntervalSec` controls how often the dev-loop persists mission state.

Scripts read `project.defaultAgent` and `project.channel` on startup, making CLI args optional.

## Edict Release Process

Changes to workflow docs, templates, or commands require a release:

1. **Make changes** in `src/`
2. **Add migration** if behavior changes (see `src/commands/sync.rs`)
3. **Run tests**: `cargo test`
4. **Commit and push** to main
5. **Tag and push**: `maw release vX.Y.Z`
6. **Install locally**: `maw exec default -- just install`

Use semantic versioning and conventional commits.

## Repository Structure

```
src/
  ├── commands/        init, sync, doctor, status, run_agent, dev_loop/, worker_loop, etc.
  ├── hooks/           Claude Code hook management (registry, runner)
  ├── templates/       Embedded templates (docs, prompts, design docs, agents-managed.md)
  ├── config.rs        .edict.toml config parsing
  ├── template.rs      Minijinja template engine
  ├── error.rs         Error types
  ├── lib.rs           Library root
  └── main.rs          CLI entrypoint (clap)
tests/                 Integration tests
evals/                 Behavioral eval framework: rubrics, scripts, results
notes/                 Extended docs (eval-framework.md, migration-system.md, workflow-docs-maintenance.md)
docs/                  Architecture docs
.bones/                Issue tracker (bones)
```

## Development

Runtime: **Rust** (stable). Tooling: **clippy** (lint), **rustfmt** (format), **cargo check** (type check).

```bash
maw exec default -- just install    # cargo install --path .
maw exec default -- just lint       # cargo clippy
maw exec default -- just fmt        # cargo fmt
maw exec default -- just check      # cargo check
maw exec default -- just test       # cargo test
```

## Testing

**Automated tests**: Run `cargo test` — these use isolated environments automatically.

**Manual testing**: ALWAYS use isolated data directories to avoid polluting actual project data:

```bash
BOTBUS_DATA_DIR=/tmp/test-botbus edict init --name test --type cli --tools bones,maw,crit,botbus --no-interactive
BOTBUS_DATA_DIR=/tmp/test-botbus bus hooks list
rm -rf /tmp/test-botbus
```

**Applies to**: Any manual testing with bus, botty, crit, maw, or bn commands during development.

## Conventions

- **Version control: Git + maw.** Create workspaces with `maw ws create`, commit with `git add` + `git commit` inside the workspace, merge with `maw ws merge --destroy`. Do not create branches manually.
- Rust stable edition 2024
- Error handling via `anyhow::Result` with `thiserror` for custom error types
- CLI parsing via `clap` derive macros
- Templates embedded at compile time via `include_str!` and rendered with `minijinja`
- Tests in `tests/` (integration) — run with `cargo test`
- Strict linting (`cargo clippy -- -D warnings`)
- All commits include the trailer `Co-Authored-By: Claude <noreply@anthropic.com>` when Claude contributes

## Debugging and Troubleshooting

### "Look at the botty session for X"

When asked to look at a botty session, immediately run `botty tail <name> --last 200` to see recent output from that agent. This is the primary workflow for:
- Checking if an agent is stuck or making progress
- Identifying tool failures or protocol violations
- Finding improvement opportunities in the tool suite
- Understanding what the agent tried and where it went wrong

Drop whatever you're doing and run the tail command. Analyze the output and report what the agent is doing, whether it's stuck, and what might need fixing.

### Agent not spawning
1. Check hook registration: `bus hooks list` — is the router hook there? Does the channel match? It should point to `edict run responder`.
2. Check claim availability: `bus claims list` — is the `agent://X-dev` claim already taken? (router hook won't fire if claimed)
3. Check botty: `botty list` — is the agent already running?
4. Verify hook command: the hook should run `botty spawn` with correct script path and `--env-inherit`

### Agent stuck or looping
1. `botty tail <name>` — what is the agent doing right now?
2. Check claims: `bus claims list --mine --agent <name>` — stuck claim?
3. Check bone state: `bn show <id>` — is the bone in expected state?
4. Check workspace: `maw ws list` — is workspace still alive?

### Review not being picked up
1. `maw exec $WS -- crit inbox --agent <reviewer>` — does it show the review? (check each workspace)
2. Verify the @mention: the bus message MUST contain `@<project>-<role>` (no @ prefix in hook registration, but @ in message)
3. Check hook: `bus hooks list` — is there a mention hook for that reviewer?
4. Verify reviewer workspace path: reviewer reads code from workspace, not project root

### Common pitfalls from evals
- **Workspace path**: Workspace files are at `ws/$WS/`. Use absolute paths for file operations. Never `cd` into workspace.
- **Re-review**: Reviewers must read from workspace path (`ws/$WS/`) to see fixed code, not main
- **Duplicate bones**: Check existing bones before creating from inbox messages
- **bn via maw exec**: Always use `maw exec default -- bn ...` — never run `bn` directly
- **crit via maw exec**: Always use `maw exec $WS -- crit ...` — crit runs in workspace context
- **Mention format**: `--mention "agent-name"` in hook registration (no @), but `@agent-name` in bus messages

## Eval Framework

Behavioral eval framework for testing agent protocol compliance. See [notes/eval-framework.md](notes/eval-framework.md) for run history, results, and instructions.

Eval types: L2 (single session), Agent Loop, R1 (reviewer bugs), R2 (author response), R3 (full review loop), R4 (integration), R5 (cross-project), R6 (parallel dispatch), R7 (planning), R8 (adversarial review), R9 (crash recovery).

Eval scripts in `evals/scripts/` use `BOTBUS_DATA_DIR` for isolation. Rubrics in `evals/rubrics.md`.

## Proposals

For significant features or changes, use the formal proposal process before implementation.

**Lifecycle**: PROPOSAL → VALIDATING → ACCEPTED/REJECTED

1. Create a bone with `proposal` tag and draft doc in `./notes/proposals/<slug>.md`
2. Validate by investigating open questions, moving answers to "Answered Questions"
3. Accept (remove tag, create implementation bones) or Reject (document why)

See [proposal.md](.agents/edict/proposal.md) for full workflow.

## Output Formats

All companion tools support output formats via `--format`:
- **text** (default for agents/pipes) — Concise, structured plain text. ID-first records, two-space delimiters, no prose. Token-efficient and parseable by convention.
- **pretty** (default for TTY) — Tables, color, box-drawing. For humans at a terminal. Never fed to LLMs or parsed.
- **json** (machines) — Structured output. Always an object envelope (never bare arrays) with an `advice` array for warnings/suggestions.

Format auto-detection: `--format` flag > `FORMAT` env > TTY→pretty / non-TTY→text. Agents always get `text` unless they explicitly request `--format json`.

## Message Labels

Labels on bus messages categorize intent: `task-request`, `task-claim`, `task-blocked`, `task-done`, `review-request`, `review-done`, `review-response`, `feedback`, `grooming`, `tool-issue`, `agent-idle`, `spawn-ack`, `agent-error`.

<!-- edict:managed-start -->
## Edict Workflow

### How to Make Changes

1. **Create a bone** to track your work: `maw exec default -- bn create --title "..." --description "..."`
2. **Create a workspace** for your changes: `maw ws create --random` — this gives you `ws/<name>/`
3. **Edit files in your workspace** (`ws/<name>/`), never in `ws/default/`
4. **Merge when done**: `maw ws merge <name> --destroy --message "feat: <bone-title>"` (use conventional commit prefix: `feat:`, `fix:`, `chore:`, etc.)
5. **Close the bone**: `maw exec default -- bn done <id>`

Do not create git branches manually — `maw ws create` handles branching for you. See [worker-loop.md](.agents/edict/worker-loop.md) for the full triage → start → work → finish cycle.

**All tools have `--help`** with usage examples. When unsure, run `<tool> --help` or `<tool> <command> --help`.

### Directory Structure (maw v2)

This project uses a **bare repo** layout. Source files live in workspaces under `ws/`, not at the project root.

```
project-root/          ← bare repo (no source files here)
├── ws/
│   ├── default/       ← main working copy (AGENTS.md, .bones/, src/, etc.)
│   ├── frost-castle/  ← agent workspace (isolated Git worktree)
│   └── amber-reef/    ← another agent workspace
├── .manifold/         ← maw metadata/artifacts
├── .git/              ← git data (core.bare=true)
└── AGENTS.md          ← stub redirecting to ws/default/AGENTS.md
```

**Key rules:**
- `ws/default/` is the main workspace — bones, config, and project files live here
- **Never merge or destroy the default workspace.** It is where other branches merge INTO, not something you merge.
- Agent workspaces (`ws/<name>/`) are isolated Git worktrees managed by maw
- Use `maw exec <ws> -- <command>` to run commands in a workspace context
- Use `maw exec default -- bn ...` for bones commands (always in default workspace)
- Use `maw exec <ws> -- crit ...` for review commands (always in the review's workspace)
- Never run `bn` or `crit` directly — always go through `maw exec`
- Do not run `jj`; this workflow is Git + maw.

### Bones Quick Reference

| Operation | Command |
|-----------|---------|
| Triage (scores) | `maw exec default -- bn triage` |
| Next bone | `maw exec default -- bn next` |
| Next N bones | `maw exec default -- bn next N` (e.g., `bn next 4` for dispatch) |
| Show bone | `maw exec default -- bn show <id>` |
| Create | `maw exec default -- bn create --title "..." --description "..."` |
| Start work | `maw exec default -- bn do <id>` |
| Add comment | `maw exec default -- bn bone comment add <id> "message"` |
| Close | `maw exec default -- bn done <id>` |
| Add dependency | `maw exec default -- bn triage dep add <blocker> --blocks <blocked>` |
| Search | `maw exec default -- bn search <query>` |

Identity resolved from `$AGENT` env. No flags needed in agent loops.

### Workspace Quick Reference

| Operation | Command |
|-----------|---------|
| Create workspace | `maw ws create <name>` |
| List workspaces | `maw ws list` |
| Check merge readiness | `maw ws merge <name> --check` |
| Merge to main | `maw ws merge <name> --destroy --message "feat: <bone-title>"` |
| Destroy (no merge) | `maw ws destroy <name>` |
| Run command in workspace | `maw exec <name> -- <command>` |
| Diff workspace vs epoch | `maw ws diff <name>` |
| Check workspace overlap | `maw ws overlap <name1> <name2>` |
| View workspace history | `maw ws history <name>` |
| Sync stale workspace | `maw ws sync <name>` |
| Inspect merge conflicts | `maw ws conflicts <name>` |
| Undo local workspace changes | `maw ws undo <name>` |

**Inspecting a workspace (use git, not jj):**
```bash
maw exec <name> -- git status             # what changed (unstaged)
maw exec <name> -- git log --oneline -5   # recent commits
maw ws diff <name>                        # diff vs epoch (maw-native)
```

**Lead agent merge workflow** — after a worker finishes a bone:
1. `maw ws list` — look for `active (+N to merge)` entries
2. `maw ws merge <name> --check` — verify no conflicts
3. `maw ws merge <name> --destroy --message "feat: <bone-title>"` — merge and clean up (use conventional commit prefix)

**Workspace safety:**
- Never merge or destroy `default`.
- Always `maw ws merge <name> --check` before `--destroy`.
- Commit workspace changes with `maw exec <name> -- git add -A && maw exec <name> -- git commit -m "..."`.

### Protocol Quick Reference

Use these commands at protocol transitions to check state and get exact guidance. Each command outputs instructions for the next steps.

| Step | Command | Who | Purpose |
|------|---------|-----|---------|
| Resume | `edict protocol resume --agent $AGENT` | Worker | Detect in-progress work from previous session |
| Start | `edict protocol start <bone-id> --agent $AGENT` | Worker | Verify bone is ready, get start commands |
| Review | `edict protocol review <bone-id> --agent $AGENT` | Worker | Verify work is complete, get review commands |
| Finish | `edict protocol finish <bone-id> --agent $AGENT` | Worker | Verify review approved, get close/cleanup commands |
| Merge | `edict protocol merge <workspace> --agent $AGENT` | Lead | Check preconditions, detect conflicts, get merge steps |
| Cleanup | `edict protocol cleanup --agent $AGENT` | Worker | Check for held resources to release |

All commands support JSON output with `--format json` for parsing. If a command is unavailable or fails (exit code 1), fall back to manual steps documented in [start](.agents/edict/start.md), [review-request](.agents/edict/review-request.md), and [finish](.agents/edict/finish.md).

### Bones Conventions

- Create a bone before starting work. Update state: `open` → `doing` → `done`.
- Post progress comments during work for crash recovery.
- **Run checks before requesting review**: `just check` (or your project's build/test command). Fix any failures before proceeding.
- After finishing a bone, follow [finish.md](.agents/edict/finish.md). **Workers: do NOT push** — the lead handles merges and pushes.
- **Install locally** after releasing: `maw exec default -- just install`

### Identity

Your agent name is set by the hook or script that launched you. Use `$AGENT` in commands.
For manual sessions, use `<project>-dev` (e.g., `myapp-dev`).

### Claims

When working on a bone, stake claims to prevent conflicts:

```bash
bus claims stake --agent $AGENT "bone://<project>/<id>" -m "<id>"
bus claims stake --agent $AGENT "workspace://<project>/<ws>" -m "<id>"
bus claims release --agent $AGENT --all  # when done
```

### Reviews

Use `@<project>-<role>` mentions to request reviews:

```bash
maw exec $WS -- crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT
bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request
```

The @mention triggers the auto-spawn hook for the reviewer.

### Bus Communication

Agents communicate via bus channels. You don't need to be expert on everything — ask the right project.

| Operation | Command |
|-----------|---------|
| Send message | `bus send --agent $AGENT <channel> "message" [-L label]` |
| Check inbox | `bus inbox --agent $AGENT --channels <ch> [--mark-read]` |
| Wait for reply | `bus wait -c <channel> --mention -t 120` |
| Browse history | `bus history <channel> -n 20` |
| Search messages | `bus search "query" -c <channel>` |

**Conversations**: After sending a question, use `bus wait -c <channel> --mention -t <seconds>` to block until the other agent replies. This enables back-and-forth conversations across channels.

**Project experts**: Each `<project>-dev` is the expert on their project. When stuck on a companion tool (bus, maw, crit, botty, bn), post a question to its project channel instead of guessing.

### Cross-Project Communication

**Don't suffer in silence.** If a tool confuses you or behaves unexpectedly, post to its project channel.

1. Find the project: `bus history projects -n 50` (the #projects channel has project registry entries)
2. Post question or feedback: `bus send --agent $AGENT <project> "..." -L feedback`
3. For bugs, create bones in their repo first
4. **Always create a local tracking bone** so you check back later:
   ```bash
   maw exec default -- bn create --title "[tracking] <summary>" --tag tracking --kind task
   ```

See [cross-channel.md](.agents/edict/cross-channel.md) for the full workflow.

### Session Search (optional)

Use `cass search "error or problem"` to find how similar issues were solved in past sessions.


### Design Guidelines


- [CLI tool design for humans, agents, and machines](.agents/edict/design/cli-conventions.md)



### Workflow Docs


- [Find work from inbox and bones](.agents/edict/triage.md)

- [Claim bone, create workspace, announce](.agents/edict/start.md)

- [Change bone state (open/doing/done)](.agents/edict/update.md)

- [Close bone, merge workspace, release claims](.agents/edict/finish.md)

- [Full triage-work-finish lifecycle](.agents/edict/worker-loop.md)

- [Turn specs/PRDs into actionable bones](.agents/edict/planning.md)

- [Explore unfamiliar code before planning](.agents/edict/scout.md)

- [Create and validate proposals before implementation](.agents/edict/proposal.md)

- [Request a review](.agents/edict/review-request.md)

- [Handle reviewer feedback (fix/address/defer)](.agents/edict/review-response.md)

- [Reviewer agent loop](.agents/edict/review-loop.md)

- [Merge a worker workspace (protocol merge + conflict recovery)](.agents/edict/merge-check.md)

- [Validate toolchain health](.agents/edict/preflight.md)

- [Ask questions, report bugs, and track responses across projects](.agents/edict/cross-channel.md)

- [Report bugs/features to other projects](.agents/edict/report-issue.md)

- [groom](.agents/edict/groom.md)

<!-- edict:managed-end -->
