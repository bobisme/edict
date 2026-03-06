# Agent Flywheel Tools Catalog

Catalog of tools found on [agent-flywheel.com](https://agent-flywheel.com), with assessment of fit for the botbox ecosystem.

**Botbox ecosystem context**: beads (issue tracking), maw (jj workspaces), botbus (messaging), seal (code review), vessel (agent runtime)

---

## Core Flywheel Tools (15)

### 1. Mail - MCP Agent Mail
- **GitHub**: https://github.com/Dicklesworthstone/mcp_agent_mail
- **Stars**: ~1.4K
- **Tech**: Python, FastMCP, FastAPI, SQLite
- **Purpose**: Coordination layer for multi-agent workflows. Agents send messages, read threads, and reserve files asynchronously via MCP tools.
- **Key features**:
  - Threaded messaging between AI agents
  - Advisory file reservations
  - SQLite-backed persistent storage
  - MCP integration

**Botbox fit**: **OVERLAPS with botbus**. Very similar problem space - agent coordination and messaging. botbus uses channels/claims, Mail uses threads/file reservations. Worth investigating their MCP integration approach.

---

### 2. BV - Beads Viewer
- **GitHub**: https://github.com/Dicklesworthstone/beads_viewer
- **Stars**: ~891
- **Tech**: Go, Bubble Tea, Lip Gloss, graph algorithms
- **Purpose**: TUI for viewing and analyzing Beads issues. Uses graph theory (PageRank, betweenness centrality, critical path) to identify bottleneck tasks.
- **Key features**:
  - PageRank-based issue prioritization
  - Critical path analysis
  - Robot mode for AI agent integration
  - Interactive TUI with vim keybindings

**Botbox fit**: **DIRECTLY RELEVANT**. This is a viewer for the same "beads" system we use (`br`). The graph analysis for prioritization is interesting and could complement our `beads-tui` (`bu`). **Worth exploring** - we might want similar PageRank-based prioritization.

---

### 3. BR - beads_rust
- **GitHub**: https://github.com/Dicklesworthstone/beads_rust
- **Stars**: ~128
- **Tech**: Rust, Serde, JSONL
- **Purpose**: Local-first issue tracking for AI agents. SQLite + JSONL hybrid.
- **Key features**:
  - Local-first issue storage
  - Dependency graph tracking
  - Labels, priorities, comments
  - JSON output for agents

**Botbox fit**: **THIS IS THE SAME BEADS**. This is the `br` command we already use. Confirms we're on the same page with issue tracking approach.

---

### 4. CASS - Coding Agent Session Search
- **GitHub**: https://github.com/Dicklesworthstone/coding_agent_session_search
- **Stars**: ~307
- **Tech**: Rust, Tantivy, Ratatui, JSONL parsing
- **Purpose**: Search across all past AI coding agent sessions. Indexes conversations from Claude Code, Codex, Cursor, Gemini, ChatGPT.
- **Key features**:
  - Unified search across all agent types
  - Sub-second search over millions of messages
  - Robot mode for AI agent integration
  - Semantic search support

**Botbox fit**: **COMPLEMENTARY**. We don't have session search. This addresses a real problem - finding "how did I solve this before?" **Worth exploring** for integration or inspiration.

---

### 5. ACFS - Flywheel Setup
- **GitHub**: https://github.com/Dicklesworthstone/agentic_coding_flywheel_setup
- **Stars**: ~234
- **Tech**: Bash, YAML manifest, Next.js wizard
- **Purpose**: One-command bootstrap for agentic coding environments on fresh VPS.
- **Key features**:
  - 30-minute zero-to-hero setup
  - Installs Claude Code, Codex, Gemini CLI
  - All flywheel tools pre-configured

**Botbox fit**: **SIMILAR TO BOTBOX**. This is a setup tool like botbox. Different philosophy - ACFS does full machine setup, botbox does project setup. Not competitive, different scope.

---

### 6. UBS - Ultimate Bug Scanner
- **GitHub**: https://github.com/Dicklesworthstone/ultimate_bug_scanner
- **Stars**: ~132
- **Tech**: Bash, pattern matching, JSON output
- **Purpose**: Custom pattern-based bug scanner with 1,000+ detection rules across multiple languages.
- **Key features**:
  - 1000+ built-in detection patterns
  - Consistent JSON output format
  - Multi-language support
  - Pre-commit hooks

**Botbox fit**: **TANGENTIAL**. Static analysis tool. We use oxlint. Could be interesting as additional pre-commit check but not core to our workflow.

---

### 7. DCG - Destructive Command Guard
- **GitHub**: https://github.com/Dicklesworthstone/destructive_command_guard
- **Stars**: ~89
- **Tech**: Rust, SIMD, shell integration
- **Purpose**: Intercepts dangerous shell commands (rm -rf, git reset --hard) before execution.
- **Key features**:
  - Intercepts dangerous commands
  - SIMD-accelerated pattern matching
  - Configurable allowlists
  - Command audit logging

**Botbox fit**: **INTERESTING FOR SAFETY**. Our scripts already have some safety checks, but this is more comprehensive. Could be useful for vessel (agent runtime). **Worth exploring** for agent safety.

---

### 8. RU - Repo Updater
- **GitHub**: https://github.com/Dicklesworthstone/repo_updater
- **Stars**: ~67
- **Tech**: Bash, Git plumbing, GitHub CLI
- **Purpose**: Keep dozens/hundreds of Git repositories in sync with single command.
- **Key features**:
  - One-command multi-repo sync
  - Parallel operations
  - Conflict detection with resolution hints
  - AI code review integration

**Botbox fit**: **LOW RELEVANCE**. We use jj (Jujutsu), not git. Multi-repo sync is not our core use case. Pass.

---

### 9. CM - CASS Memory System
- **GitHub**: https://github.com/Dicklesworthstone/cass_memory_system
- **Stars**: ~152
- **Tech**: TypeScript, Bun, MCP Protocol, SQLite
- **Purpose**: Three-layer cognitive architecture: Episodic (experiences), Working (active context), Procedural (skills/lessons).
- **Key features**:
  - Three memory layers
  - MCP integration
  - Automatic memory consolidation
  - Cross-session context persistence

**Botbox fit**: **INTERESTING CONCEPT**. We don't have persistent agent memory. This is orthogonal to our issue tracking - it's about what agents learn. **Worth exploring** if we want agents to improve over time.

---

### 10. NTM - Named Tmux Manager
- **GitHub**: https://github.com/Dicklesworthstone/ntm
- **Stars**: ~69
- **Tech**: Go, Bubble Tea, tmux
- **Purpose**: Named tmux sessions with project-specific persistence. Organized workspaces for multi-agent development.
- **Key features**:
  - Named agent panes with type classification
  - Broadcast prompts to agent types
  - Session persistence across reboots
  - Dashboard view of active agents

**Botbox fit**: **SIMILAR TO BOTTY**. This is agent orchestration like vessel. Different approach - NTM uses tmux panes, vessel uses PTY management. **Worth studying** their UX for agent management.

---

### 11. SLB - Simultaneous Launch Button
- **GitHub**: https://github.com/Dicklesworthstone/simultaneous_launch_button
- **Stars**: ~49
- **Tech**: Go, Bubble Tea, SQLite
- **Purpose**: Two-person rule CLI for approving dangerous commands. Requires second reviewer.
- **Key features**:
  - Two-person rule enforcement
  - Command queue with approval workflow
  - Pattern-based risk detection
  - SQLite persistence

**Botbox fit**: **INTERESTING SAFETY CONCEPT**. Human-in-the-loop for dangerous operations. Could complement DCG. Our seal (code review) is similar but for code, not commands. **Worth exploring** for command approval workflow.

---

### 12. MS - Meta Skill
- **GitHub**: https://github.com/Dicklesworthstone/meta_skill
- **Stars**: ~10
- **Tech**: Rust, SQLite, Tantivy, MCP
- **Purpose**: Skill management platform: store, search, track effectiveness, package for sharing.
- **Key features**:
  - MCP server for native AI agent integration
  - Thompson sampling optimizes suggestions
  - Multi-layer security
  - Hybrid search with RRF

**Botbox fit**: **LOW RELEVANCE**. Skills management is not our focus. Claude Code has its own skills system. Pass.

---

### 13. RCH - Remote Compilation Helper
- **GitHub**: https://github.com/Dicklesworthstone/remote_compilation_helper
- **Stars**: ~35
- **Tech**: Rust, rsync, zstd, SSH
- **Purpose**: Offload Rust compilation to remote workers. Transparent cargo interception.
- **Key features**:
  - Transparent cargo interception
  - Multi-worker pool with priority scheduling
  - Incremental artifact sync
  - Daemon mode with status monitoring

**Botbox fit**: **LOW RELEVANCE**. Build offloading is nice but not core to agent coordination. Pass.

---

### 14. WA - WezTerm Automata
- **GitHub**: https://github.com/Dicklesworthstone/wezterm_automata
- **Stars**: ~42
- **Tech**: Rust, WezTerm API, SQLite FTS5
- **Purpose**: Terminal hypervisor that captures pane output, detects agent state, enables automation.
- **Key features**:
  - Real-time terminal observation
  - Intelligent pattern detection
  - Robot Mode JSON API
  - Event-driven automation

**Botbox fit**: **INTERESTING FOR BOTTY**. Terminal observation for agents. Could complement vessel for detecting stuck agents, rate limits, etc. **Worth exploring** for agent health monitoring.

---

### 15. Brenner - Brenner Bot
- **GitHub**: https://github.com/Dicklesworthstone/brenner_bot
- **Stars**: ~28
- **Tech**: TypeScript, Bun, Agent Mail, Multi-model AI
- **Purpose**: Research orchestration platform inspired by Sydney Brenner's methodology.
- **Key features**:
  - Primary source corpus with citations
  - Multi-agent research sessions
  - Discriminative test ranking
  - Adversarial critique generation

**Botbox fit**: **LOW RELEVANCE**. Research orchestration is a different domain. Pass.

---

## Supporting Tools (7)

### 16. GIIL - Get Image from Internet Link
- **GitHub**: https://github.com/Dicklesworthstone/giil
- **Stars**: ~24
- **Purpose**: Download images from iCloud share links for remote debugging.
- **Botbox fit**: **LOW RELEVANCE**. Utility tool, not core workflow.

### 17. SRPS - System Resource Protection Script
- **GitHub**: https://github.com/Dicklesworthstone/system_resource_protection_script
- **Stars**: ~50
- **Purpose**: Auto-deprioritize background processes during heavy builds.
- **Botbox fit**: **LOW RELEVANCE**. System tool, not agent coordination.

### 18. XF - X Archive Search
- **GitHub**: https://github.com/Dicklesworthstone/xf
- **Stars**: ~156
- **Purpose**: Search over X/Twitter data archives.
- **Botbox fit**: **NOT RELEVANT**. Social media tool.

### 19. S2P - Source to Prompt TUI
- **GitHub**: https://github.com/Dicklesworthstone/source_to_prompt_tui
- **Stars**: ~78
- **Purpose**: Combine source files into LLM-ready prompts with token counting.
- **Botbox fit**: **LOW RELEVANCE**. Prompt crafting utility.

### 20. APR - Automated Plan Reviser Pro
- **GitHub**: https://github.com/Dicklesworthstone/automated_plan_reviser_pro
- **Stars**: ~85
- **Purpose**: Automated iterative specification refinement using extended AI reasoning.
- **Botbox fit**: **INTERESTING CONCEPT**. Plan refinement could feed into beads creation. Not core but interesting.

### 21. JFP - JeffreysPrompts CLI
- **URL**: https://jeffreysprompts.com
- **Stars**: ~120
- **Purpose**: Browse and install prompts as Claude Code skills.
- **Botbox fit**: **LOW RELEVANCE**. Prompt marketplace, not core workflow.

### 22. PT - Process Triage
- **GitHub**: https://github.com/Dicklesworthstone/process_triage
- **Stars**: ~45
- **Purpose**: Find and terminate stuck processes with Bayesian scoring.
- **Botbox fit**: **LOW RELEVANCE**. System utility.

---

## Summary: Tools Worth Exploring

### High Priority (directly relevant)
1. **BV (Beads Viewer)** - Same issue tracker, graph analysis for prioritization
2. **CASS (Session Search)** - Find past solutions, complements our workflow
3. **DCG (Destructive Command Guard)** - Safety for agent commands

### Medium Priority (interesting concepts)
4. **WA (WezTerm Automata)** - Terminal observation for agent health
5. **SLB (Simultaneous Launch Button)** - Two-person rule for dangerous ops
6. **CM (CASS Memory)** - Persistent agent learning
7. **NTM (Named Tmux Manager)** - Different approach to agent orchestration

### Low Priority (tangential)
- Mail (overlaps with botbus)
- ACFS (different scope - machine setup vs project setup)
- UBS (static analysis - we have oxlint)

### Not Relevant
- RU (git-focused, we use jj)
- MS, RCH, Brenner, GIIL, SRPS, XF, S2P, JFP, PT, APR

---

## Ecosystem Observations

1. **Shared Foundation**: They use the same `beads` (BR) issue tracker we do - this is the same ecosystem.

2. **MCP Integration**: Many tools expose MCP servers. We could consider MCP integration for botbus/vessel.

3. **Safety Focus**: Multiple tools (DCG, SLB) address agent safety - a real concern for autonomous agents.

4. **Session History**: CASS fills a gap we have - searchable agent session history. Worth investigating.

5. **Philosophy Overlap**: "The Flywheel" concept matches our vision - tools that reinforce each other.
