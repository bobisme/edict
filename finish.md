# Finish

**Mandatory teardown** after completing work on a bead. Never skip this, even on failure paths.

All steps below are required — they clean up resources, prevent workspace leaks, and ensure the bead ledger stays synchronized. Run `br` commands via `maw exec default --` and `crit` commands via `maw exec $WS --`.

## Arguments

- `$AGENT` = agent identity (required)
- `<bead-id>` = bead to close out (required)

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `rite whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. Verify you posted at least one progress comment (`maw exec default -- br comments <bead-id>`). If not, add one now: `maw exec default -- br comments add --actor $AGENT --author $AGENT <bead-id> "Progress: <what was done>"`
3. Add a completion comment to the bead: `maw exec default -- br comments add --actor $AGENT --author $AGENT <bead-id> "Completed by $AGENT"`
4. Close the bead: `maw exec default -- br close --actor $AGENT <bead-id> --reason="Completed" --suggest-next`
5. **Check risk-based merge requirements** before merging:
   - Check the bead's risk label: `maw exec default -- br show <bead-id>` (look for `risk:low`, `risk:high`, `risk:critical` in labels)
   - **risk:low**: A review may not have been created — that's expected. Proceed directly to merge (step 6).
   - **risk:medium** (default, no label): Standard path — review should already be LGTM before reaching finish.
   - **risk:high**: Verify the security reviewer completed the failure-mode checklist (5 questions answered in review comments) before merge. Check: `maw exec $WS -- crit review <review-id>` and confirm comments address failure modes, edge cases, rollback, monitoring, and validation.
   - **risk:critical**: Verify human approval exists. Check rite history for an approval message referencing the bead/review from a listed approver (`.botbox.json` → `project.criticalApprovers`): `rite history $BOTBOX_PROJECT -n 50`. If found, record the approval message ID in a bead comment: `maw exec default -- br comments add --actor $AGENT --author $AGENT <bead-id> "Human approval received: rite message <msg-id>"`. If no approval found, do NOT merge — instead post: `rite send --agent $AGENT $BOTBOX_PROJECT "risk:critical bead <bead-id> awaiting human approval before merge" -L review-request` and STOP.
6. **Merge and destroy the workspace**: `maw ws merge $WS --destroy` (where `$WS` is the workspace name from the start step — **never `default`**)
   - The `--destroy` flag is required — it cleans up the workspace after merging
   - **Never merge or destroy the default workspace.** Default is where other workspaces merge into.
   - `maw ws merge` now produces linear history: workspace commits are rebased onto main and squashed into a single commit (as of v0.22.0)
   - Scaffolding commits are automatically abandoned; main bookmark is automatically moved and ready for push
   - If merge fails due to conflicts, do NOT destroy. Instead add a comment: `maw exec default -- br comments add --actor $AGENT --author $AGENT <bead-id> "Merge conflict — workspace preserved for manual resolution"` and announce the conflict in the project channel.
   - If the command succeeds but the workspace still exists (`maw ws list`), report: `rite send --agent $AGENT $BOTBOX_PROJECT "Tool issue: maw ws merge --destroy did not remove workspace $WS" -L tool-issue`
7. Release all claims held by this agent: `rite claims release --agent $AGENT --all`
8. Sync the beads ledger: `maw exec default -- br sync --flush-only`
9. **If pushMain is enabled** (check `.botbox.json` for `"pushMain": true`), push to GitHub main:
   - `maw push` (maw v0.24.0+ handles bookmark and push automatically)
   - If push fails, announce: `rite send --agent $AGENT $BOTBOX_PROJECT "Push failed for <bead-id>, manual intervention needed" -L tool-issue`
10. Announce completion in the project channel: `rite send --agent $AGENT $BOTBOX_PROJECT "Completed <bead-id>: <bead-title>" -L task-done`

## After Finishing a Batch of Beads

When you've completed multiple beads in a session (or a significant single bead), check if a **release** is warranted:

**Chores only** (docs, refactoring, config changes, version bumps):
- Push to main is sufficient, no release needed

**Features or fixes** (user-visible changes):
- Follow the project's release process:
  1. Bump version (Cargo.toml, package.json, etc.) using **semantic versioning**.
  2. Update changelog/release notes if the project has one.
  3. Commit the release prep in default workspace: `maw exec default -- git add -A && maw exec default -- git commit -m "chore: release vX.Y.Z"`
  4. Run release: `maw release vX.Y.Z`
  5. Announce on rite: `rite send --no-hooks --agent $AGENT $BOTBOX_PROJECT "<project> vX.Y.Z released - <summary>" -L release`

Use **conventional commits** (`feat:`, `fix:`, `docs:`, `chore:`, etc.) for clear history.

A "release" = user-visible changes shipped with a version tag. When in doubt, release — it's better to ship small incremental versions than batch up large changes.

## Merge Conflict Recovery

If `maw ws merge` detects conflicts:

### Quick fix for ledger/docs conflicts only

`.beads/issues.jsonl` often conflicts because multiple agents update it concurrently. If your feature changes are clean and only ledger/docs paths conflict (`.beads/`, `.agents/`, `.claude/`):

```bash
maw exec $WS -- git restore --source refs/heads/main -- .beads/ .agents/ .claude/
```

Then retry `maw ws merge $WS --destroy`.

Then retry `maw ws merge $WS --destroy`.

### Full recovery when conflicts are messy

If merge retries keep failing:

```bash
# 1. Inspect detailed conflicts
maw ws conflicts $WS --format json

# 2. Undo local merge attempt and return workspace to a clean base
maw ws undo $WS

# 3. Ensure workspace is synced to latest default
maw ws sync $WS

# 4. Resolve and stage files in the workspace
maw exec $WS -- git status
maw exec $WS -- git add <resolved-file>

# 5. Retry merge
maw ws merge $WS --destroy
```

### When to escalate

If recovery takes more than 2-3 attempts, preserve the workspace and escalate:

```bash
maw exec default -- br comments add --actor $AGENT --author $AGENT <bead-id> "Merge conflict unresolved. Workspace $WS preserved for manual resolution."
rite send --agent $AGENT $BOTBOX_PROJECT "Merge conflict in $WS for <bead-id>. Manual help needed." -L tool-issue
```

If the workspace was accidentally removed, recreate it with `maw ws restore $WS`.

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- The workspace was created with `maw ws create --random` during [start](start.md). `$WS` is the workspace name from that step.
