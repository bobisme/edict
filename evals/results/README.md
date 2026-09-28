# Eval Results

Runs from the hermetic harness (2026-09 on) are written by `evals/scripts/run.sh`
as `<date>-<label>-<scenario>-<variant>-<model>-<id>.md`, one file per run. Labels:
`smoke` (harness check), `managed-baseline` (bn-25zp), `managed-trimmed` (bn-1kp6).
See `evals/README.md` and the top section of `evals/rubrics.md`.

The table below covers the legacy (Jan-Feb 2026) runs, made with the botbox-era
tools and the scripts now in `evals/scripts/legacy/`.

## Runs

| Run | Type | Model | Beads | Score | Key Finding |
|-----|------|-------|-------|-------|-------------|
| L2-1 | Single session | Opus | 1 | 92/92 (100%) | Baseline: perfect protocol compliance |
| L2-2 | Single session | Sonnet | 1 | 81/92 (88%) | Missing optional steps (triage, progress, cleanup) |
| L2-3 | Single session | Sonnet | 1 | 88/92 (96%) | Progress comment docs helped (+7) |
| L2-4 | Single session | Sonnet | 1 | 83/92 (90%) | Workspace destroy docs helped (+2) |
| L2-5 | Single session | Sonnet | 3 | 89/96 (93%) | Multi-bead unlocked triage + grooming |
| L2-6 | Single session | Sonnet | 2 | 92/96 (96%) | maw output fix confirmed workspace path usage |
| Loop-1 | agent-loop.sh v1 | Sonnet | 2 | 28/30 shell | Sandbox blocked file writes; found has_work() bugs |
| Loop-2 | agent-loop.sh v1 | Sonnet | 2 | 211/218 (97%) | Happy path works! Both beads completed across iterations |
| Loop-3 | agent-loop.sh v2 | Sonnet | 2 | 117/218 (54%) | CWD deletion broke all finish steps |
| Loop-4 | agent-loop.sh v2 | Sonnet | 2 | 215/218 (99%) | CWD fix validated — absolute paths resolve finish breakage |
| Loop-5 | agent-loop.sh v3 | Sonnet | 2 | 215/248 (87%) | Inbox triage completely skipped (0/30) — prompt too dense |
| Loop-6 | agent-loop.sh v3 | Sonnet | 2 | 245/248 (99%) | Inbox perfect (30/30) after splitting INBOX as separate step |
| Loop-7 | agent-loop.sh v3 | Sonnet | 2 | 232/248 (94%) | Duplicate bead from inbox; 4 iterations instead of 3 |
| Loop-8 | agent-loop.sh v3 | Haiku | 2 | 205/248 (83%) | First haiku run: no inbox replies, duplicate bead, stale br ready |
| Loop-9 | agent-loop.sh v3 | Haiku | 2 | 65/248 (26%) | **FAIL**: bead spam from inbox, phantom close, timeout |
| Loop-10 | agent-loop.sh v2.1 | Haiku | 2 | 206/218 (94%) | Clean run — excellent grooming, tests, br sync fix confirmed |
| R1-1 | Review (Fixture A) | Sonnet | — | 51/65 (78%) | Found path traversal; 3 false positives (Axum route syntax, static mut) |
| R1-2 | Review (Fixture A) | Sonnet | — | 61/65 (94%) | v2 prompt: clippy + web search eliminated Axum FP, grounded static mut |
| R1-3 | Review (Fixture A v2) | Sonnet | — | 65/65 (100%) | Fixed fixture: static mut was genuinely problematic, not clean code |
| R2-1 | Author Response | Sonnet | — | 65/65 (100%) | All 3 threads fixed correctly; canonicalize+starts_with for path traversal |
| R3-1 | Full Review Loop | Sonnet | — | 60/65 (92%) | Re-review LGTM + merge; first merge attempt timed out (wrong crit command) |
| R4-1 | Integration (Full Lifecycle) | Sonnet | 1 | 89/95 (94%) | End-to-end triage→merge works; re-review needed prompt fix for workspace visibility |
| R4-2 | Integration (Full Lifecycle) | Sonnet | 1 | 95/95 (100%) | crit v0.9.1 vote override fix confirmed; perfect score with workspace path hint |
| R8-1 | Adversarial Review (v1) | Sonnet | — | 54/65 (83%) | v1 single-file: found all 3 bugs; 1 FP on permission check; over-severity on quality |
| R8-2 | Adversarial Review (v2) | Opus | — | 49/65 (75%) | v2 multi-file: found race + TOCTOU but missed pagination; no cross-file reasoning |
| R8-3 | Adversarial Review (v2) | Sonnet | — | 41/65 (63%) | v2 multi-file: **FAIL** — TOCTOU missed entirely; pagination missed; quality perfect |
| R7-1 | Planning (Decomposition + Execution) | Opus | 1+7 | 76/95 (80%) | Diamond DAG (7 subtasks, 3 parallel). Completed 3/7 before context limit. 8 tests pass. |
| E10-1 | Full Lifecycle (2 projects, 3 agents, 8 phases) | Opus+Sonnet | 2 | 158/160 (99%) | Near-perfect. Security reviewer found 7 issues (2 CRITICAL). All agents followed protocol. |
| E10-2 | Full Lifecycle (2 projects, 3 agents, 8 phases) | Opus+Sonnet | 2 | 159/160 (99%) | Reproducible. Clean run with no setup workarounds needed. crit FK constraint persists. |
| E11-L3-1 | Botty-Native Full Lifecycle (2 projects, 3 agents) | Opus | 2 | 133/140 (95%) | First vessel-native eval. All agents spawn via hooks. Cross-project coordination organic. |
| E11-L4-1 | Mission (simple project) | Opus | 1+4 | 68/125 (54%) | Agent worked solo — single-file project made decomposition irrational. |
| E11-L4-6 | Mission (modular project) | Opus | 1+4 | 37/125 (30% cap) | Uncapped 93/125 (74%). Perfect protocol, but tasks ~30 LOC each — agent rationally chose solo. |
| E11-L4-7 | Mission (bulked specs) | Opus+Sonnet | 1+4 | 39/130 (30% cap) | Uncapped 108/130 (83%). Agent used Task tool instead of vessel spawn — bypassed coordination. |
| E11-L4-8 | Mission (vessel required) | Opus+Sonnet | 1+4 | 124/130 (95%) | **First full mission success.** 3 vessel workers, dependency-aware dispatch, 46 tests, ~8 min. |
| E11-L4-9 | Mission (prompt tuning) | Opus+Sonnet | 1+4 | 119/130 (92%) | Fewer tool errors (8 vs 12) from --env-inherit fix; lost 5 pts on claims staking inconsistency. |
| E11-L4-10 | Mission (solo regression) | Opus | 1+4 | 39/130 (30% cap) | Agent went solo despite 4 children — "coordination overhead" rationalization. Prompt not strong enough. |
| E11-L4-11 | Mission (dispatch required) | Opus+Sonnet | 1+4 | 122/130 (94%) | **All 26 checks pass.** Strengthened dispatch language worked. Only friction points lost (7 errors, 5 retries). |

## Key Learnings

- Multi-bead evals are strictly better (force observable triage/grooming)
- Every doc/tooling improvement produced measurable score gains
- Workspace path usage requires maw's "IMPORTANT" output line (fixed in maw v0.6.0+)
- Merge issue fixed in maw v0.8.0
- `workspace://<project>/<workspace>` is the claim URI format for workspaces
- `claude -p` needs `--dangerously-skip-permissions` for autonomous file operations
- `has_work()` had two JSON parser bugs (br ready returns array, bus inbox --count-only returns int)
- Agent's own bus messages fixed upstream: bus v0.3.8 filters self-messages from inbox
- Single-workspace merge fixed: maw v0.9.0 supports merging when only 1 workspace exists
- Run `br` commands from project root, not inside `.workspaces/$WS/` (prevents beads merge conflicts)
- Agent naming convention: `<project>-dev` for interactive, random names for agent-loop.sh
- **Do not `cd` into workspace and stay there** — use absolute paths for file ops, `maw ws jj` for jj commands. Workspace destroy deletes the directory and breaks the shell session (Loop-3 regression, Loop-4 fix)
- `br ready` may show stale state after workspace merge — agent can waste an iteration re-doing closed work (Loop-4, Loop-7 observation)
- **Duplicate bead detection from inbox is inconsistent** — agent sometimes creates a new bead from inbox task-request instead of recognizing an existing bead covers it (Loop-7). Prompt says "do NOT create another bead" but this isn't always followed.
- **Reviewer prompt: clippy + web search + severity levels** dramatically reduce false positives (R1-1 → R1-2: 3 FPs → 1). Instruction to "ground findings in evidence" is key.
- **Eval fixtures must be genuinely correct** — original R1 fixture used `static mut` as "clean code" but it was actually problematic (clippy warns, deprecated, unsound under tokio). Reviewer was right to flag it. Fixed in Fixture A v2.
- **`claude -p` via shell script is more reliable than inline** — long prompts with escaped quotes in direct bash invocation caused sessions to hang. Writing a launcher script with a `$PROMPT` variable resolved the issue.
- **Reviewer severity levels provide sufficient signal for author triage** — R2 agent correctly prioritized CRITICAL > MEDIUM > INFO without explicit "if CRITICAL then fix" logic. The review comments themselves communicated required action.
- **All comments treated as "fix"** — R2 Run 1 fixed all 3 threads. Doesn't exercise "address" (won't-fix) or "defer" (create bead) paths. Future R2 runs should include a comment the author should push back on.
- **Workspace visibility is critical for re-review** — When the reviewer re-reviews after author fixes, they must read code from the workspace (`.workspaces/$WS/`), not the main branch. The main branch still has the pre-fix code until merge. Re-review prompts must include the workspace path explicitly.
- **Crit index doesn't update votes on override** — When a reviewer casts LGTM after previously blocking, the SQLite index retains the old block vote. The event log is correct. Workaround: `rm .crit/index.db` to force rebuild.
- **Full dev-agent lifecycle works end-to-end** — R4 validates triage→start→work→review→feedback→merge with 5 sequential `claude -p` invocations, 2 agents, coordinated via crit+botbus+beads. Score: 89/95 (94%).
- **Full review loop works with sequential `claude -p` invocations** — each agent reads shared state (crit + botbus), acts, updates state for next agent. No explicit agent-to-agent communication needed.
- **`crit reviews merge` not `crit reviews close`** — agent timed out trying to find a "close" command. Precise command names in prompts prevent this.
- **Reviewer re-review was thorough** — read actual code, ran clippy, verified each fix against original issue. Didn't rubber-stamp based on author's thread replies alone.
- **crit v0.9.1 vote override fix confirmed** — R4-2 LGTM properly overrides block in SQLite index. The 6-point Phase 4 improvement (4/10 → 10/10) is entirely attributable to this fix + workspace path hint.
- **Sonnet finds execution-path bugs with the v2 prompt** — R8-1 found all 3 adversarial bugs (race condition, TOCTOU delete, pagination underflow) that require comparing code paths rather than pattern matching. Expected range was 35-50; actual was 54/65 (83%). The v2 prompt's evidence-grounding instruction helps with subtle bugs too.
- **Clean code traps must be truly unambiguous** — R8-1 flagged the `mode & 0o444` permission check as LOW because the comment said "Standard Unix permission check" but the code only checks if bits are set, not actual process readability. The reviewer's argument has some merit. Future traps should be code that is both correct AND has accurate comments.
- **Quality issue over-severity is the main calibration gap** — R8-1 rated the non-UTF-8 `.unwrap()` as HIGH ("DoS") rather than LOW. While a panic is impactful, the trigger (non-UTF-8 filename on disk) isn't attacker-controlled in normal upload flows. Severity calibration degrades with more complex code.
- **R4 results are reproducible** — R4-1 and R4-2 with different agents, same protocol, same outcomes (modulo the fixed bug). Validates the eval framework produces consistent measurements.
- **Multi-file split is a meaningful difficulty increase** — R8 v2 (7 files) dropped scores significantly vs v1 (1 file). Sonnet: 83% → 63% (FAIL). The TOCTOU bug went from trivially found (adjacent functions) to completely missed (separate files). Cross-file reasoning is genuinely harder than single-file scanning.
- **Presence of correct-looking code creates false confidence** — In R8-3, Sonnet saw `canonicalize()` + `starts_with()` in delete.rs and concluded it was correct, without tracing which variable flows into subsequent operations. Explicitly stated "download and delete correctly use canonicalize" when delete doesn't.
- **Both models missed pagination in the multi-file layout** — Neither Opus nor Sonnet found `(page - 1) * per_page` underflow when page=0 in list.rs. The "boring" list endpoint got less scrutiny when split into its own file. In v1, Sonnet found it.
- **Cross-file reasoning doesn't emerge naturally** — Neither model explicitly compared download.rs (correct `&canonical`) vs delete.rs (buggy `&file_path`) when identifying the TOCTOU. Opus found the bug through single-file analysis; Sonnet missed it entirely. The cross-file reasoning rubric category measures something real.
- **v2 FP rules are better calibrated** — LOW/MEDIUM comments on clean code traps are legitimate reviewer observations that authors can triage. Only penalizing HIGH+ or block citations avoids unfair deductions for valid low-severity nitpicks.
- **E10 full lifecycle validated at 99%** — 2 Rust projects, 3 agents (Opus + Sonnet), 8 phases, cross-project communication, security review block/fix/LGTM cycle, full finish protocol. Agents gracefully worked around setup bugs (missing bookmark, broken path dependency).
- **`crit reviews list` FK constraint bug** — Threads created referencing reviews not yet in SQLite index. Manifests as "Failed to apply event (type: ThreadCreated) FOREIGN KEY constraint failed". Needs crit fix. Workaround: grep agent logs for review IDs.
- **Tool JSON key names are inconsistent** — `maw ws list` omits `path`, `crit reviews list` uses `review_id` not `id`, `crit review` uses `thread_id` not `id`. Eval scripts must match actual JSON schemas.
- **`agent://` claims are hook-managed** — When hooks fire on announcements, they re-stake `agent://` claims. Verify scripts should only check for work claims (`bead://`, `workspace://`).
- **`vessel tail` only shows the last session** — When an agent restarts (e.g., alpha-security spawned twice for initial review + re-review), the first session's log is lost from `vessel tail`. Verify scripts must also check channel history as a fallback. Run scripts should capture incremental logs when agents transition from running to not running.
- **Cargo path dependencies break across `maw init`** — `path = "../beta"` in Cargo.toml resolves relative to the workspace. After `maw init` moves files to `ws/default/`, the path changes. Fix: compile both projects before running `botbox init` on either.
- **E11-L3 validates the full vessel-native spawn chain** — hooks → vessel spawn → loop scripts → cross-project coordination → review cycle. All autonomous from a single task-request. 133/140 (95%), only friction points lost.
- **crit workspace confusion is a persistent friction source** — Agents run `maw exec default -- crit ...` instead of `maw exec <ws> -- crit ...`. This accounted for 2/9 tool errors in E11-L3-1. The distinction (br = always default, crit = always workspace) is documented but agents still get confused when switching between the two.
- **Mission eval: tasks must be substantial enough for parallelism** — E11-L4-1/L4-6 failed because subcommand specs were ~30 LOC each, making the agent rationally choose solo work. After bulking specs to ~100-200 LOC per subcommand with multiple flags and modes, the agent correctly chose parallel dispatch.
- **Agents will use Task tool over vessel spawn if not explicitly told** — E11-L4-7 showed the agent dispatching 3 Claude Code Task subagents instead of vessel spawn workers. Task subagents are faster but bypass all coordination infrastructure (no observability, no crash recovery, no bus messages, no maxWorkers). Explicit "use vessel spawn, NOT Task tool" guidance in the prompt fixed this.
- **`--pass-env` vs `--env-inherit`** — The dev-loop prompt template said `--pass-env` but vessel uses `--env-inherit`. Agents recover by checking `--help` but it costs a retry. The prompt template should match the actual CLI.
- **`br list -l` only shows open beads by default** — Run/verify scripts must use `--all` when looking up mission beads and children that may be closed. The agent closes mission beads before the run script captures artifacts.
- **Bash associative array expansion with `set -u`** — `${!ARRAY[@]+${!ARRAY[@]}}` is unreliable. Use `if [[ ${#ARRAY[@]} -gt 0 ]]; then for k in "${!ARRAY[@]}"; do` instead.
- **Dependency-aware dispatch works** — E11-L4-8 correctly did error.rs first (blocker), merged it, then dispatched 3 parallel workers for the unblocked children. The dependency graph was correctly interpreted.
- **Verify script regex: `\|` in `grep -E` is literal pipe, not alternation** — Check 15 (count/status in checkpoint) was failing even when checkpoints existed because `grep -iE "foo\|bar"` matches literal `|`, not alternation. Use `"foo|bar"` with `-E`. This masked a passing check in runs 8-9.
- **Error counting must distinguish tool friction from intentional CLI testing** — Agents run `cargo run` with bad inputs to verify error handling. Those exit code 1s are NOT tool errors. Filter by checking the preceding command: `cargo run`, `cd && cargo run`, `./target/` are intentional project testing. Only count errors from bus/br/maw/vessel/jj/cargo build/test.
- **Claims staking for workers is inconsistent across runs** — Run 8 staked 3+ claims (pass); Run 9 staked only 2 (fail). The dev staked claims for its own error.rs work but not for the 3 worker beads/workspaces. The prompt tells workers to stake their own claims, but the dev should also pre-stake for dispatched workers.
- **Agents rationalize solo work even with strong dispatch signals** — Run 10 (39/130): agent decomposed into 4 children then said "I'll do them myself sequentially to ensure quality — spawning workers would add coordination overhead." Must use imperative language: "You MUST dispatch workers. Do NOT implement children yourself sequentially."
- **Missions enabled by default is safe** — After runs 8-11, the mission framework is validated. Enabled by default in dev-loop.mjs and botbox init config. Agents correctly decompose, dispatch, monitor, and synthesize.

## Upstream Tool Versions (as of 2026-01-31)

- bus v0.3.8: self-message filtering, `claims --since`, `#channel` syntax
- maw v0.15.0: `maw ws merge --destroy` default (no `-f`), single-workspace merge, agent-oriented error output, absolute path guidance in help text, jj concept explanations
- All workflow docs updated with eval learnings (identity, br-from-root, tool-issue reporting, progress comments, blocked bead re-evaluation, absolute workspace paths)

## Scoring Rubric

### Single Session (96 points, multi-bead)

- Critical steps: 50 pts (claim, start, finish, release, sync)
- Optional steps: 16 pts (identity, triage, groom, workspace create/path/destroy, progress, announce)
- Work quality: 20 pts (task complete, tests pass, code quality)
- Error handling: 10 pts (progress updates, bug reporting)
- Pass: ≥70 pts (73%) | Excellent: ≥85 pts (89%)

### Agent Loop (218 points = 30 shell + 2×94 per-iteration)

- Shell mechanics: 30 pts (lease, spawn announce, has_work() gating, one-bead-per-iteration, cleanup, shutdown)
- Per-iteration: 94 pts (50 critical + 14 optional + 20 quality + 10 error handling; identity N/A = -2)
- Pass: ≥170 pts (77%) | Excellent: ≥200 pts (90%)

### Author Response (65 points)

- CRITICAL fix: 25 pts (identifies as must-fix, secure code fix, compiles, thread reply, no regressions)
- MEDIUM fix: 15 pts (identifies as should-fix, correct fix, thread reply, no breakage)
- INFO handling: 10 pts (identifies as non-blocking, appropriate action, thread reply)
- Protocol compliance: 15 pts (jj commit, re-request review, botbus announcement)
- Pass: ≥45 pts (69%) | Excellent: ≥55 pts (85%)

### Integration / R4 (95 points)

- Phase 1 (Work + Review Request): 40 pts (triage 10, start 5, implementation 10, review request 10, deferred finish 5)
- Phase 2 (Reviewer): 20 pts (bug/quality assessment 10, correct vote 5, protocol 5)
- Phase 3 (Handle Feedback): 15 pts (categorize 3, fix issues 5, thread replies 3, compile+re-request 4) — auto-award if Phase 2 LGTM
- Phase 4 (Re-review): 10 pts (read code 3, verify+LGTM 5, announcement 2) — auto-award if Phase 2 LGTM
- Phase 5 (Merge + Finish): 10 pts (verify LGTM 2, crit merge 2, maw merge 2, close+release 2, sync+announce 2)
- Pass: ≥66 pts (69%) | Excellent: ≥81 pts (85%)

### Adversarial Review / R8 (65 points, v2)

- Bug detection: 30 pts (race condition 12, TOCTOU delete 12, pagination underflow 6)
- Blocking decision: 5 pts (block if HIGH+ issues exist)
- Quality feedback: 10 pts (non-UTF-8 unwrap 3, silent error discard 3, constructive 4)
- Cross-file reasoning: 5 pts (explicitly compare download.rs vs delete.rs for TOCTOU)
- FP resistance: 5 pts (only penalize if clean trap flagged HIGH+ or cited in block reason)
- Protocol compliance: 10 pts (crit commands 5, botbus announcement 5)
- Pass: ≥45 pts (69%) | Excellent: ≥55 pts (85%)

### Planning / R7 (95 points)

- Phase 1 — Decomposition: 45 pts (triage+recognition 10, subtask creation 15, dependency graph 15, SQLite adaptability 5)
- Phase 2 — Execution: 50 pts (worker loop compliance 25, implementation quality 15, cross-subtask coherence 10)
- Pass: ≥66 pts (69%) | Excellent: ≥81 pts (85%)

### Scoring Notes

- **Progress comments**: Required by docs (cheap insurance for crash recovery), but only -1 pt if missing on a task completed quickly. The requirement exists for failure-case visibility, not ceremony.

## Individual Reports

- [Loop-1](2026-01-30-agent-loop-run1-sonnet.md)
- [Loop-2](2026-01-30-agent-loop-run2-sonnet.md)
- [Loop-3](2026-01-30-agent-loop-run3-sonnet.md)
- [Loop-4](2026-01-30-agent-loop-run4-sonnet.md)
- [Loop-5](2026-01-30-agent-loop-run5-sonnet-v3.md)
- [Loop-6](2026-01-30-agent-loop-run6-sonnet-v3.md)
- [Loop-7](2026-01-30-agent-loop-run7-sonnet-v3.md)
- [Loop-8](2026-01-30-agent-loop-run8-haiku-v3.md)
- [Loop-9](2026-01-30-agent-loop-run9-haiku-v3.md)
- [Loop-10](2026-01-30-agent-loop-run10-haiku-v2.1.md)
- [R1-1](2026-01-31-review-run1-sonnet.md)
- [R1-2](2026-01-31-review-run2-sonnet.md)
- [R1-3](2026-01-31-review-run3-sonnet.md)
- [R2-1](2026-01-31-review-r2-run1-sonnet.md)
- [R3-1](2026-01-31-review-r3-run1-sonnet.md)
- [R4-1](2026-01-31-review-r4-run1-sonnet.md)
- [R4-2](2026-01-31-review-r4-run2-sonnet.md)
- [R8-1](2026-02-01-review-r8-run1-sonnet.md)
- [R8-2](2026-02-01-review-r8-run2-opus.md)
- [R8-3](2026-02-01-review-r8-run3-sonnet.md)
- [R7-1](2026-02-01-planning-r7-run1-opus.md)
- [E10-1](2026-02-06-e10-run1-opus.md)
- [E10-2](2026-02-07-e10-run2-opus.md)
- [E11-L3-1](2026-02-11-e11-l3-run1-opus.md)
