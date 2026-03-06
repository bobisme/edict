# Evaluation: cass_memory_system (cm)

**Recommendation: RECOMMENDED** — install cass for session search; cm playbook layer is optional but useful

**Date**: 2026-02-05
**Evaluator**: botbox-dev
**Beads**: bd-22c (evaluation), bd-3sh (hands-on testing)

---

## Executive Summary

Tested cass and cm hands-on. **The session search capability alone justifies installation.**

Indexed **1,620 conversations with 64,204 messages** across Claude Code, OpenCode, Gemini, and Codex. Searches for past solutions ("botbox sync migration", "jj divergent commit", "beads crash recovery") returned highly relevant results with full context.

The cm playbook layer (rules, effectiveness tracking, auto-extraction) works as advertised but requires more setup. Start with cass for immediate value.

---

## Hands-On Test Results

### What Got Indexed

| Agent | Conversations | Messages |
|-------|---------------|----------|
| Claude Code | 1,511 | ~50,678 |
| OpenCode | 102 | ~13,507 |
| Gemini | 4 | 13 |
| Codex | 3 | 9 |
| **Total** | **1,620** | **64,204** |

### Session Search Tests (cass)

**Test 1: "botbox sync migration"**
```bash
cass search "botbox sync migration" --robot --limit 5
```
Found: Full migration system design discussion from OpenCode session — reasoning, alternatives, implementation details. Score 54.5.

**Test 2: "jj divergent commit"**
```bash
cass search "jj divergent commit" --robot --limit 3
```
Found:
- Exact fix: `jj abandon <change-id>/0`
- CLAUDE.md documentation decisions
- Bead filing for maw doctor detection

**Test 3: "beads crash recovery"**
```bash
cass search "beads crash recovery" --robot --limit 3
```
Found:
- R9 eval design with scoring criteria
- CLAUDE.md bead tracking requirements update
- Full context of crash recovery architecture decisions

**Expand Command** — shows full conversation context around hits:
```bash
cass expand /path/to/session.jsonl -n 187 -C 3 --json
```
Returns surrounding messages with full reasoning, not just snippets.

### Playbook Tests (cm)

**Adding a rule:**
```bash
cm playbook add "Botrite hooks are managed through migrations, not direct sync logic" --category workflow
```
Rule created with ID `b-ml9k2jya-y88o42`, maturity "candidate".

**Context query includes the rule:**
```bash
cm context "add a new rite hook type" --json
```
Returns rule with relevance score 4, plus 10 history snippets from cass.

**Feedback updates score:**
```bash
cm mark b-ml9k2jya-y88o42 --helpful
```
Score increased from 0 to 0.5. Needs 3+ helpful marks for "established" maturity.

**Gap analysis:**
```bash
cm onboard status --json
```
Identified 9 "critical" categories (no rules) and 1 "underrepresented" (workflow).

---

## Installation

### cass (Session Search) — Install This

```bash
# One-liner install
curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/coding_agent_session_search/main/install.sh \
  | bash -s -- --easy-mode --verify

# Index all sessions (~15 seconds for 64K messages)
cass index --full --json

# Search
cass search "your query" --robot --limit 10

# Expand context around a hit
cass expand /path/to/session.jsonl -n <line> -C 5 --json
```

### cm (Playbook Layer) — Optional

```bash
# Install cm
curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/cass_memory_system/main/install.sh \
  | bash -s -- --easy-mode --verify

# Initialize
cm init

# Get context before a task
cm context "your task description" --json

# Add rules manually
cm playbook add "Your rule" --category debugging

# Mark rules helpful/harmful
cm mark <bullet-id> --helpful
cm mark <bullet-id> --harmful --reason "Why"
```

---

## Architecture

### cass (Episodic Memory)

Indexes session files from multiple agents:
- `~/.claude/projects/*/*.jsonl` — Claude Code
- `~/.local/share/opencode/` — OpenCode
- `~/.cursor/` — Cursor
- `~/.codex/` — Codex
- And more (Aider, Gemini, ChatGPT, etc.)

Provides:
- Full-text search with scoring
- Agent/workspace filtering
- Timeline views
- Context expansion

### cm (Procedural Memory)

Builds on cass to add:
- **Playbook** — rules with categories and maturity levels
- **Effectiveness tracking** — confidence decay (90-day half-life), 4x harmful multiplier
- **Auto-extraction** — LLM reflects on sessions to propose rules (requires API key)
- **Gap analysis** — identifies missing knowledge categories

```
EPISODIC (cass)     →  WORKING (diary)  →  PROCEDURAL (playbook)
Raw session logs       Session summaries    Distilled rules
64K messages           Structured insights  Scored, decaying
```

---

## Value Assessment

### High Value (cass alone)

| Use Case | Before | After |
|----------|--------|-------|
| "How did I solve X?" | Grep through jsonl files manually | `cass search "X"` with ranked results |
| "What was the reasoning?" | Can't access | `cass expand` shows full context |
| "Find similar problems" | Memory or nothing | Searchable 64K messages |

### Medium Value (cm playbook)

| Use Case | Before | After |
|----------|--------|-------|
| "What patterns work?" | Manual CLAUDE.md | Effectiveness-tracked rules |
| "What should I avoid?" | Memory | Anti-patterns from harmful feedback |
| "What knowledge am I missing?" | Unknown | Gap analysis by category |

### Tested: Local LLM via Ollama (Does Not Work)

Patched cm to support Ollama via baseURL config. Three files modified in `~/repos/cass_memory_system`:

- `src/types.ts` — added `baseURL` to ConfigSchema
- `src/llm.ts` — added `baseURL` to LLMConfig interface and provider creation
- `~/.cass-memory/config.json` — configured for Ollama

Config for Ollama:
```json
{
  "provider": "openai",
  "model": "glm-4.7-flash:latest",
  "baseURL": "http://localhost:11434/v1"
}
```

**Result**: Connection works, but structured output fails. cm uses Vercel AI SDK's `generateObject` which relies on tool/function calling. Local models (tested: gpt-oss, qwen3:32b, glm-4.7-flash) don't properly support this:

```
WARNING: [LLM] Schema validation failed: No object generated: the tool was not called.
```

**Conclusion**: Auto-extraction requires an API key for OpenAI/Anthropic/Google. The patch enables connection but local models lack the structured output capability cm requires. Manual rule management (`cm playbook add`) works fine without LLM.

**llama.cpp server attempt**: Tried to use llama-server directly (native grammar-based JSON schema), but:
- System llama.cpp-cuda package had version mismatch errors (`LLAMA_MODEL_META_KEY_SAMPLING_*` undefined)
- AUR package marked out-of-date
- Would require building from git to test properly

**Bottom line**: For auto-reflection with local models, either wait for better structured output support in local model tooling, or use API keys for occasional high-quality extraction.

### Not Tested

- **Long-term decay** — needs time to observe maturity progression
- **Cross-project rules** — tested single project only

---

## Recommendation

### Now: Install cass

Session search provides immediate value with minimal setup:

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/coding_agent_session_search/main/install.sh \
  | bash -s -- --easy-mode --verify

# Index (one-time, ~15s)
cass index --full

# Use
cass search "your problem" --robot --limit 10
```

### Optional: Add cm

If you want rule management and effectiveness tracking:

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/cass_memory_system/main/install.sh \
  | bash -s -- --easy-mode --verify

# Initialize
cm init

# Use before tasks
cm context "task description" --json
```

### Future: Consider Integration

If cass/cm prove consistently valuable:
- Add to `botbox doctor` as optional checks
- Add to workflow docs
- Consider Claude Code hook for automatic reflection

---

## Quick Reference

### cass Commands

| Command | Purpose |
|---------|---------|
| `cass index --full` | Index all sessions |
| `cass search "query" --robot` | Search with JSON output |
| `cass expand <file> -n <line> -C 5` | Show context around a hit |
| `cass stats --json` | Index statistics |
| `cass health` | Quick health check |

### cm Commands

| Command | Purpose |
|---------|---------|
| `cm context "task" --json` | Get rules + history for a task |
| `cm playbook list` | Show all rules |
| `cm playbook add "rule" --category X` | Add a rule |
| `cm mark <id> --helpful/--harmful` | Feedback on a rule |
| `cm stats --json` | Playbook health metrics |
| `cm onboard status` | Gap analysis |

---

## What This Doesn't Replace

- **beads** — tracks work (issues, status). cm tracks knowledge. Complementary.
- **CLAUDE.md** — manual rules still valuable. cm augments with auto-discovered rules.
- **rite** — coordination between agents. cm is knowledge management.

---

## Conclusion

**cass is worth installing now.** Having 64K messages searchable across 1,620 sessions is genuinely useful. The "how did I solve this before?" question is now answerable.

**cm is worth trying** if you want rule management with effectiveness tracking. The playbook system works, but requires more investment to populate with rules.

Start with cass. Add cm if you find yourself wanting structured rules on top of search.
