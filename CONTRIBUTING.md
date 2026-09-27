# Contributing

Botbox is an experimental project exploring multi-agent workflows for software development. Because of its experimental nature and tight coupling with agentic tooling, the development process is unusual — most changes are made by AI agents following the botbox protocol itself.

## Filing Issues

Issues and bug reports are welcome. If you encounter a problem or have a suggestion, please open a GitHub issue with:

- What you were trying to do
- What happened instead
- Steps to reproduce (if applicable)

## Pull Requests

You're welcome to open PRs to illustrate a proposed fix or improvement. However, there's no guarantee that external PRs will be merged directly — the agentic workflow may pick up the idea and implement it through the standard bead/review pipeline instead.

If you do submit a PR:

- Keep changes focused and minimal
- Include a clear description of what and why
- Expect that the change may be re-implemented rather than merged as-is

## Development Setup

Botbox is written in **Rust** and uses **jj** (not git) for version control.

```bash
cargo build              # build
cargo test               # run tests
just lint                # cargo clippy
just fmt                 # cargo fmt
just check               # cargo check
just install             # cargo install --path .
```

Changes to workflow docs, templates, or hook-registration logic: before running `edict init`
or `edict sync` by hand, read "Testing template changes safely" in AGENTS.md. It covers the
`--dry-run` preview and the sandboxed recipe that keeps the machine's live rite hooks
untouched.

## Questions

For questions about how botbox works or how to use it, open a GitHub issue with the `question` label.
