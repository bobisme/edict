# Managed-section A/B: baseline (pre-trim) vs trimmed (bn-25zp, bn-1kp6)

Binary under test: one `edict` release build from this workspace, HEAD
`11399f92` (workspace synced onto main `5797c72b`, plus one added commit for
the pre-trim variant file). `cargo build --release`; the resulting
`target/release/edict` was put first on `PATH` (via
`evals/.bin/edict` symlink) for every run below, so both variants and all
three scenarios ran against the identical binary — the managed block was the
only thing that changed between baseline and trimmed runs. No knob existed
for this in the harness, so this `PATH`-prefix approach was used (noted here
and left in the workspace at `evals/.bin/`).

- **Baseline variant**: `evals/variants/managed-pre-trim.md` — 3,076 words,
  ~5,068 tokens (harness char/4 estimate).
- **Trimmed variant**: `current` (this HEAD's `edict init` output) — 658
  words, ~1,327 tokens. Comfortably under the bn-30a0 exit-gate budget of
  ~1,500 tokens.
- 3 runs per scenario per variant, interleaved (i=1..3: worker-baseline,
  worker-trimmed, review-loop-baseline, review-loop-trimmed,
  lead-merge-baseline, lead-merge-trimmed), 18 runs total, run sequentially.
- Models: `sonnet` for `worker` and `review-loop`, `opus` for `lead-merge`.
- All 18 result files are in `evals/results/2026-09-27-managed-{baseline,trimmed}-*.md`.

## Safety

All 18 runs: **Safety: yes**. Live rite hook count stayed at 27 → 27 for
every run; live `#projects` last id unchanged; 0 real vessel agents
referenced any run dir. No run exited 2. Auth token did not expire during
the ~44-minute series (22:31–23:15 local).

## Cost / time

- Total cost across all 18 runs (main-agent `claude -p` cost only, from each
  run's `total_cost_usd`): **$10.80**. Add ~18 × $0.02 for the per-run haiku
  AGENTS.md-load probe: **~$11.2 total**, well under the $25 stop-limit and
  in the expected $10–15 range.
- Total wall time (agent run only, sum of all 18): 2,575s (~43 min); the
  whole series including setup/teardown/build ran in ~44 minutes.

## Results table (mean / min, n=3 per cell)

| Scenario | Variant | Mean score | Min score | Max possible | Failing checks (count/3) | Mean tool calls | Mean cost | Mean wall time | Managed-block size |
|---|---|---|---|---|---|---|---|---|---|
| worker | baseline (pre-trim) | 133.3 | 130 | 135 | W14 ×1 | 30.3 | $0.52 | 129.7s | 3,076 words / ~5,068 tok |
| worker | trimmed (current) | 135.0 | 135 | 135 | none | 48.3 | $0.61 | 244.7s | 658 words / ~1,327 tok |
| review-loop | baseline (pre-trim) | 181.7 | 180 | 185 | R5 ×2 | 53.0 | $0.99 | 235.7s | 3,076 words / ~5,068 tok |
| review-loop | trimmed (current) | 185.0 | 185 | 185 | none | 57.7 | $0.88 | 189.0s | 658 words / ~1,327 tok |
| lead-merge | baseline (pre-trim) | 103.3 | 100 | 110 | L7 ×2 | 5.7 | $0.28 | 25.3s | 3,076 words / ~5,068 tok |
| lead-merge | trimmed (current) | 106.7 | 100 | 110 | L7 ×1 | 8.7 | $0.31 | 34.0s | 658 words / ~1,327 tok |

("Tool calls" = every tool_use in the transcript, including Bash; only
`worker`/`review-loop` go through Claude Code's full tool surface —
`lead-merge` setup hands the agent a narrower job so its counts are smaller
by construction, consistent across variants.)

## Verdict: does trimmed regress anywhere?

**No.** Every failing check in this series either:
- failed *only* under baseline (W14, R5), or
- failed under both variants at the same or a lower rate under trimmed (L7:
  2/3 baseline vs 1/3 trimmed).

Trimmed's mean score matched or beat baseline in all three scenarios
(worker 135.0 vs 133.3, review-loop 185.0 vs 181.7, lead-merge 106.7 vs
103.3). At n=3 none of these differences should be over-read, but the
direction is uniformly non-regressive, and the trimmed group had strictly
fewer failing checks overall (1 vs 5) across the 18 runs.

Trimmed used noticeably more tool calls on `worker` (48.3 vs 30.3 mean) and
somewhat more on `review-loop`/`lead-merge`. Checked directly: `--help`/`tldr`
lookups were rare and about even across variants (2 total in baseline runs,
2 total in trimmed runs, out of 12 sonnet runs). The extra trimmed tool
calls were mostly additional `.agents/edict/*.md` doc reads and routine
exploration, not repeated failed attempts — bn-1kp6's stated expectation
("some extra --help or tldr calls are expected and fine; a wrong action is
not") holds: no wrong action was observed under trimmed that wasn't also
seen under baseline.

### Per-check root cause analysis

**L7 — "no edits at the repo root (trunk)" (lead-merge; baseline 2/3, trimmed 1/3)**

Cause, confirmed from `bash-commands.txt` in all three failing runs
(`lead-merge-5qNT69`, `lead-merge-TJT2Dw` baseline; `lead-merge-OefSFW`
trimmed): the lead ran `cargo test` directly at the project root (not inside
a maw workspace) to sanity-check `main` after merging — reasonable lead
behavior — and this generates an untracked `Cargo.lock` at the root, which
the harness's `.gitignore` (only `/target/`) does not cover. The check
treats any dirty root file as a failure, including this build artifact.

This is **not traceable to a rule the trim dropped**: it occurred *more*
often under the fuller pre-trim block (2/3) than under trimmed (1/3), and
neither managed-block variant tells the lead not to run `cargo test` at the
root, or to `.gitignore` `Cargo.lock`. This looks like a harness/scaffold
gap (recommend adding `Cargo.lock` to the `greeter` scaffold's
`.gitignore`, or having L7/W3 exclude recognized build artifacts) rather
than an agent-behavior regression. Flagging per rubrics.md's own guidance
that a check failing "in every run for tool reasons, not agent reasons,
carries no A/B signal."

**R5 — "author answered the reviewer's thread in Seal" (review-loop; baseline 2/3, trimmed 0/3)**

Cause: R5 requires a Seal `CommentAdded` event by the author (i.e. `seal
reply <thread-id> --agent ... "..."` on the reviewer's specific comment
thread), not a `bn bone comment add`. In the two failing baseline runs
(`review-loop-80wYUO`, `review-loop-ScWMmk`) the agent retargeted and
re-requested review without ever calling `seal reply` on the blocking
thread. In the one baseline pass and **all three** trimmed runs, the agent
called `seal reply th-... --agent ... "Fixed: ..."` before retargeting.

Neither managed-block variant names the `seal reply` command explicitly —
both only describe generic `rite`-message reply anchoring and `bn bone
comment add`; the specific instruction to answer a Seal review thread comes
from the constant, variant-independent workflow doc
(`.agents/edict/security-review.md`) and the reviewer's own printed
instructions, which are identical in every run. Since the miss rate was
*higher* under baseline (fuller text) than trimmed, this is not attributable
to anything the trim removed — it reads as ordinary Sonnet execution
variance at n=3, if anything favoring trimmed.

**W14 — "announced start and finish on the project channel" (worker; baseline 1/3, trimmed 0/3)**

Cause: in `worker-nR0HtV` (baseline), the agent went straight from claiming
the bone to requesting review, never sending the `-L task-claim` rite
message, then did send `task-done` at the end. Both managed-block variants
explicitly describe the `[task-claim] Working on <bone-id>: <title>`
announcement (pre-trim inline; trimmed via the `start.md` pointer, "Claim a
bone, create its workspace, announce"). The rule is present in both texts;
this is a one-off omission under the *longer* text, not something the trim
introduced, and it did not recur in any of the three trimmed runs.

### Bottom line

No regression traced to the trim in this A/B series. The two checks that
failed exclusively under baseline (R5, W14) and the one that failed more
under baseline than trimmed (L7) all have causes independent of the
managed-block content — either a harness/scaffold gap shared by both
variants (L7/Cargo.lock) or ordinary small-sample model variance on rules
present in both texts (R5, W14). Recommend the lead treat this as
supportive of proceeding with the trim, note the harness's Cargo.lock
gitignore gap as a small housekeeping fix, and, given n=3 per cell, consider
this a promising but not statistically strong result — a larger n (5–10)
would firm up the review-loop and worker findings if more confidence is
wanted before wider rollout.
