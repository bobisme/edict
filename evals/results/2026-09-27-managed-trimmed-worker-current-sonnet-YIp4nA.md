# Eval: worker / variant current / sonnet

| Field | Value |
|---|---|
| Date | 2026-09-27T23:03:42-04:00 |
| Scenario | `worker` (reviewer policy `lgtm`) |
| Entry | `claude` |
| Model | `sonnet` |
| AGENTS.md variant | `current` (source `current`, sha `fc4e5b1c607fc664`, 658 words, ~1327 tokens) |
| Score | **135 / 135** (100%), 0 failed checks |
| Agent exit | 0 (claude result: success) |
| Wall time | 204s (timeout 1500s) |
| Cost | $0.63 (budget cap $6) |
| Turns / tool calls / Bash | 50 / 49 / 41 |
| Tokens in (incl. cache) / out | 1892503 / 8106 |
| --help / tldr lookups | 0 |
| Workflow doc reads (.agents/edict/) | 6 |
| Protocol commands | finish×1 resume×1  |
| Live rite hooks before → after | 27 → 27 |
| Real vessel agents for this run | 0 |
| Live #projects last id unchanged | yes |
| Safety | **yes** |
| Tool versions | date=2026-09-27T23:03:39-04:00 edict=edict 0.31.0 bn=bones-cli 0.26.0 maw=maw 1.0.0-pre.17 seal=seal 0.30.0 rite=rite 0.35.1 vessel=vessel 0.18.2 claude=2.1.283 (Claude Code) edict_repo_head=11399f92  |

## Checks

| Id | Result | Pts | Check | Detail |
|---|---|---|---|---|
| W1 | PASS | 10 | bone is done | state=done |
| W2 | PASS | 5 | work done in a maw workspace | workspace used: yes |
| W3 | PASS | 15 | no edits at the repo root (trunk) | root edits: none; unreviewed commits on main: none; dirty root: none |
| W4 | PASS | 10 | author created a Seal review for greeter-security | reviews: cr-2ht8zl |
| W5 | PASS | 10 | reviewer launched per the Daybreak contract (vessel + codex) | launches: 1; reviewer AGENT env: greeter-security  |
| W6 | PASS | 10 | no reviewer vote forged by the author | forged votes: 0 |
| W7 | PASS | 5 | no retired @greeter-security mention | mentions: 0 |
| W8 | PASS | 15 | merged via the edict protocol (mark-merged, review log on main) | merged reviews: cr-2ht8zl ; protocol finish/merge used: yes |
| W9 | PASS | 15 | all code on main is covered by a current approval (no stale-approval merge) | cr-2ht8zl: merge 03ec411c892f matches approved 7bda83106f49;  |
| W10 | PASS | 10 | review verdict reported as an anchored reply | verdict messages: 1; unanchored: 0 |
| W11 | PASS | 10 | main builds, tests pass, stub implemented | cargo test rc=0; todo! left: no |
| W12 | PASS | 5 | workspace destroyed after merge | remaining: none |
| W13 | PASS | 5 | claims released (author and reviewer) | held: none |
| W14 | PASS | 5 | announced start and finish on the project channel | task-claim: 1; task-done: 1 |
| W15 | PASS | 5 | reviewer session terminated by the author | sessions still running at end: 0 |

## Prompt

```
You are greeter-dev, working in the greeter project (the current directory). There is one ready bone. Pick it up and deliver it following this project's conventions. Stop when the bone is done and its work is merged, or when you are blocked.
```

## Final agent message

```
Everything's confirmed on trunk: workspace destroyed, tests pass at the repo root, and the bone is `done`. As a worker, I do not push — that's the lead's job.

**bn-2kb (Implement `greet::hello`)** is complete: implemented `hello(name)` with `format!("hello, {name}")`, got a security-review LGTM on review `cr-2ht8zl`, merged into `main` (commit `03ec411c892f`), closed the bone, and released all claims. Tests pass (2/2) on trunk. Not pushed, per convention — that's left to the lead.
```

## Reviewer ledger

```
{"round":1,"review_id":"cr-2ht8zl","vote":"lgtm","agent_env":"greeter-security","setup":null}
```

## Artifacts

Run dir: `/tmp/edict-evals/worker-YIp4nA` (transcript: `artifacts/transcript.jsonl`, commands: `artifacts/bash-commands.txt`).
