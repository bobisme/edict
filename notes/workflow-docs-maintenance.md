# Workflow Docs Maintenance

## Where Docs Live

Workflow docs are embedded in the `botbox` binary as templates in `src/templates/docs/`. When `botbox init` runs in a target project, they're rendered into `.agents/botbox/` and referenced from the generated AGENTS.md.

## Doc Index

| Doc | Purpose |
|-----|---------|
| triage.md | Find exactly one actionable bead from inbox and ready queue |
| start.md | Claim a bead, create a workspace, announce |
| update.md | Post progress updates during work |
| finish.md | Close bead, merge workspace, release claims, sync |
| worker-loop.md | Full triage-start-work-finish lifecycle |
| review-request.md | Request a code review via seal |
| review-response.md | Handle reviewer feedback (fix/address/defer) and merge after LGTM |
| review-loop.md | Reviewer agent loop until no pending reviews |
| merge-check.md | Verify approval before merging |
| preflight.md | Validate toolchain health before starting work |
| report-issue.md | Report bugs/features to other projects via #projects registry |
| groom.md | Groom ready beads: fix titles, descriptions, priorities, break down large tasks |

## When to Update Docs

These docs define the protocol that every agent follows. Update them when:
- A bus/maw/br/seal/vessel CLI changes its flags or behavior
- You discover a missing step, ambiguity, or edge case during real agent runs
- A new workflow is added (e.g., a new review strategy, a new teardown step)

Do **not** update docs for project-specific conventions — those belong in the target project's AGENTS.md above the managed section.

## How to Update Docs

1. Edit the template in `src/templates/docs/`
2. Run `cargo test` — the version hash will change, confirming the update is detected
3. If adding a new doc, register it in the template rendering code in `src/commands/sync.rs`
4. Target projects pick up changes on their next `botbox sync`
