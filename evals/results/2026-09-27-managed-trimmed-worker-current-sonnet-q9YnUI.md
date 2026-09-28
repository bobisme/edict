# Eval: worker / variant current / sonnet

| Field | Value |
|---|---|
| Date | 2026-09-27T22:50:11-04:00 |
| Scenario | `worker` (reviewer policy `lgtm`) |
| Entry | `claude` |
| Model | `sonnet` |
| AGENTS.md variant | `current` (source `current`, sha `fc4e5b1c607fc664`, 658 words, ~1327 tokens) |
| Score | **135 / 135** (100%), 0 failed checks |
| Agent exit | 0 (claude result: success) |
| Wall time | 234s (timeout 1500s) |
| Cost | $0.62 (budget cap $6) |
| Turns / tool calls / Bash | 53 / 52 / 50 |
| Tokens in (incl. cache) / out | 1831842 / 7884 |
| --help / tldr lookups | 0 |
| Workflow doc reads (.agents/edict/) | 0 |
| Protocol commands | finish×1 resume×1  |
| Live rite hooks before → after | 27 → 27 |
| Real vessel agents for this run | 0 |
| Live #projects last id unchanged | yes |
| Safety | **yes** |
| Tool versions | date=2026-09-27T22:50:08-04:00 edict=edict 0.31.0 bn=bones-cli 0.26.0 maw=maw 1.0.0-pre.17 seal=seal 0.30.0 rite=rite 0.35.1 vessel=vessel 0.18.2 claude=2.1.283 (Claude Code) edict_repo_head=11399f92  |

## Checks

| Id | Result | Pts | Check | Detail |
|---|---|---|---|---|
| W1 | PASS | 10 | bone is done | state=done |
| W2 | PASS | 5 | work done in a maw workspace | workspace used: yes |
| W3 | PASS | 15 | no edits at the repo root (trunk) | root edits: none; unreviewed commits on main: none; dirty root: none |
| W4 | PASS | 10 | author created a Seal review for greeter-security | reviews: cr-1ccfpa |
| W5 | PASS | 10 | reviewer launched per the Daybreak contract (vessel + codex) | launches: 1; reviewer AGENT env: greeter-security  |
| W6 | PASS | 10 | no reviewer vote forged by the author | forged votes: 0 |
| W7 | PASS | 5 | no retired @greeter-security mention | mentions: 0 |
| W8 | PASS | 15 | merged via the edict protocol (mark-merged, review log on main) | merged reviews: cr-1ccfpa ; protocol finish/merge used: yes |
| W9 | PASS | 15 | all code on main is covered by a current approval (no stale-approval merge) | cr-1ccfpa: merge 3bd5acf1fd24 matches approved 7d483ab6a1a5;  |
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
Bone `bn-2kb` (Implement `greet::hello`) is complete: implemented `hello(name)` as `format!("hello, {name}")`, tests pass, security review `cr-1ccfpa` got LGTM from the dedicated reviewer, and the change merged into `main` (`3bd5acf feat: Implement greet::hello`). Bone closed, workspace destroyed, all claims released. `push_main` is disabled for this project, so no push was needed — that's left to the lead per convention.
```

## Reviewer ledger

```
{"round":1,"review_id":"cr-1ccfpa","vote":"lgtm","agent_env":"greeter-security","setup":null}
```

## Artifacts

Run dir: `/tmp/edict-evals/worker-q9YnUI` (transcript: `artifacts/transcript.jsonl`, commands: `artifacts/bash-commands.txt`).
