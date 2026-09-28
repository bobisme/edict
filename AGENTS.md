# Edict

Edict is a setup and sync tool for multi-agent workflows. It bootstraps projects with workflow docs, scripts, and hooks that enable multiple AI coding agents to collaborate on the same codebase — triaging work, claiming tasks, reviewing each other's code, and communicating via channels.

Edict is NOT a runtime. It copies files and regenerates config; the actual coordination happens through the companion tools below.

## Ecosystem

Edict orchestrates these companion projects (all ours):

| Project | Binary | Purpose |
|---------|--------|---------|
| **rite** | `rite` | Channel-based messaging, claims (advisory locks), agent coordination |
| **maw** | `maw` | Multi-agent workspaces — isolated Git worktrees for concurrent edits |
| **seal** | `seal` | Distributed code review — threads, votes, LGTM/block workflow |
| **vessel** | `vessel` | PTY-based agent runtime — spawn, manage, and communicate with agents |

External (not ours, but used heavily):
- **bones** (`bn`) — Unified issue tracker (replaces beads, beads-view, beads-tui)

## How the Whole System Works End-to-End

Understanding the full chain from "message arrives" to "agent does work" is critical for debugging and development.

### The Agent Spawn Chain

```
1. Message lands on a rite channel (e.g., `rite send myproject "New task" -L task-request`)
2. rite checks registered hooks (`rite hooks list`) for matching conditions
3. Matching hook fires → runs its command (typically `vessel spawn ...`)
4. vessel spawn creates a PTY session, runs `edict run <subcommand>`
5. The agent loop iterates: triage → start → work → review → finish
6. Agent communicates back via `rite send`, updates bones via `bn`, manages workspace via `maw`
```

### Hook Types That Trigger Agents

Registered during `edict init` (and updated via migrations):

| Hook Type | Trigger | Spawns | Example |
|-----------|---------|--------|---------|
| **Router** (claim-based) | Any message on project channel, when no agent claimed | `edict run responder` | `rite hooks add --channel myproject --claim "agent://myproject-dev" ...` |

Security review has no ambient Rite hook. The author starts one named Vessel
Codex session for the exact Seal review using
`.agents/edict/security-review.md`; it uses `gpt-daybreak-blue-latest` and is
watched through Agentbus.

The router hook spawns `edict run responder` which routes messages based on `!` prefixes:
- `!dev [msg]` — create bone + spawn dev-loop
- `!bead [desc]` — create bone (with dedup via `bn search`)
- `!q [question]` — answer with sonnet
- `!qq [question]` — answer with gpt-5.6-luna
- `!bigq [question]` — answer with opus
- `!q(model) [question]` — answer with explicit model
- No prefix — smart triage via gpt-5.6-luna (chat → reply, question → conversation mode, work → bone + dev-loop)

Also accepts old-style `q:` / `qq:` / `big q:` / `q(model):` prefixes for backwards compatibility.

Hook commands use `vessel spawn --env-inherit "RITE_*,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT"` to forward environment to the spawned agent. One namespace covers every var rite sets — `RITE_CHANNEL`, `RITE_MESSAGE_ID`, `RITE_AGENT`, `RITE_HOOK_ID`, and the lease-only `RITE_BATCH_COUNT` / `RITE_BATCH_MESSAGE_IDS` / `RITE_LEASE_PATTERN` — plus `RITE_DATA_DIR`, which keeps an isolated test store isolated across a spawn. The value lives in one place — `reply::hook_env_inherit()` (`src/reply.rs`).

A vessel older than 0.18 treats `RITE_*` as a literal name and inherits nothing, so `hook_env_inherit()` probes `vessel spawn --help` and falls back to the explicit list.

`RITE_AGENT` holds the hook's `claim_owner` when it has one, which for every edict hook is the spawned agent itself, not the sender. The loops still resolve identity from `--agent` and override the env.

### Hook Convergence

`ensure_rite_hook` (`src/subprocess.rs`) converges a hook by its stable name (`rite hooks add --name edict:<project>:<role> --owner edict`), which rewrites the record in place and keeps its ID. Never remove-and-add:

- The ID is the spawn-lease key (`spawn://<hook-id>/<channel>`). A new ID lets a replacement spawn beside a responder that still holds the old lease.
- Fields edict does not pass keep their current values, so a converge cannot strip configuration edict never learned about — a lease above all.
- `last_fired` survives, so a cooldown hook gets no free firing.

Hooks registered before named hooks existed carry no name. Adding a named hook beside one produces a duplicate, so the first converge adopts them with `rite hooks set <id> --name … --owner edict`, which keeps the ID and the lease. Against a rite with no `--name` (probed from `hooks add --help`, since the feature landed after v0.33.0 was cut), the old remove-and-add path still applies.

`RITE_MESSAGE_ID` is also the spawned agent's **reply anchor** — the message it must answer with `rite send --reply-to`. For a lease batch the anchor is the LAST id in `RITE_BATCH_MESSAGE_IDS` (chronological, triggering message last).

### Hook Announcement and the Live-Hook Guard

Before `ensure_rite_hook` mutates anything, it announces what's about to happen (`src/rite_hook_guard.rs`, `announce`): the resolved rite data dir (`RITE_DATA_DIR`, else `$XDG_DATA_HOME/rite`, else `$HOME/.local/share/rite` — mirroring `rite/src/core/project.rs::data_dir()`), printed once per process, followed by each hook's name/channel/cwd/command on stderr.

It then guards (`guard`): if the hook's `--cwd` is under the system temp directory (`std::env::temp_dir()` or `/tmp`) and `RITE_DATA_DIR` is unset, registration is refused with an error naming `RITE_DATA_DIR` and `--dry-run` — a live hook there would otherwise spawn real agents against a throwaway project, using the machine's real rite data directory. Pass `--allow-live-hooks` to `edict init`/`edict sync`/`edict hooks install` to register anyway; setting `RITE_DATA_DIR` also allows it (and is the right choice for tests — see "Testing template changes safely" below). `edict init`/`sync` fail non-zero when the guard refuses on their primary registration path; best-effort hook migrations in `edict sync` log a warning and continue.

### Observing Agents in Action

```bash
vessel list                    # See running agents
vessel tail <name>             # Stream real-time agent output (primary debugging tool)
vessel tail <name> --last 100  # See last 100 lines
vessel kill <name>             # Stop a misbehaving agent
vessel send <name> "message"   # Send input to agent's PTY

rite history <channel> -n 20   # See recent channel messages
rite statuses list             # See agent presence/status
rite claims list               # See all active claims
rite inbox --all               # See unread messages across all channels
```

`vessel tail` is the primary way to see what an agent is doing, whether it's stuck, and what tools it's calling. This is how you evaluate the effectiveness of the entire tool suite.

## Companion Tools Deep Dive

### rite (`rite`) — Messaging and Coordination

SQLite-backed channel messaging system. Default output is `text` format (concise, token-efficient). Use `--format json` when you need structured data for parsing.

**Core commands:**
- `rite send [--agent $AGENT] <channel> "message" [-L label]` — Post message to channel. Labels categorize messages (task-request, review-request, task-done, feedback, etc.)
- `rite inbox [--channels <ch>] [--mentions] [--mark-read]` — Check unread messages. `--mentions` checks all channels for @agent mentions. `--count-only` for just the count.
- `rite history <channel> [-n count] [--from agent] [--since time]` — Browse message history. Channel can also be passed as `-c/--channel <ch>`. `rite history projects` shows the project registry.
- `rite search <query> [-c channel]` — Full-text search (FTS5 syntax)
- `rite wait [-c channel] [--mentions] [--from agent] [-L label] [-t timeout]` — Block until matching message arrives. Used by the responder for follow-up conversations.
- `rite watch [-c channel] [--all]` — Stream messages in real-time

**Threading (rite >= 0.33):**
- `rite send ... --reply-to <ULID>` — Anchor a message under a parent. No flag means top-level. An unknown parent warns and links up when it syncs in.
- `rite send ... --format json` — Prints the new message id (`.id`), which is the anchor others wait on.
- `rite history --thread <ULID>` — Read the whole thread from any message in it; finds the channel itself. `thread.complete:false` means the result is a fragment (`missing_parent`, `self_reference`, `cycle`, `depth_limit`).
- `rite wait --reply-to <ULID> [-t secs] [--allow-missing-parent]` — Block until someone answers that message. Exit 0 = answered, 1 = timeout (escalate, never re-ask), 2 = not a ULID or unknown to this store. `--reply-to` NARROWS: `--from`, `-c`, `-L` only subtract candidates. No race with send, and your own reply never satisfies your wait.

Edict builds the per-turn anchor instruction in `src/reply.rs` and appends it to every loop prompt (responder, dev, worker, reviewer) on every iteration — an agent told its anchor once at spawn drifts within a few turns.

**Claims (advisory locks):**
- `rite claims stake --agent $AGENT "<uri>" [-m memo] [--ttl duration]` — Claim a resource
- `rite claims release --agent $AGENT [--all | "<uri>"]` — Release claims
- `rite claims list [--mine] [--agent $AGENT]` — List active claims
- Claim URI patterns: `bone://project/id`, `workspace://project/ws`, `agent://name`, `respond://name`

**Hooks (event triggers):**
- `rite hooks add --channel <ch> --cwd <dir> [--claim uri] [--mention name] [--ttl secs] <command>` — Register hook. `--cwd` is mandatory.
- `rite hooks list` — List registered hooks with their conditions
- `rite hooks remove <id>` — Remove a hook
- Hook matching: `--claim` fires when claim is available; `--mention` fires on @name in message

**Other:**
- `rite statuses set/clear/list` — Agent presence and status messages
- `rite generate-name` — Generate random agent names (used by dev-loop for worker dispatch)
- `rite whoami [--agent $AGENT]` — Show/verify agent identity

### maw — Multi-Agent Workspaces

Creates isolated Git worktrees so multiple agents can edit files concurrently without conflicts.

**Core commands:**
- `maw ws create <name> --from main --description "..."` — Create a trunk-based workspace. Use the bone ID as the name and bone title as the description. Use `--change <change-id>` instead of `--from main` for change-bound work. Workspace files live at `ws/<name>/`.
- `maw ws list [--format json]` — List all workspaces with their status
- `maw ws merge <name> --into default --destroy` — Merge a workspace into `default` and delete it. `--destroy` is required. Swap `default` for a change id when merging tracked change work. **Never use on `default` as the source workspace.**
- `maw ws destroy <name>` — Delete workspace without merging. **Never use on `default`.**
- `maw exec <name> -- <command>` — Run any command inside a workspace (e.g., `maw exec myws -- cargo test`)
- `maw ws status` — Comprehensive view of all workspaces, conflicts, and unmerged work
- `maw init` — Initialize maw in a project
- `maw push` — Push changes to remote
- `maw doctor` — Validate maw configuration

**Critical rules:**
- **Never merge or destroy the default workspace.** It is the main working copy — other workspaces merge INTO it.
- Use `maw exec <ws> -- <command>` to run commands in workspace context (bn, seal, cargo, etc.)
- Use `maw exec default -- bn ...` for bones commands (always in default workspace)
- Use `maw exec <ws> -- seal ...` for review commands (always in the review's workspace)
- Workspace files are at `ws/<name>/` — use absolute paths for file operations
- Never `cd` into a workspace directory and stay there — it breaks cleanup when the workspace is destroyed
- Do not create git branches manually — `maw ws create` handles branching for you.

### seal (`seal`) — Code Review

Distributed code review system. Reviews are tied to workspace diffs, with file-line-based comment threads and LGTM/BLOCK voting.

**Review lifecycle:**
```bash
maw exec $WS -- seal reviews create --agent $AGENT --title "..." --reviewers <name>  # Create review + assign reviewer
maw exec $WS -- seal reviews retarget <id> --agent $AGENT         # Move target to HEAD, clear votes (after fixes, BEFORE re-request)
maw exec $WS -- seal reviews request <id> --reviewers <name> --agent $AGENT  # Re-assign reviewer (after fixes)
maw exec $WS -- seal review <id> [--format json] [--since time]  # Show full review with threads
maw exec $WS -- seal comment --file <path> --line <n> <review-id> "msg"  # Add line comment
maw exec $WS -- seal reply <thread-id> "message"                 # Reply to existing thread
maw exec $WS -- seal lgtm <review-id> [-m "message"]             # Approve
maw exec $WS -- seal block <review-id> --reason "..."            # Block (request changes)
maw exec $WS -- git status --porcelain --untracked-files=all      # Must list nothing outside .seal/reviews/<review-id>/
maw exec $WS -- seal reviews mark-merged <review-id>              # Mark as merged BEFORE maw ws merge, then
maw exec $WS -- git add .seal/reviews/<review-id>                 #   commit only the review log in $WS so the
maw exec $WS -- git commit -m "chore: seal review <review-id>" -- .seal/reviews/<review-id>  # merge carries it
maw exec $WS -- seal inbox --agent $AGENT                        # Show reviews/threads needing attention
```

**Key details (seal >= 0.28):**
- A review covers a **commit range**, not one commit. `seal reviews create` discovers the
  branch/workspace fork point and prints the resolved range plus commit count. `--base <rev>`
  sets it explicitly; `--base <target>~1` restores tip-only. The base is persisted on the
  `ReviewCreated` event, so later commits extend the range instead of shifting it.
- `seal reviews request` re-assigns reviewers on an **existing** review but does not move its
  target commit. After new commits, run `seal reviews retarget <id>` (seal >= 0.29) first — it
  moves the review to the workspace's current HEAD and clears votes, requiring fresh approval.
  Skipping it leaves the dedicated reviewer verifying a stale, pre-fix diff.
- An approval records the commit it covered. `seal reviews mark-merged` **exits 1** when
  commits landed after the approval. The fix is `seal reviews retarget` followed by a fresh
  LGTM; `--allow-stale-approval` is the deliberate override.
- `seal diff <id> --format json` reports `base_is_persisted`, `approval_stale`,
  `approved_commit` and `uncovered_commits`. `edict protocol merge` reads `approval_stale`
  and agrees with seal, falling back to comparing the target commit against the workspace
  HEAD on older seal (`freshness_from_summary`, `src/commands/protocol/merge.rs`).
- Always run seal commands via `maw exec <ws> --` in the workspace context
- Reviewers iterate workspaces via `maw ws list` + `maw exec $WS -- seal inbox` per workspace
- Agent identity via `--agent` flag or `CRIT_AGENT`/`RITE_AGENT` env vars
- `--user` flag switches to human identity ($USER) for manual reviews

### vessel — Agent Runtime

PTY-based agent spawner and manager. Runs Claude Code sessions in managed PTY processes.

**Core commands:**
- `vessel spawn [--pass-env] [--model model] [--timeout secs] <name> <command...>` — Spawn agent. `--pass-env` forwards RITE_* env vars to the spawned process.
- `vessel list [--format json]` — List running agents with PIDs and uptime
- `vessel tail <name> [--last n] [--follow]` — Stream agent output. **Primary debugging tool.**
- `vessel kill <name>` — Terminate agent
- `vessel send <name> "message"` — Send text to agent's PTY stdin

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

Identity resolved from `$AGENT`/`$RITE_AGENT` env. No `--actor`/`--author` flags needed.

## Agent Subcommands

All agent loops are built into the `edict` binary as subcommands under `edict run`. They are invoked by rite hooks via `vessel spawn`.

### `edict run dev-loop` — Lead Dev Agent

Triages work, dispatches parallel workers, monitors progress, merges completed work.

**Config:** `.edict.toml` → `agents.dev.{model, timeout, maxLoops, pause}`

**Per iteration:**
1. Read inbox, create bones from task requests
2. Check next bones and in-progress work
3. For N >= 2 ready bones: dispatch Haiku workers in parallel via `vessel spawn`
4. For single bone or when solo: work directly
5. Monitor worker progress, merge completed workspaces
6. Check for releases (feat/fix commits → version bump + tag)

**Dispatch pattern:** Creates workspace per worker, generates random worker name via `rite generate-name`, stakes claims, comments bone with worker/workspace info, spawns via `vessel spawn`.

### `edict run worker-loop` — Worker Agent

Sequential: one bone per iteration. Triage → start → work → review → finish.

**Config:** `.edict.toml` → `agents.worker.{model, timeout}`

**Per iteration:**
1. Resume check (crash recovery via bone comments)
2. Triage: inbox → create bones → `bn next` → pick one
3. Start: claim bone, create workspace, announce
4. Work: implement in workspace using absolute paths
5. Stuck check: 2 failed attempts = post and move on
6. Review: create the Seal review, then launch its exact Daybreak reviewer
7. Finish: close bone, merge workspace (`maw ws merge --into default --destroy`), release claims
8. Release check: unreleased feat/fix → bump version

### Dedicated Daybreak Security Reviewer

The author launches one short-lived Vessel Codex session per exact Seal review.
It receives the review id, workspace, target commit, canonical Seal identity,
and Rite reply anchor. It never scans workspaces or discovers other work.
Follow `.agents/edict/security-review.md`; verify the actual Seal vote after
Agentbus reports completion. Timeouts, blocked sessions, and unavailable
Agentbus are blockers, never implicit approval.

### `edict run responder` — Universal Message Router

THE single entrypoint for all project channel messages. Routes based on `!` prefixes, maintains conversation context across turns, and can escalate to dev-loop mid-conversation.

**Commands:** `!dev` → dev-loop, `!bead` → create bone, `!q`/`!qq`/`!bigq`/`!q(model)` → question answering, no prefix → gpt-5.6-luna triage (chat/question/work)

**Flow:** Fetch message → route by prefix → dispatch to handler. Question mode enters a conversation loop with transcript buffer. Triage classifies bare messages and routes accordingly. Mid-conversation escalation creates a bone with conversation context and spawns dev-loop.

**Unaddressed spawns:** the router hook fires on every channel message, so the responder also wakes for traffic between other agents. Before any agent run, it classifies the spawn batch (the trigger plus `RITE_BATCH_MESSAGE_IDS`) with the addressing rules below. When every message is known to be addressed to someone else (it only @mentions other agents, or replies in someone else's thread), the responder logs the skip to stderr, counts `edict.responder.skipped_unaddressed_total`, releases its `agent://` claim and clears its status, and exits with no agent run and no channel post. It does not stake a `message://` claim for a skipped batch. Addressed and unknown batches (for example, a thread fetch that fails) run as before. The check applies only to the spawn batch; follow-ups inside a conversation are answered without it.

**Turn failures:** a failed agent turn is logged and counted (`edict.responder.turn_failures_total`, attribute `addressed`), and posted as "Could not answer that: … Send it again to retry." only when the message was addressed to the responder. Addressed means: a DM with the responder, a `!`/colon command prefix, an @mention of the responder, a reply to a message the responder wrote, or a top-level message that mentions no one. A message that only @mentions other agents, or replies in someone else's thread, gets no failure post. For a hook batch (`RITE_BATCH_MESSAGE_IDS`) any addressed message counts. If the thread cannot be fetched (`rite history --thread <id> --format json`), the failure is posted. The logic lives in `src/commands/responder_addressing.rs`.

**Config:** `.edict.toml` → `agents.responder.{model, timeout, wait_timeout, max_conversations}`

### `edict run triage` — Token-Efficient Triage

Wraps `bn triage` output into scannable output: top picks, blockers, quick wins, health metrics.

### `edict run iteration-start` — Combined Status

Aggregates inbox, ready bones, pending reviews, active claims into a single status snapshot at iteration start.

## Subcommand Eligibility

Subcommands require specific companion tools to be enabled in `.edict.toml`:

| Subcommand | Requires |
|------------|----------|
| `worker-loop`, `dev-loop` | bones + maw + seal + rite |
| `responder` | rite |
| `triage` | bones |
| `iteration-start` | bones + seal + rite |

## Claude Code Hooks

Hooks are registered in `.claude/settings.json` as `edict hooks run <name>` commands:

| Hook | Event | Requires | Purpose |
|------|-------|----------|---------|
| `init-agent` | SessionStart | rite | Display agent identity and project channel |
| `check-jj` | SessionStart | maw | Display workspace tips and maw usage reminders |
| `check-rite-inbox` | PostToolUse | rite | Check for unread rite messages, inject reminder with previews |
| `claim-agent` | SessionStart, PostToolUse, SessionEnd | rite | Stake/refresh/release agent claim for session duration |

## How edict sync Works

`edict sync` keeps projects up to date with latest docs, scripts, conventions, and hooks. It manages:

- **Workflow docs** (`.agents/edict/*.md`) — copied from bundled source
- **AGENTS.md managed section** — regenerated from templates (between `<!-- edict:managed-start/end -->` markers)
- **Claude Code hooks** — registered in `.claude/settings.json` as `edict hooks run` commands
- **Design docs** (`.agents/edict/design/*.md`) — copied based on project type
- **Config migrations** (`.edict.toml`) — runs pending migrations

Each component is version-tracked via SHA-256 content hashes stored in marker files (`.version`, `.hooks-version`, `.design-docs-version`). Sync detects staleness by comparing installed hash vs current bundled hash.

### Migrations

**Botrite hooks** (registered via `rite hooks add`) and other runtime changes are managed through **migrations**, not direct sync logic.

Migrations are defined in `src/commands/sync.rs`. Each has an ID (semantic version), title, and migration function.

Migrations run automatically during `edict sync` when the config version is behind. **When adding new rite hook types or changing runtime behavior, add a migration.**

### Init vs Sync

**`edict init`** does everything: interactive config, creates `.agents/edict/`, copies all files, generates AGENTS.md + `.edict.toml`, initializes external tools (`bn init`, `maw init`, `seal init`), registers rite hooks, seeds initial bones, creates .gitignore. `edict init --dry-run` (like `edict sync --dry-run`) lists every one of those actions — project files, the global agent hooks under `$HOME` (`~/.claude/settings.json`, the Pi extension), `bn`/`maw`/`seal init`, the `#projects` announcement, the rite router hook (name, channel, cwd, command), seed bones, the `.gitignore` fetch (planned, not fetched) and the commit — then exits 0 having changed nothing. It works with `--no-interactive`; in interactive mode the prompts still run, since they only gather choices.

**`edict sync`** is incremental: checks staleness, runs pending migrations, updates only changed components, preserves user edits outside managed markers. `--check` mode exits non-zero without changing anything (CI use). `--dry-run` lists what a sync would change — files created/updated (with a `+N -M lines` summary), migration steps, rite hooks to create/update/adopt/replace (name, channel, cwd, command), `git`/`bn` calls and the commit — then exits 0 without writing anything or running a mutating subprocess. Every sync side effect goes through `crate::effects::Effects` (`src/effects.rs`), which performs it or, under `--dry-run`, records it; new sync and init steps must use it (`fx.write`, `fx.write_generated` for content fetched at apply time, `fx.run`, `fx.run_command`, `fx.announce`, `fx.report` for apply-mode progress lines) rather than `fs::write`/`Tool::run`/`println!` directly. The only `dry_run` check is where each command builds its `Effects`.

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
  "tools": { "bones": true, "maw": true, "seal": true, "rite": true, "vessel": true },
  "review": { "enabled": true, "reviewers": ["security"] },
  "pushMain": false,
  "agents": {
    "dev": { "model": "opus", "maxLoops": 20, "pause": 2, "timeout": 900,
      "missions": { "enabled": true, "maxWorkers": 4, "maxChildren": 12, "checkpointIntervalSec": 30 }
    },
    "worker": { "model": "haiku", "timeout": 600 },
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

### Testing template changes safely

A change to workflow docs, `agents-managed.md.jinja`, or hook-registration logic is not
verified until you see it run through a real `init`/`sync`. Doing that carelessly can register
a live rite hook in the machine's real data dir — see "Hook Announcement and the Live-Hook
Guard" above; each of those hooks spawns a real agent. Verify without touching it.

**1. Preview.** `edict sync --dry-run` / `edict init --dry-run` render the full plan — files,
hooks, migrations, the commit — and change nothing (see "Init vs Sync" above). Start here; it
covers most template changes.

**2. Full run, sandboxed.** When a dry-run is not enough, run `init` and `sync` for real, with
`RITE_DATA_DIR`, `HOME`, the `XDG_*` vars, and `VESSEL_SOCKET` all pointed inside a fresh
tempdir, and the agent-identity env vars unset so nothing spawns or routes as a live agent.
Run it with `bash` (from any shell): the overrides live only in that child process, so your
own shell never ends up with a sandboxed `HOME`.

`VESSEL_SOCKET` is not optional: vessel ignores `XDG_RUNTIME_DIR` for its socket (it hardcodes
`/run/user/$UID/vessel.sock` unless `--socket`/`VESSEL_SOCKET` says otherwise) and, when it
auto-starts a server, that server re-execs itself into the fixed systemd unit
`vessel-server.scope` — the same unit name the machine's real vessel server already owns. Without
`VESSEL_SOCKET`, a sandbox rite hook that fires `vessel spawn` reaches the REAL vessel server
(see bn-61kf); the `systemd-run` shim below forces the re-exec to fail deterministically so
vessel falls back to a bare, private server bound to the sandboxed socket instead of racing the
real unit.

```bash
bash <<'SANDBOX'
set -eu
sandbox=$(mktemp -d)
export RITE_DATA_DIR="$sandbox/rite"
export HOME="$sandbox/home"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_STATE_HOME="$HOME/.local/state"
export XDG_RUNTIME_DIR="$sandbox/run"
export VESSEL_SOCKET="$sandbox/run/vessel.sock"
unset AGENT RITE_AGENT BOTBUS_AGENT

mkdir -p "$sandbox/project" "$sandbox/run" "$sandbox/bin"
# vessel's auto-started server re-execs into the fixed unit `vessel-server.scope`
# via systemd-run; the real server already owns that unit, so make systemd-run
# fail here and force the deterministic bare-server fallback (see above).
cat >"$sandbox/bin/systemd-run" <<'EOF'
#!/bin/sh
echo "sandbox: systemd-run disabled" >&2
exit 1
EOF
chmod 755 "$sandbox/bin/systemd-run"
export PATH="$sandbox/bin:$PATH"

cd "$sandbox/project"
git init -q && git config user.email test@example.com && git config user.name Test
git commit -q --allow-empty -m init

edict init --no-interactive --name sandbox-proj --type cli --tools rite --no-commit
edict sync --no-commit
rite hooks list   # the hook is here, inside the sandbox
echo "sandbox: $sandbox (delete it when done)"
SANDBOX
```

Do not pass `--language` — it triggers a network `.gitignore` fetch, irrelevant here.
`tests/hermetic_rite_hooks.rs` runs this same recipe automatically; read it for the full
end-to-end proof, including the assertion that nothing lands outside `$RITE_DATA_DIR`.

**3. Confirm the live hook set and the live vessel server are untouched.** From a normal shell,
with no `RITE_DATA_DIR`/`VESSEL_SOCKET` override, run this before and after step 2:

```bash
rite hooks list | wc -l
vessel list --format json
```

Both must match before/after: the hook count, and the set of real agents (the sandbox recipe
must never add one). The sandboxed run registers its hook inside `$RITE_DATA_DIR` only, so
the real data dir never sees it.

**4. If you forget the sandbox.** The live-hook guard refuses to register a hook when the
project's `--cwd` is under the system temp directory and `RITE_DATA_DIR` is unset — `init`/
`sync` exit non-zero, naming `RITE_DATA_DIR` and `--allow-live-hooks`. It only catches the
temp-dir case: a throwaway project rooted elsewhere is not protected. Trust the sandbox
recipe above, not the guard, to keep a test run isolated.

**Applies to**: Any manual testing with rite, vessel, seal, maw, or bn commands during
development.

## Conventions

- **Version control: Git + maw.** Create workspaces with `maw ws create <bone-id> --from main --description "<title>"` (or `--change <change-id>`), commit with `git add` + `git commit` inside the workspace, merge with `edict protocol merge <name>` and the steps it prints (a bare `maw ws merge <name> --into default --destroy` only for work with no review). Do not create branches manually.
- Rust stable edition 2024
- Error handling via `anyhow::Result` with `thiserror` for custom error types
- CLI parsing via `clap` derive macros
- Templates embedded at compile time via `include_str!` and rendered with `minijinja`
- Tests in `tests/` (integration) — run with `cargo test`
- Strict linting (`cargo clippy -- -D warnings`)
- All commits include the trailer `Co-Authored-By: Claude <noreply@anthropic.com>` when Claude contributes

## Debugging and Troubleshooting

### "Look at the vessel session for X"

When asked to look at a vessel session, immediately run `vessel tail <name> --last 200` to see recent output from that agent. This is the primary workflow for:
- Checking if an agent is stuck or making progress
- Identifying tool failures or protocol violations
- Finding improvement opportunities in the tool suite
- Understanding what the agent tried and where it went wrong

Drop whatever you're doing and run the tail command. Analyze the output and report what the agent is doing, whether it's stuck, and what might need fixing.

### Agent not spawning
1. Check hook registration: `rite hooks list` — is the router hook there? Does the channel match? It should point to `edict run responder`.
2. Check claim availability: `rite claims list` — is the `agent://X-dev` claim already taken? (router hook won't fire if claimed)
3. Check vessel: `vessel list` — is the agent already running?
4. Verify hook command: the hook should run `vessel spawn` with correct script path and `--env-inherit`

### Agent stuck or looping
1. `vessel tail <name>` — what is the agent doing right now?
2. Check claims: `rite claims list --mine --agent <name>` — stuck claim?
3. Check bone state: `bn show <id>` — is the bone in expected state?
4. Check workspace: `maw ws list` — is workspace still alive?

### Dedicated security review did not complete
1. Confirm the exact target: `maw exec $WS -- seal review <review-id> --format json` and `seal diff`.
2. Inspect the named session: `vessel snapshot <session>` and `vessel list --format json`.
3. Inspect Agentbus with the recorded Vessel PID and send timestamp.
4. If Agentbus or Seal cannot verify a vote, post an anchored blocker, release the review claim, and stop.

### Common pitfalls from evals
- **Workspace path**: Workspace files are at `ws/$WS/`. Use absolute paths for file operations. Never `cd` into workspace.
- **Re-review**: Reviewers must read from workspace path (`ws/$WS/`) to see fixed code, not main
- **Duplicate bones**: Check existing bones before creating from inbox messages
- **bn via maw exec**: Always use `maw exec default -- bn ...` — never run `bn` directly
- **seal via maw exec**: Always use `maw exec $WS -- seal ...` — seal runs in workspace context
- **Exact review target**: pass the Seal review id and workspace to the direct reviewer; never use a mention or scan to select work.

## Eval Framework

Behavioral eval framework for testing agent protocol compliance. See [notes/eval-framework.md](notes/eval-framework.md) for run history, results, and instructions.

Eval types: L2 (single session), Agent Loop, R1 (reviewer bugs), R2 (author response), R3 (full review loop), R4 (integration), R5 (cross-project), R6 (parallel dispatch), R7 (planning), R8 (adversarial review), R9 (crash recovery).

Eval scripts in `evals/scripts/` use `RITE_DATA_DIR` for isolation. Rubrics in `evals/rubrics.md`.

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

Labels on rite messages categorize intent: `task-request`, `task-claim`, `task-blocked`, `task-done`, `review-request`, `review-done`, `review-response`, `feedback`, `grooming`, `tool-issue`, `agent-idle`, `spawn-ack`, `agent-error`.

<!-- edict:managed-start -->
## Edict Workflow

Tools teach their own commands: bones `bn tldr` · maw `maw tldr` (`maw --help`) · seal `seal --help` · rite `rite tldr` · edict `edict protocol --help`. Some tool docs are out of date: where a tool's quick start differs from the Rules below, the Rules win. Identity: `$AGENT`, set by the launcher; manual sessions use `<project>-dev`.

Layout: the repo root is the trunk (the `default` workspace); agent workspaces live in `.maw/workspaces/<name>/`.

### Rules

- **Track all work in a bone.** Create it before you start (`bn create`), move it `open` → `doing` → `done`, and post progress comments for crash recovery. See [update.md](.agents/edict/update.md).
- **Edit only in a maw workspace** named after the bone: `maw ws create <bone-id> --from main`, then work in `.maw/workspaces/<bone-id>/`. Never edit the trunk at the repo root directly, and never create git branches: maw workspaces replace them. See [start.md](.agents/edict/start.md).
- **Run each tool in its place.** Run commands in a workspace with `maw exec <ws> -- <cmd>`. Run `bn` directly at the repo root. Run seal through `maw exec <ws> -- seal`.
- **Never merge or destroy `default`.** It is the merge target.
- **Run the edict protocol at each transition:** `edict protocol resume|start|review|finish|merge|cleanup … --agent $AGENT`, then run the steps it prints, in order. If it exits 1, follow the matching workflow doc below.
- **Reviewed work merges only through `edict protocol merge <ws> --message "feat: …"`.** A bare `maw ws merge` skips the review log, the clean check and the `risk:critical` gate; use it only for work with no review. Commit no code after the LGTM, only the review log. See [merge-check.md](.agents/edict/merge-check.md#the-review-log-and-the-clean-check).
- **A conflicted workspace is a normal state, not a failure:** `maw ws resolve <ws> --list`. See [merge-check.md](.agents/edict/merge-check.md#conflict-recovery).
- **Run `maw ws recover` before you conclude work is lost** or start a bone over. Destroyed workspaces keep snapshots. See [worker-loop.md](.agents/edict/worker-loop.md).
- **Seal completion is not approval.** Confirm the verdict with `seal review <id> --format json`. `--reviewers` spawns nobody; for security review follow [security-review.md](.agents/edict/security-review.md#who-sends-what), and never mention `@<project>-security`.
- **Answer `$RITE_MESSAGE_ID` with `--reply-to`.** Never reuse the anchor from an earlier turn. See [cross-channel.md](.agents/edict/cross-channel.md#threads).
- **Ask, then wait on the anchor:** capture the id you sent (`--format json`), then `rite wait --reply-to <id> -t 300`. On exit 1, post one `-L task-blocked` and move on; never re-send. Stuck on a companion tool? Ask its project channel this way. See [cross-channel.md](.agents/edict/cross-channel.md#ask-and-wait).
- **Rite messages are one labelled line that leads with the bone id**, e.g. `-L task-blocked "<bone-id>: blocked on <thing>, needs <what unblocks it>"`. No status blocks or recaps. See [cross-channel.md](.agents/edict/cross-channel.md#message-shape).
- **Run the project's check command before committing**, and fix failures first. Workers do not push; the lead merges and pushes. See [finish.md](.agents/edict/finish.md).
- **Confirm before destructive actions** (deleting data, force-pushing, discarding unmerged work): ask the human first.

### Release

1. Commit and push to main
2. Tag and push: `maw release vX.Y.Z`
3. Install locally: `maw exec default -- just install`
4. Run `edict sync` in all downstream projects

### Design Guidelines

- [CLI tool design for humans, agents, and machines](.agents/edict/design/cli-conventions.md)

### Workflow Docs

- [worker-loop.md](.agents/edict/worker-loop.md): Full worker cycle: resume, triage, start, work, review, finish
- [triage.md](.agents/edict/triage.md): Find one actionable bone and groom along the way
- [start.md](.agents/edict/start.md): Claim a bone, create its workspace, announce
- [update.md](.agents/edict/update.md): Change a bone's state and announce it
- [review-request.md](.agents/edict/review-request.md): Request a review: commit first, review range, retarget before re-request
- [review-response.md](.agents/edict/review-response.md): Handle reviewer feedback; no code after the LGTM
- [security-review.md](.agents/edict/security-review.md): Launch one dedicated security review; who sends what
- [finish.md](.agents/edict/finish.md): Close the bone, merge or hand off, release claims; conflict recovery
- [merge-check.md](.agents/edict/merge-check.md): Merge a workspace: review log, clean check, merge gates, conflicts
- [cross-channel.md](.agents/edict/cross-channel.md): Rite threads, ask-and-wait, message shape, cross-project asks
- [report-issue.md](.agents/edict/report-issue.md): Superseded by cross-channel.md
- [planning.md](.agents/edict/planning.md): Turn a spec or PRD into actionable bones
- [scout.md](.agents/edict/scout.md): Explore unfamiliar code before planning
- [proposal.md](.agents/edict/proposal.md): Propose and validate a significant change before building it
- [groom.md](.agents/edict/groom.md): Groom ready bones to improve backlog quality
- [mission.md](.agents/edict/mission.md): Missions: split a parent bone across parallel workers
- [coordination.md](.agents/edict/coordination.md): Coordinate with sibling workers inside a mission
- [preflight.md](.agents/edict/preflight.md): Validate toolchain health before multi-agent work
<!-- edict:managed-end -->
