# Changelog

## [Unreleased]

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
