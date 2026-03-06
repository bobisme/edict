# DCG (Destructive Command Guard) Evaluation

Evaluation of [DCG](https://github.com/Dicklesworthstone/destructive_command_guard) for potential integration with botbox/vessel.

## What It Does

DCG is a pre-execution security hook that intercepts potentially destructive commands before they run. It targets AI coding agents (Claude Code, Gemini CLI, Aider, Codex, Continue) and blocks commands that could permanently destroy uncommitted work or critical infrastructure.

### Commands Blocked

**Git operations:**
- `git reset --hard`, `git reset --merge` (destroys uncommitted changes)
- `git checkout -- <file>`, `git restore` without `--staged` (discards modifications)
- `git clean -f` (permanently deletes untracked files)
- `git push --force` (overwrites remote history)
- `git stash drop/clear` (permanently deletes stashes)
- `git branch -D` (force-deletes branches)

**Filesystem:**
- `rm -rf` operations outside temp directories (`/tmp`, `/var/tmp`, `$TMPDIR`)

**Embedded script scanning:**
Detects destructive operations within heredocs and inline scripts in bash, Python, JavaScript, TypeScript, Ruby, Perl, and Go.

### Commands Allowed

Safe operations pass through silently: `git status`, `git log`, `git add`, `git commit`, `git push` (without `--force`), `git pull`, temporary directory cleanup, branch creation with `-b`, staged operations.

## Installation

### Quick Install (Recommended)
```bash
curl -fsSL "https://raw.githubusercontent.com/Dicklesworthstone/destructive_command_guard/master/install.sh?$(date +%s)" | bash -s -- --easy-mode
```

This auto-detects platform (Linux x86_64/ARM64, macOS Intel/Apple Silicon, Windows) and downloads the appropriate prebuilt binary.

### Install Options
- `--easy-mode` - Automatic configuration (registers hooks with detected agents)
- `--from-source` - Compile locally (requires Rust nightly)
- `--system` - System-wide installation (sudo)
- `--no-configure` - Skip agent hook setup
- `--version <tag>` - Specific version

### Updates
```bash
dcg update
```

## How It Works

### Hook Integration

DCG integrates directly into AI agent command flows (not shell preexec):

- **Claude Code**: Uses `PreToolUse` hook protocol via `~/.claude/settings.json`
- **Gemini CLI**: Integrates through `BeforeTool` hooks in `~/.gemini/settings.json`
- **Aider**: Operates via git pre-commit hooks

The hook receives JSON input on stdin containing the command, returns either allow (empty stdout) or deny (JSON response with reason and suggestions).

### Performance Architecture

Six-stage pipeline optimized for sub-millisecond execution:

1. **JSON Parsing** - Extracts command from hook input
2. **Command Normalization** - Strips absolute paths using zero-copy `Cow<str>`
3. **Quick Rejection** - SIMD-accelerated substring matching via memchr
4. **Safe Pattern Matching** - Whitelist check (early exit)
5. **Destructive Pattern Matching** - Blacklist check using dual regex engines
6. **Heredoc Analysis** - Optional three-tier AST-based scanning

**Performance budget**: <50μs for critical operations, 200ms hard timeout for entire pipeline.

**Fail-open design**: On timeouts, parse errors, or resource limits, DCG allows execution while logging warnings. This prevents workflow blockages.

### Dual Regex Engine

- **Linear Engine** (`regex` crate): O(n) guaranteed for ~85% of patterns
- **Backtracking Engine** (`fancy_regex`): Supports lookahead/lookbehind for ~15% of patterns

## Configuration

### Five-Layer Configuration Hierarchy (ascending priority)

1. Compiled defaults
2. System config (`/etc/dcg/config.toml`)
3. User config (`~/.config/dcg/config.toml`)
4. Project config (`.dcg.toml` in repo root)
5. Environment variables (`DCG_*` prefix)

### Modular Pack System

49+ security packs organized by category:

**Core (enabled by default):**
- `core.filesystem` - Dangerous rm -rf outside temp
- `core.git` - Destructive git operations

**Optional categories:**
- Databases (PostgreSQL, MySQL, MongoDB, Redis, SQLite)
- Containers (Docker, Podman, Docker Compose)
- Orchestration (kubectl, Helm, Kustomize)
- Cloud (AWS, Azure, GCP with service-specific sub-packs)
- Infrastructure (Terraform, Pulumi, Ansible)
- Messaging (Kafka, NATS, RabbitMQ)
- CI/CD (GitHub Actions, GitLab CI, Jenkins)

Example config:
```toml
[packs]
enabled = [
  "database.postgresql",
  "kubernetes.kubectl",
  "cloud.aws"
]

[agents.claude-code]
trust_level = "high"
```

### Allowlist System

Three-layer allowlist (project, user, system):
```bash
dcg allowlist add <rule_id> -r "reason"
dcg allowlist add-command "git push --force" -r "intentional history rewrite"
```

### CLI Commands

```bash
dcg                         # Hook mode (primary function)
dcg test <command>          # Evaluate without executing
dcg explain <command>       # Detailed decision trace
dcg allow-once <code>       # Temporary exception (expires 24h)
dcg scan                    # Repository scanning for committed commands
dcg packs --verbose         # List available packs
```

## Pros for Botbox Integration

### Aligns with Botbox Philosophy

1. **Pre-execution interception**: Catches mistakes before they happen, not after.

2. **Agent-aware**: Specifically designed for AI coding agents, with per-agent trust levels.

3. **Configurable per-project**: `.dcg.toml` allows project-specific rules, aligning with botbox's project-centric approach.

4. **Fail-open design**: Won't block legitimate work due to bugs or timeouts.

5. **Comprehensive coverage**: Blocks more than simple shell aliases would (heredoc scanning, embedded scripts).

6. **Good UX**: Provides actionable suggestions ("Use `--force-with-lease` instead") rather than just blocking.

### Practical Benefits

7. **Low overhead**: Sub-millisecond latency via SIMD and lazy compilation.

8. **Easy installation**: Single curl command, auto-detects platform.

9. **Extensible**: Custom packs via YAML in `~/.config/dcg/packs/` or `.dcg/packs/`.

10. **Audit trail**: Can be used in scan/CI mode for pre-commit checks.

## Cons for Botbox Integration

### Architectural Mismatches

1. **Git-focused, not jj-focused**: DCG's core patterns are Git-specific. Botbox uses jj (Jujutsu). While jj has some interop with git commands, the protection patterns don't match jj's native commands (`jj abandon`, `jj op undo`, etc.).

2. **PreToolUse hook dependency**: DCG integrates via Claude Code's PreToolUse hooks. This is a different layer than vessel's PTY-based runtime. Integration paths:
   - **Botty**: Could potentially wrap commands through DCG, but would require custom integration.
   - **Claude Code hooks**: Already supported, but botbox already manages Claude Code hooks in `.agents/botbox/hooks/`.

3. **Rust binary dependency**: Adds another binary to the toolchain (though it's self-contained).

### Overlap with Existing Protections

4. **Claude Code already has some protections**: Claude's built-in safety won't run `rm -rf /` without confirmation. DCG adds more granular control but there's overlap.

5. **Botbox hooks could do similar checks**: Our existing hook infrastructure (rite hooks, Claude Code hooks) could implement project-specific guards without DCG.

### Gaps

6. **Doesn't cover jj-specific dangers**: Commands like `jj abandon`, `jj op restore`, `jj workspace forget` aren't covered.

7. **Doesn't protect against non-shell operations**: File operations via Python, JavaScript, API calls, etc. are not intercepted.

8. **Can be bypassed**: Commands in scripts (not piped to bash) aren't caught. Determined attackers can work around it.

## Recommendation: DEFER

**Do not integrate DCG into botbox at this time.** Revisit when:

1. **jj support exists**: If DCG adds jj-specific patterns, it becomes more relevant.
2. **Botty integration path is clearer**: If we want command-level safety in vessel, DCG could be wrapped as a guard.
3. **User demand**: If users request it, we can add a migration that installs DCG.

### Why Not Skip Entirely?

DCG is a well-designed tool solving a real problem. The engineering (SIMD patterns, fail-open design, heredoc scanning) is solid. The ecosystem alignment (same author as beads_rust) suggests future compatibility.

### What to Do Instead

For botbox projects:

1. **Rely on jj's safety**: jj has operation history and conflict handling that makes "destructive" operations recoverable via `jj op restore`.

2. **Use maw workspace isolation**: Each agent works in an isolated workspace, limiting blast radius.

3. **Trust Claude's built-in protections**: Claude Code already refuses truly dangerous operations.

4. **Add project-specific guards in hooks if needed**: For specific projects with high-risk commands, add custom checks in `.agents/botbox/hooks/`.

### Future Consideration

If we want command-level safety in vessel:

```bash
# Hypothetical vessel integration
vessel spawn --guard dcg claude -- "work on feature"
```

This would pipe all commands through DCG before execution. But this requires vessel changes and DCG's permission model would need to work with our jj-based workflow.

---

**Status**: Deferred
**Revisit trigger**: jj pattern support in DCG, or explicit user request
**Alternative**: Project-specific hooks for high-risk commands
