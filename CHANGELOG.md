# Changelog

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
