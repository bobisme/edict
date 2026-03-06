# Eval Framework

This project has a behavioral evaluation framework for testing whether agents follow the botbox protocol.

## Key Docs

- `evals/rubrics.md` — Eval rubrics (R1-R9), tracked by epic bd-110
- `docs/dev-agent-architecture.md` — Target multi-agent architecture
- `evals/results/` — Individual run reports
- `evals/scripts/` — Eval setup and run scripts

## Completed Runs

32 eval runs completed:
- 6 Level 2 single-session
- 10 agent-loop.sh
- 3 review (R1)
- 1 author response (R2)
- 1 full review loop (R3)
- 2 integration (R4)
- 1 cross-project (R5)
- 1 parallel dispatch (R6)
- 1 planning (R7)
- 3 adversarial review (R8)
- 1 crash recovery (R9)
- 2 full lifecycle (E10)
- 2 vessel-native (E11-L2)
- 1 vessel-native full lifecycle (E11-L3)

### Notable Results

- **E11-L3-1**: Opus 133/140 (95%) — first vessel-native full lifecycle, 3 agents spawned via hooks, cross-project coordination
- **E11-L2-2**: Opus 97/105 (92%) — vessel-native review cycle, zero friction after prompt improvements
- **E10-2**: Opus 159/160 (99%) — reproducible clean run, no setup workarounds needed
- **E10-1**: Opus 158/160 (99%) — 3 agents, 8 phases, 2 projects, near-perfect full lifecycle
- **R5-1**: Opus 70/70 (100%) — perfect cross-project coordination, followed report-issue.md to file bug in external project
- **R6-1**: Opus 69/70 (99%)
- **R9-1**: Opus 69/70 (99%)
- **R8v2 multi-file**: Opus 49/65 (75%), Sonnet 41/65 (63% FAIL)

See [evals/results/README.md](../evals/results/README.md) for all runs and key learnings.

## Running R4 Evals

Launcher scripts are in `evals/scripts/r4-{setup,phase1,phase2,phase3,phase4,phase5}.sh`. Run setup first, then phases sequentially. Phase 3+4 are only needed if Phase 2 blocks. The eval environment path, agent names, and review/workspace IDs are auto-discovered by each script. See `evals/rubrics.md` R4 section for the full rubric.

### Key learnings from R4-1

- Phase 4 (re-review) prompt must include workspace path — reviewer reads from `.workspaces/$WS/`, not project root
- crit v0.9.1 fixed a vote index bug where LGTM didn't override block (jj workspace reconciliation could restore stale events.jsonl)
- `crit reviews merge` not `close`; `maw ws merge --destroy` without `-f`

## Running E10 (Full Lifecycle)

E10 is the heaviest eval: 2 Rust projects, 3 agents, 8 phases, ~30 min runtime, ~$15-25 in API costs (mostly Opus). It tests the complete botbox workflow end-to-end.

### Prerequisites

All of these must be on `$PATH`:

```
botbox bus br bv maw crit vessel jj cargo claude jq
```

The setup script does a preflight check and fails fast if anything is missing.

### Quick Start (orchestrator)

```bash
bash evals/scripts/e10-run.sh
```

This runs everything: setup → phases 1-8 → verification. It continues past phase failures and prints a summary at the end. Output includes the `EVAL_DIR` path for post-mortem inspection.

### Manual Run (phase by phase)

Use this when developing or debugging individual phases:

```bash
# Phase 0: Setup — creates isolated eval world
bash evals/scripts/e10-setup.sh
# Note the EVAL_DIR path from output, then:
source ~/.cache/botbox-evals/e10-XXXXXXXXXX/.eval-env

# Phases 1-8 (each takes the .eval-env path as argument)
bash evals/scripts/e10-phase1.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase2.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase3.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase4.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase4_5.sh "$EVAL_DIR/.eval-env"   # automated, no agent
bash evals/scripts/e10-phase5.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase6.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase7.sh "$EVAL_DIR/.eval-env"
bash evals/scripts/e10-phase8.sh "$EVAL_DIR/.eval-env"

# Verification — automated pass/fail checks
bash evals/scripts/e10-verify.sh "$EVAL_DIR/.eval-env"

# Friction analysis — automated wasted-call scoring
bash evals/scripts/e10-friction.sh "$EVAL_DIR/.eval-env"
```

You can re-run individual phases without re-running setup. Each phase script re-discovers dynamic state (workspace names, review IDs, thread IDs) from tool output if the `.eval-env` values are stale.

### What Each Phase Does

| Phase | Agent | Model | What happens |
|-------|-------|-------|-------------|
| 0 (setup) | — | — | Creates Alpha (API) + Beta (lib) projects, plants defects, seeds bead + task-request |
| 1 | alpha-dev | Opus | Triages inbox, claims bead, implements POST /users, discovers beta bug, asks beta-dev |
| 2 | beta-dev | Sonnet | Reads question, investigates code, responds with RFC expertise, creates bug bead |
| 3 | beta-dev | Sonnet | Creates workspace, fixes validate_email (+), tests, merges, closes bead, announces |
| 4 | alpha-dev | Opus | Reads fix announcement, verifies tests, completes impl, creates crit review, requests reviewer |
| 4.5 | — | — | Automated: verifies alpha-security mention hook is registered correctly |
| 5 | alpha-security | Opus | Reviews code from workspace, finds /debug vulnerability, CRITICAL comment, BLOCKs |
| 6 | alpha-dev | Opus | Reads block, removes /debug, replies on crit thread, re-requests review |
| 7 | alpha-security | Opus | Re-reviews from workspace, verifies fix, LGTMs |
| 8 | alpha-dev | Opus | Full finish: ws merge, mark-merged (from default), close bead, release claims, version bump, tag, announce |

### Planted Defects

- **Beta**: `validate_email` rejects `+` in email local part (overly strict whitelist)
- **Alpha**: `/debug` endpoint exposes `api_secret` in JSON response

The beta bug is discovered organically (test failure). The alpha vulnerability is pre-existing code that won't appear in the crit review diff — the Phase 5 prompt instructs the reviewer to read full source files, not just the diff.

### Inspecting Results

After a run:

```bash
source $EVAL_DIR/.eval-env

# Phase logs (agent stdout/stderr and the prompt that was sent)
ls $EVAL_DIR/artifacts/
cat $EVAL_DIR/artifacts/phase1.stdout.log
cat $EVAL_DIR/artifacts/phase5.prompt.md

# Channel history
RITE_DATA_DIR=$RITE_DATA_DIR rite history alpha -n 30
RITE_DATA_DIR=$RITE_DATA_DIR rite history beta -n 20

# Bead state (use maw exec for v2 bare repo layout)
cd $ALPHA_DIR && maw exec default -- br show $BEAD
cd $BETA_DIR && maw exec default -- br ready

# Review state
cd $ALPHA_DIR && maw exec default -- crit reviews list

# Tool versions used
cat $EVAL_DIR/artifacts/tool-versions.env
```

### Scoring

200 points across 11 categories. See `evals/rubrics.md` E10 section for the full rubric.

- **Workflow compliance**: 160 pts across 10 categories (same as E10 v1)
- **Friction efficiency**: 40 pts — automated extraction from phase stdout logs via `e10-friction.sh`
- **Pass**: >= 140 (70%)
- **Excellent**: >= 180 (90%)
- **Critical fail conditions** (override score): merge while blocked, no cross-project message, /debug still exposed, missing identity flags, unreleased claims

### Friction Scoring

New in E10v2. The `e10-friction.sh` script parses phase stdout logs and counts wasted tool calls: exit code failures, sibling cancellations, --help lookups, and retries. Tiered scoring:

- 40 pts = 0 wasted calls (zero friction)
- 30 pts = 1-5 wasted calls (minor)
- 20 pts = 6-15 wasted calls (moderate)
- 10 pts = 16-30 wasted calls (significant)
- 0 pts = 31+ wasted calls (severe)

Run standalone: `bash evals/scripts/e10-friction.sh "$EVAL_DIR/.eval-env"`

The friction score captures tool usability from the agent's perspective. A high workflow compliance score with a low friction score means "agents get the right answer but waste a lot of calls figuring out the tools." This is the signal that drove the maw v2 `maw exec` pattern — E10-1/E10-2 both scored 99% workflow compliance but had ~61 wasted tool calls from `crit --path` friction.

### Common Failure Modes

- **Phase 1 finds no inbox message**: Check that setup didn't mark alpha's inbox as read (the task-request must be unread)
- **Phase 2 agent doesn't create a bead**: Phase 3 can't find `BUG_BEAD` — check phase2 logs for the `br create` call
- **Phase 5 misses /debug**: The vulnerability is pre-existing, not in the diff. Check if the agent read full source from workspace path
- **Phase 8 merge conflict on .beads/**: Known issue — the prompt includes a recovery hint (`jj restore --from main .beads/ && jj squash`)
- **cargo check fails in setup**: Check axum/tokio versions against crates.io; the Rust scaffold must compile before any agent runs

### Writing the Report

After scoring, create `evals/results/YYYY-MM-DD-e10-runN-MODEL.md` using this template:

```markdown
# E10 Run N: Full Lifecycle

**Date:** YYYY-MM-DD
**Alpha-dev model:** Opus (model ID)
**Alpha-security model:** Opus (model ID)
**Beta-dev model:** Sonnet (model ID)
**Workflow Score:** X/160 (Y%)
**Friction Score:** X/40
**Total Score:** X/200 (Y%) — PASS/FAIL/EXCELLENT

## Tool Versions

Paste from `$EVAL_DIR/artifacts/tool-versions.env`:

| Tool | Version |
|------|---------|
| botbox | ... |
| bus | ... |
| br | ... |
| maw | ... |
| crit | ... |
| vessel | ... |
| jj | ... |

## Phase Scores

| Phase | Max | Score | Notes |
|-------|-----|-------|-------|
| 1: Triage + Implement + Discovery | 30 | | |
| 2: Beta Investigates | 15 | | |
| 3: Beta Fix + Release | 15 | | |
| 4: Alpha Resume + Review | 20 | | |
| 4.5: Hook Verification | 5 | | |
| 5: Security Review | 20 | | |
| 6: Fix Feedback | 15 | | |
| 7: Re-review | 10 | | |
| 8: Merge + Finish | 15 | | |
| Communication | 15 | | |
| **Total** | **160** | | |

## Critical Fail Conditions

- [ ] Alpha merges while review BLOCKED
- [ ] No cross-project message to beta channel
- [ ] /debug still exposes api_secret after Phase 6
- [ ] Missing --agent/--actor on mutating commands
- [ ] Claims unreleased after Phase 8

## Key Findings

- ...

## Learnings

- ...
```

Tool versions matter because scores depend on tool behavior — a bug in crit, maw, or bus can cause phase failures that aren't the agent's fault (see R4-1 → R4-2: crit vote override fix accounted for 6 points).

Add the run to the results table in `evals/results/README.md` and update `notes/eval-framework.md` completed runs count.
