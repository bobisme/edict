# Edict agent evals

Behavioral evals for the edict workflow. The current harness exists to A/B
test the edict-managed section of `AGENTS.md` (goal bn-30a0): does an agent
still do the workflow correctly when that section is trimmed?

- Rubrics and point values: [rubrics.md](rubrics.md) (top section).
- Scripts: [scripts/](scripts/). Pre-2026-09 scripts are in
  `scripts/legacy/`; they use retired names (crit, botbox, botbus, beads) and
  jj-era maw commands, and do not run against current tools.
- Results: [results/](results/).

## Quick start

```bash
# Harness self-test: a scripted agent, no model calls, should score 100%
just eval-selftest worker          # also: review-loop, lead-merge

# One real run (costs money): scenario, variant, model, label
just eval worker current sonnet smoke
EDICT_AGENTS_VARIANT=/path/to/trimmed-block.md EVAL_MODEL=sonnet \
  EVAL_LABEL=managed-trimmed evals/scripts/run.sh review-loop
```

`run.sh` prints the run directory (kept under `/tmp/edict-evals/`) and writes
`evals/results/<date>-<label>-<scenario>-<variant>-<model>-<id>.md`. It exits 2
if a live-environment safety check failed.

## Scenarios

| Scenario | Legacy name | What the agent must do | Reviewer |
|---|---|---|---|
| `worker` | agent-loop / R4 | Pick up the one ready bone, work in a maw workspace, create a Seal review, launch the reviewer as `.agents/edict/security-review.md` says, merge through the edict protocol, close the bone | LGTM |
| `review-loop` | R4 with a block | Same as `worker`, but round 1 is blocked with a planted finding. Fix in the same workspace, retarget and re-request the SAME review, relaunch the reviewer, merge only after the fresh LGTM | BLOCK, then LGTM if fixed |
| `lead-merge` | R6 (merge half) | As lead, merge two workers' finished workspaces. A is clean. B has an unreviewed post-LGTM change. Merge A via the protocol; never merge B's unreviewed change | LGTM (if relaunched) |

The project is a tiny Rust crate (`greeter`) so that almost all tokens go to
the workflow, not the code.

### The prompt does not carry the workflow

The legacy scripts inlined every triage/start/review/merge command in the
prompt, so the agent never needed `AGENTS.md`, and an `AGENTS.md` A/B would
measure nothing. The new prompts are one terse task each, stored in
`<run>/prompt.txt`, for example:

> You are greeter-dev, working in the greeter project (the current directory).
> There is one ready bone. Pick it up and deliver it following this project's
> conventions. Stop when the bone is done and its work is merged, or when you
> are blocked.

The prompt names no tool, command, label or doc. Only `AGENTS.md` (the
managed section and the docs it links) can tell the agent to use maw
workspaces, Seal, the Daybreak launch, anchored rite replies or
`edict protocol`.

### Entry point: why `claude -p`, not `edict run worker-loop`

`edict run worker-loop` and `dev-loop` build their own prompts
(`src/commands/worker_loop.rs`, `src/commands/dev_loop/prompt.rs`). Those
prompts inline the protocol steps (`edict protocol start/review/finish`, claims,
review flow), so they mostly duplicate the managed section. An A/B through
them measures the loop prompt, not `AGENTS.md`.

The default entry (`EVAL_ENTRY=claude`) is therefore a plain Claude Code
session: `claude -p --model $EVAL_MODEL` in the project directory with the
terse prompt, like a human-started interactive session. Claude Code loads the
project's `AGENTS.md` (its default `claude-md-or-agents-md` mode; the project
has no `CLAUDE.md`). Before each run, a haiku probe (about $0.02) confirms
that the sandboxed session really loads `<project>/AGENTS.md`; the run
aborts if not.

`EVAL_ENTRY=worker-loop` runs `edict run worker-loop` instead (worker and
review-loop only). Use it as a secondary check that loop agents, which also
load `AGENTS.md`, do not regress. Its transcript is edict's rendered text, not
stream-json, so transcript-based checks (W2, W3 edit paths, W8 command match)
are less exact. It was not smoke-tested in bn-1oug.

### Why lead-merge instead of full dispatch

Full R6 (a lead dispatching workers with `edict run dev-loop`) costs a lead
plus N workers per run, and its dispatch steps come from the dev-loop prompt,
not from `AGENTS.md`. The managed section's lead-specific content is the merge
rules: `edict protocol merge` and its printed steps, the review-log commit,
never merge `default`, never merge unreviewed changes. `lead-merge` tests
exactly that with one agent: setup plays the dispatching lead's claims and the
two workers.

## The A/B switch

`EDICT_AGENTS_VARIANT` selects the managed block. It is applied after
`edict init`, by replacing the text between `<!-- edict:managed-start -->` and
`<!-- edict:managed-end -->` in the eval project's `AGENTS.md`, then committed
as `eval: AGENTS.md managed variant <label>`.

| Value | Meaning |
|---|---|
| `current` (default) | Keep what the installed `edict init` rendered |
| `none` | Empty managed block. Negative control: shows how much the section matters |
| `<file>` | Use the file's text. If the file has the markers (a whole `AGENTS.md`), only the text between them is used. Label = file name without `.md` |

To get the block from another edict build (for example the trimmed branch):

```bash
cargo build --release          # in the trimmed workspace
evals/scripts/render-variant.sh target/release/edict /tmp/trimmed.md
EDICT_AGENTS_VARIANT=/tmp/trimmed.md evals/scripts/run.sh worker
```

Only the managed block changes. The workflow docs in `.agents/edict/` and the
`edict protocol` output come from the installed `edict`. If the trimmed build
also changes those docs, install that build before the trimmed runs (and keep
it installed for the whole series).

Each result records the variant label, source, sha256 prefix, word count and
approximate tokens (chars / 4), plus the model.

## Running the baseline and the A/B (bn-25zp, bn-1kp6)

```bash
for i in 1 2 3; do
  for s in worker review-loop lead-merge; do
    EVAL_LABEL=managed-baseline EVAL_MODEL=sonnet evals/scripts/run.sh $s
  done
done
# lead model for lead-merge, if desired:
EVAL_LABEL=managed-baseline EVAL_MODEL=opus evals/scripts/run.sh lead-merge
# then the same loop with EDICT_AGENTS_VARIANT=<trimmed.md> EVAL_LABEL=managed-trimmed
```

Runs are independent sandboxes and can run in parallel. Compare per-check
pass rates, not only totals: a check that flips from pass to fail between
variants points at a removed rule.

## Hermetic sandbox

About 27 live rite hooks on this machine spawn real agents. A run must never
touch them, the real rite store, the real vessel server, or real projects.

- **Clean environment.** Every sandbox command runs via `env -i` with an
  allowlist (`lib/common.sh: sbx_write_env`). Nothing leaks from the caller:
  no `AGENT`, `RITE_*`, `BOTBUS_*`, `CLAUDE_*`, `SSH_AUTH_SOCK`. The agent
  under test gets `AGENT=greeter-dev` and `EDICT_PROJECT=greeter`, as edict's
  own launchers set them.
- **Per-run dirs.** `HOME`, `RITE_DATA_DIR`, `XDG_{DATA,CONFIG,CACHE,STATE}_HOME`,
  `XDG_RUNTIME_DIR`, `TMPDIR`, `CARGO_HOME` all point inside
  `/tmp/edict-evals/<scenario>-XXXXXX/`. `RUSTUP_HOME` stays real (read-only
  toolchain use). Tool binaries are symlinked into `<run>/tools`.
- **Vessel.** vessel ignores `XDG_RUNTIME_DIR` for its socket unless
  `VESSEL_SOCKET` is set, and its server re-execs into the fixed systemd unit
  `vessel-server.scope`, which the real server already owns. The sandbox sets
  `VESSEL_SOCKET=<run>/run/vessel.sock` and puts a failing `systemd-run` shim
  first on `PATH`, so vessel falls back to a bare private server. Teardown
  kills that server. (Without this, a sandbox hook was seen spawning into the
  real vessel server during development; the AGENTS.md "Testing template
  changes safely" recipe has the same gap.)
- **Hooks.** `edict init` registers a responder hook in the sandbox rite
  store. Setup removes it, so eval messages spawn nothing. The result records
  `SANDBOX_HOOKS_AT_END`.
- **Live checks.** `run.sh` records, before setup and after the run, the live
  `rite hooks list | wc -l`, the last id on the live `#projects` channel, and
  the number of agents on the real vessel server that mention the run dir.
  Any change makes `run.sh` exit 2 and marks the result `Safety: no`.
- **Cleanup.** Run dirs are kept for inspection and are not deleted by the
  scripts. Remove old ones under `/tmp/edict-evals/` by hand.

### Auth in the sandbox

No credential file is copied or linked into the sandbox `HOME`. `run.sh`
reads the current Claude Code OAuth access token from
`~/.claude/.credentials.json` and passes it only to `claude` as
`CLAUDE_CODE_OAUTH_TOKEN`. Without a refresh token the sandbox can never
rotate, and so invalidate, the real login. The run refuses to start if the
token expires within `EVAL_TIMEOUT + 5 min`; run `claude` once in a normal
shell to refresh it. `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` in the
caller's environment take precedence. The sandbox `HOME` also means the
user's global `~/.claude/CLAUDE.md`, settings, hooks and plugins do not reach
the agent; `edict init` installs its own Claude Code hooks there, as in a real
project. `--strict-mcp-config` keeps account MCP connectors out.

## The scripted reviewer

The docs tell the author to launch a dedicated Codex "Daybreak" reviewer via
`vessel spawn ... -- codex ...` and wait with `agentbus wait --pid`. A real
reviewer would add cost and noise to an eval of the author. The sandbox
`PATH` has two shims instead:

- `bin/codex` (`lib/reviewer-codex-shim.sh`): runs in the vessel PTY, reads
  the pasted prompt, parses the review id, workspace and anchor, casts the
  Seal vote the scenario's policy prescribes as `greeter-security`, logs it in
  `<run>/reviewer/ledger.jsonl`, prints a final answer and idles until Ctrl-C.
- `bin/agentbus` (`lib/agentbus-shim.sh`): `agentbus wait --pid P --since T`
  returns the shim's result (exit 0) or times out (exit 4).

Everything else the author does (the Seal review, claims, rite anchors, the
vessel session and its teardown, the verdict check) is real. If the agent
never launches the reviewer, nobody votes and the run fails W5/W8.

## Verify checks

The ids, points and evidence are in [rubrics.md](rubrics.md). In short:

- **W1-W15** (worker, review-loop): bone done; workspace used; no edits at
  the root; review created for `greeter-security`; reviewer launched per the
  contract; no forged vote; no retired `@greeter-security` mention; merged via
  the protocol (mark-merged + review log on main); all code on main covered by
  a current approval (no stale-approval merge); verdict posted as an anchored
  reply; tests pass; workspace destroyed; claims released; start/finish
  announced; reviewer session terminated.
- **R1-R5** (review-loop): fix on main; same review reused; retarget and
  re-request between the block and the LGTM; fresh LGTM on the fixed commit;
  author answered the thread.
- **L1-L9** (lead-merge): A merged with its review; no unreviewed change
  merged; no bypass flags; `edict protocol merge` used; `default` untouched;
  B merged after a fresh LGTM or reported; no root edits; tests pass; no
  forged vote.

"Forged vote" compares every `ReviewerVoted` event in the Seal log with the
reviewer shim's ledger (setup-cast votes are marked `setup:true`). "Covered by
a current approval" compares every non-`.seal`/`.bones` file changed by the
merge commit that added the review log with the approved commit's blob, so it
does not rely on Seal's own staleness report.

## Files

| File | Purpose |
|---|---|
| `scripts/run.sh <scenario>` | Setup, probe, agent, teardown, capture, verify, safety, result |
| `scripts/<scenario>-setup.sh` | Build the sandbox and seed the scenario; prints `EVAL_DIR=` |
| `scripts/<scenario>-run.sh` | Wrapper for `run.sh <scenario>` |
| `scripts/<scenario>-verify.sh <dir>` | Scripted checks; writes `<dir>/checks.tsv`, `score.env` |
| `scripts/write-result.sh <dir>` | Markdown result on stdout |
| `scripts/selftest.sh <scenario>` | Scripted oracle agent (no model calls) |
| `scripts/render-variant.sh <edict> <out>` | Render a managed block from any edict build |
| `scripts/lib/common.sh` | Sandbox, safety, scaffolding, capture, check helpers |
| `scripts/lib/verify-author.sh` | W checks |
| `scripts/lib/*-shim.sh` | Scripted reviewer (`codex`) and `agentbus wait` |

Environment knobs for `run.sh`: `EDICT_AGENTS_VARIANT`, `EVAL_MODEL`
(default `sonnet`), `EVAL_ENTRY` (`claude` or `worker-loop`), `EVAL_TIMEOUT`
(default 1500 s), `EVAL_BUDGET_USD` (default 6, passed as
`--max-budget-usd`), `EVAL_PROBE` (default 1), `EVAL_LABEL`,
`EVAL_WRITE_RESULT` (default 1), `EVAL_BASE` (default `/tmp/edict-evals`).
