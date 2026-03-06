# edict

![The Engine Under The Dome](images/edict-embed.jpg)

Setup, sync, and runtime for multi-agent workflows. Bootstraps projects with workflow docs and companion tool configurations, keeps them synchronized across upgrades, and provides built-in agent loop subcommands with protocol guidance.

## Eval Results

32 behavioral evaluations across Opus, Sonnet, and Haiku. The eval framework tests whether agents follow the edict protocol when driven autonomously through the vessel-native spawn chain (hooks → vessel spawn → edict run).

| Eval       | Model       | Score         | What it tests                                                                                                            |
| ---------- | ----------- | ------------- | ------------------------------------------------------------------------------------------------------------------------ |
| **E11-L3** | Opus        | 133/140 (95%) | Full lifecycle: 2 projects, 3 agents, cross-project coordination, security review cycle — all from a single task-request |
| **E10**    | Opus+Sonnet | 159/160 (99%) | 8-phase scripted lifecycle: 2 projects, 3 agents, cross-project bug discovery, review block/fix/LGTM                     |
| **E11-L2** | Opus        | 97/105 (92%)  | Botty-native dev + reviewer: single project, review cycle through real hooks                                             |
| **R5**     | Opus        | 70/70 (100%)  | Cross-project coordination: file bugs in external projects via rite channels                                              |
| **R4**     | Sonnet      | 95/95 (100%)  | Integration: full triage → work → review → merge lifecycle                                                               |
| **R8**     | Opus        | 49/65 (75%)   | Adversarial review: multi-file security bugs requiring cross-file reasoning                                              |

**Takeaway**: The full autonomous pipeline works. Agents spawn via hooks, coordinate across projects via rite channels, review each other's code via seal, and merge work through maw — all without human intervention. Friction comes from CLI typos, not protocol failures. See [evals/results/](evals/results/README.md) for all 32 runs and detailed findings.

## What is edict?

`edict` is a Rust CLI (stable edition 2024) that:

1. **Initializes projects** for multi-agent collaboration (interactive or via flags)
2. **Syncs workflow docs** from embedded templates to `.agents/edict/`
3. **Validates health** via `doctor` command
4. **Runs agent loops** as built-in subcommands (`dev-loop`, `worker-loop`, `reviewer-loop`, `responder`)
5. **Provides protocol commands** that guide agents through state transitions (`protocol start`, `merge`, `finish`, etc.)

It glues together 5 companion tools (rite, maw, br/bv, seal, vessel) into a cohesive workflow and provides the runtime that drives agent behavior.

## Install

```bash
cargo install edict-cli
# or from source:
cargo install --path .
```

## Usage

```bash
# Bootstrap a new project (interactive)
edict init

# Bootstrap with flags (for agents)
edict init --name my-api --type api --tools bones,maw,seal,rite --reviewers security --no-interactive

# Sync workflow docs after edict upgrades
edict sync

# Check if sync is needed
edict sync --check

# Validate toolchain and project setup
edict doctor

# Run agent loops (typically invoked by vessel spawn, not manually)
edict run dev-loop --agent myproject-dev
edict run worker-loop --agent myproject-dev/worker-1
edict run reviewer-loop --agent myproject-security

# Protocol commands — check state and get guidance at transitions
edict protocol start <bead-id> --agent $AGENT
edict protocol merge <workspace> --agent $AGENT
edict protocol finish <bead-id> --agent $AGENT
```

## What gets created?

After `edict init`:

```
.agents/edict/          # Workflow docs (embedded in binary, synced to project)
  docs/
    triage.md            # Find work from inbox and bones
    start.md             # Claim bone, create workspace, announce
    update.md            # Post progress updates
    finish.md            # Close bead, merge workspace, release claims
    worker-loop.md       # Full triage-start-work-finish lifecycle
    review-request.md    # Request code review via seal
    review-response.md   # Handle reviewer feedback (fix/address/defer)
    review-loop.md       # Reviewer agent loop
    merge-check.md       # Verify approval before merge
    preflight.md         # Validate toolchain health
    cross-channel.md     # Ask questions, report bugs across projects
    report-issue.md      # Report bugs/features to other projects
    planning.md          # Turn specs/PRDs into actionable bones
    scout.md             # Explore unfamiliar code before planning
    proposal.md          # Create and validate proposals before implementation
    groom.md             # Groom backlog
    mission.md           # Mission-based parallel dispatch (dev-loop)
    coordination.md      # Multi-agent coordination patterns
  design/
    cli-conventions.md   # CLI tool design for humans, agents, and machines
  prompts/
    reviewer.md          # Generic reviewer prompt template
    reviewer-security.md # Security reviewer prompt template
  hooks/                 # Claude Code event hooks (SessionStart, PostToolUse)
  .version               # Version hash for sync tracking
AGENTS.md                # Generated with managed section + project-specific content
CLAUDE.md -> AGENTS.md   # Symlink
.edict.json             # Project configuration
```

## Workflow docs

The workflow docs in `.agents/edict/docs/` define the protocol. These are embedded in the Rust binary as compile-time templates and synced to projects during `edict init` and `edict sync`.

When edict updates, run `edict sync` to pull the latest workflow doc changes.

## Agent loops

Agents are spawned automatically via rite hooks when messages arrive on project channels. The spawn chain:

```
message → rite hook → vessel spawn → edict run responder → edict run dev-loop
```

Agent loops are built-in Rust subcommands of the `edict` binary:

- **`edict run responder`** — Universal router. Routes `!dev`, `!q`, `!bead` prefixes; triages bare messages.
- **`edict run dev-loop`** — Lead dev. Triages work, dispatches parallel workers, monitors progress, merges.
- **`edict run worker-loop`** — Worker. Sequential: triage → start → work → review → finish.
- **`edict run reviewer-loop`** — Reviewer. Processes seal reviews, votes LGTM or BLOCK.
- **`edict run triage`** — Token-efficient triage. Wraps `bv --robot-triage` with scannable output.
- **`edict run iteration-start`** — Combined status snapshot. Aggregates inbox, bones, reviews, claims.

No manual agent management needed — send a message to a project channel and the hook chain handles the rest.

## Ecosystem

Edict coordinates these specialized tools that work together to enable multi-agent workflows:

| Tool                                            | Purpose                          | Key commands                                  | Repository                                          |
| ----------------------------------------------- | -------------------------------- | --------------------------------------------- | --------------------------------------------------- |
| **[rite](https://github.com/bobisme/rite)** | Communication, claims, presence  | `send`, `inbox`, `claim`, `release`, `agents` | Pub/sub messaging, resource locking, agent registry |
| **[maw](https://github.com/bobisme/maw)**       | Isolated jj workspaces           | `ws create`, `ws merge`, `ws destroy`         | Concurrent work isolation with Jujutsu VCS          |
| **[bones](https://github.com/bobisme/bones)**   | Issue tracking and triage (`bn`) | `create`, `next`, `do`, `done`, `triage`      | Event-sourced issue tracker with built-in triage    |
| **[seal](https://github.com/bobisme/seal)**  | Code review                      | `review`, `comment`, `lgtm`, `block`          | Asynchronous code review workflow                   |
| **[vessel](https://github.com/bobisme/vessel)**   | Agent runtime                    | `spawn`, `kill`, `tail`, `snapshot`           | Process management for AI agent loops               |

### How they work together

1. **rite** provides the communication layer: agents send messages, claim resources (bones, workspaces), and discover each other
2. **bones** tracks work items and priorities, exposing a triage interface (`bn next`, `bn triage`)
3. **maw** creates isolated workspaces so multiple agents can work concurrently without conflicts
4. **seal** enables code review: agents request reviews, reviewers comment, and changes merge after approval
5. **vessel** spawns and manages agent processes, handling crashes and lifecycle

**edict** configures projects to use these tools, keeps workflow docs synchronized, and runs the agent loops (`edict run dev-loop`, `edict run worker-loop`, etc.) that drive the entire workflow.

## Architecture

Edict is a Rust project (edition 2024) with:

- **Zero build step** beyond `cargo build` — workflow docs are embedded at compile time via `include_str!` and rendered with `minijinja`
- **Agent loops as subcommands** — `dev-loop`, `worker-loop`, `reviewer-loop`, `responder` are built into the binary
- **Protocol commands** — `edict protocol start/merge/finish` check preconditions and output guidance
- **Config migrations** — `edict sync` runs version-based migrations to update `.edict.json` and rite hooks

See [CLAUDE.md](CLAUDE.md) for full architecture docs, development conventions, and companion tool deep dives.

## Cross-project feedback

The `#projects` registry on rite tracks which tools belong to which projects:

```bash
# Find who owns a tool
rite history projects -n 50 | grep "tools:.*vessel"

# File bugs in their repo
cd ~/src/vessel
br create --actor $AGENT --owner $AGENT --title="Bug: ..." --type=bug --priority=2
rite send --agent $AGENT vessel "Filed bd-xyz: description @vessel-dev" -L feedback
```

See `.agents/edict/docs/report-issue.md` for full workflow.

## Development

Runtime: **Rust** (stable, edition 2024). Tooling: **clippy** (lint), **rustfmt** (format), **cargo check** (type check).

```bash
# From ws/default/ (maw v2 bare repo layout)
maw exec default -- just install    # cargo install --path .
maw exec default -- just lint       # cargo clippy
maw exec default -- just fmt        # cargo fmt
maw exec default -- just check      # cargo check
maw exec default -- just test       # cargo test
```

This project uses Jujutsu (jj) for version control and maw for workspace management. Source files live in `ws/default/`, not at the project root. Run `maw exec default -- <command>` to execute commands in the workspace context.
