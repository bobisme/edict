# Changelog

## [0.32.0] - 2026-09-28

### Changed

- The edict-managed AGENTS.md section shrank from about 5,000 to about 1,400 tokens. It now points at each tool's own reference (`bn tldr`, `maw tldr`, `seal --help`, `rite tldr`, `edict protocol --help`) and keeps a short Rules list for the cross-tool policy that no `--help` teaches. The quick-reference tables, the directory tree, and the long review, merge and rite sections are gone; their detail lives in the workflow docs (`merge-check.md`, `review-request.md`, `security-review.md`, `cross-channel.md`), and each rule links to it. Rules are gated on the project's `tools` and `review` settings. A test holds the section under 900 words. An A/B eval (`evals/results/2026-09-27-managed-ab-summary.md`) found no regression.
- The ASD-STE100 writing rules and the "Replies to a human" style rules are dropped from the managed section. "Confirm before destructive actions" stays as a rule.
- Task-claim announcements lead with the bone id (`"<bone-id>: <title>" -L task-claim`).
- `edict init --no-interactive` runs `bn init` when bones is enabled. `--no-init-bones` opts out.

### Fixed

- `edict protocol finish` and `edict protocol merge` treat a code commit made after the LGTM as stale even when Seal reports `approval_stale: false`. Seal 0.30 misses such commits on the detached HEAD of a maw workspace, so unreviewed code could merge. Edict now compares the approved commit with the workspace HEAD; only the review-log commit may differ.
- `edict protocol review` no longer emits the retired `@<project>-security` mention. It emits the anchored request and the security-review.md launch contract.
- Protocol output uses the project's layout for `bn` (no `maw exec default --` in the root layout) and the real agent name (no `--agent agent`).
- Protocol merge guidance no longer claims `maw ws merge` auto-syncs a stale source. It tells the agent to run `maw ws sync` first.
- Workflow docs and loop prompts agree: run `maw ws recover` before recreating a destroyed workspace; reviewed work merges only through the protocol steps; workers do not push; a subagent review needs the Seal verdict; `maw ws merge` also merges uncommitted changes.
- The "Testing template changes safely" recipe and the hermetic tests isolate vessel (`VESSEL_SOCKET` and a `systemd-run` shim). vessel ignores `XDG_RUNTIME_DIR`, so a sandboxed hook could reach the real vessel server.

### Added

- A hermetic eval harness (`evals/`, `just eval`) with worker, review-loop and lead-merge scenarios and an AGENTS.md variant switch for A/B runs.

## [0.31.0] - 2026-09-27

### Added

- `edict sync --dry-run` and `edict init --dry-run` list every file, migration, rite hook, tool init and commit the command would make, then exit 0 without changing anything. Every mutation in init and sync goes through one choke point (`src/effects.rs`), so a new step cannot skip the dry-run by accident. `--check` still only reports stale or not stale.
- Before registering a rite hook, `edict init`/`sync` print the resolved rite data dir and each hook's name, channel, cwd and command.
- `edict doctor` runs `pi auth check` for each non-Anthropic provider the project's agents use and reports any login that is not usable.
- AGENTS.md has a "Testing template changes safely" section: a dry-run preview, then a sandboxed full run with `RITE_DATA_DIR`, `HOME` and `XDG_*` in a tempdir.

### Changed

- `edict init`/`sync` refuse to register a live rite hook for a project under the system temp dir when `RITE_DATA_DIR` is unset. Such a hook spawns real agents against a throwaway directory. `--allow-live-hooks` overrides it. A dry-run shows the refusal instead of failing. `edict init` also skips its `#projects` registration message in that case, and now sends it after hook registration, so a refused init posts nothing.
- The responder skips the agent run when every message in the spawn batch @mentions only other agents or replies in someone else's thread. Previously each such ping started a full triage run. Plain top-level messages, command prefixes, DMs, @mentions of the responder, and messages it cannot classify still run.
- Responder triage falls back to the responder's default model when the triage model fails, so one provider outage no longer fails every trigger.

### Fixed

- A failed `edict run agent` child now reports its real reason, for example `claude failed (exit 1): Invalid API key (edict run agent exited with code 4)`, instead of only the exit code. The responder's "Could not answer that" reply carries it.
- An expired or revoked runner login (pi OAuth refresh, Claude `/login`) fails with a one-line message that says how to sign in again.
- The responder no longer posts "Could not answer that" into threads addressed to other agents. The failure is still logged and counted (`edict.responder.turn_failures_total`, attribute `addressed`).
- Every step that re-requests a review after new commits runs `seal reviews retarget <id>` first. `seal reviews request` alone left the review on the old commit, so the reviewer verified stale code. This covers the protocol commands, the dev-loop and worker prompts, and the review docs.
- `security-review.md`: `rite claims stake --ttl` takes seconds (`--ttl 1200`, not `20m`), and `vessel spawn --cwd` is absolute (`$(maw cd "$ws")`), because vessel resolves a relative path against its server's cwd.

## [0.30.1] - 2026-09-23

### Fixed

- Seal review logs now reach trunk. Seal keeps each review in `.seal/reviews/<id>/` in the workspace that created it, and no workflow step committed it, so `maw ws merge --destroy` discarded every review. Whoever merges now runs `seal reviews mark-merged` in the workspace while HEAD is still the approved commit, commits only `.seal/reviews/<id>`, and then merges. `edict protocol merge` and `edict protocol finish` emit these steps before the merge, and the commit step is a no-op on a retry.
- `edict protocol finish` and `edict protocol merge` refuse to emit merge steps for a `risk:critical` bone, even with `--force`. `protocol merge --force` also blocks when it cannot load a bone bound to that exact workspace (a sole unrelated bone claim no longer counts), or when `bn show` fails, because it cannot then rule out `risk:critical`. Its human approval lives in a Rite message that no protocol state verifies, and the standalone worker prompt ran the protocol before its human-approval check. The worker prompt now runs that check first.
- After the review is recorded, a merge conflict whose resolution changes code outside `.seal/` and `.bones/` needs a fresh review. This includes restoring `.agents/` or `.claude/`. The docs and the conflict-recovery guidance say so.
- The record steps refuse to continue when anything outside `.seal/reviews/<id>/` is uncommitted. `maw ws merge` also merges uncommitted additions and deletions, so a change made after the LGTM without a commit passed `mark-merged` and landed unreviewed. The merge step repeats the check in the same shell command, in every protocol step and documented merge command, and a failed `git status` stops the merge instead of reading as clean. A small window remains while maw snapshots the workspace; closing it needs a maw merge of committed content only.
- The managed AGENTS.md section routes every merge recipe (change recipe, quick reference, lead merge workflow) through `edict protocol merge` and its steps. A bare `maw ws merge --destroy` is only for work with no review.
- No step runs `mark-merged` in a destroyed workspace. `merge-check.md` ran it inside the workspace after `--destroy`, and the protocol commands ran it in `default`, where the review does not exist.
- The docs no longer forbid every commit after the LGTM: the review log commit is the one exception. The lead's merge protocol no longer runs `git add -A` on a reviewed workspace, which merged uncommitted files that no reviewer saw. `protocol finish` also stops committing everything after an approved review. A standalone worker records the review and merges itself. A dispatched worker (`--no-merge`) leaves the review open for the lead's merge gate.

## [0.30.0] - 2026-09-18

### Changed

- The Claude hooks occupy `agent://<name>` as a rite session attachment of the harness session (`rite sessions attach --harness claude --kind pull`, from the hook payload's `session_id`) instead of an ownerless claim. Tool activity renews the attachment before its ten-minute claim lapses and SessionEnd detaches it. An ownerless claim made `rite channel` refuse the identity in the same session; a `pull` attachment of the same agent is what the channel takes over (rite bn-316s; a channel without it still refuses when the hook attaches first). A session whose start-time attach was refused, or that started before the previous session's detach ran, attaches on its next tool call. A harness whose hooks carry no session id keeps the ownerless claim. edict installs no Codex hooks, so a Codex launcher still owns reserve, attach, and detach itself.

## [0.29.1] — 2026-09-07

### Fixed

- Made the authoring agent, rather than the workspace-write Daybreak reviewer, own the anchored Rite handoff, review-claim release, and Vessel session lifecycle.
- Explicitly terminate each exact security-review Vessel session after a verified Seal verdict or after a snapshot on an unverified outcome; graceful Codex exit is followed by a bounded exact-session kill backstop.

## [0.29.0] - 2026-09-06

### Changed

- Replaced the ambient security reviewer loop and `@<project>-security` launch hooks with an explicit, exact-target Codex Daybreak security review session managed through Vessel and Agentbus.
- Retained Seal reviewer identities and approval gates while making Daybreak review completion fail closed unless the reviewer records a verified Seal vote.

### Migration

- `edict sync` retires only Edict-owned named reviewer hooks and removes the retired reviewer-loop configuration and managed templates.
