# UX Test

Test agent comprehension of the botbox workflow documentation.

## Purpose

Validate that AGENTS.md (and its linked workflow docs) provide sufficient, clear information for a new agent to understand and execute the botbox workflow without external guidance.

## Method

1. **Create test environment**:
   ```bash
   WORKDIR=$(mktemp -d)
   cd "$WORKDIR" && jj git init
   botbox init --name ux-test --type api --tools beads,maw,crit,rite,vessel --no-interactive
   ```

2. **Spawn subagent** in that directory:
   ```bash
   # Use Task tool or spawn via vessel/claude CLI
   # Set working directory to $WORKDIR
   ```

3. **Prime the agent**:
   > You are a coding agent who just started working on a project. You're currently in the directory [workdir]. First, read the AGENTS.md file to understand the project workflow.

4. **Ask non-leading questions**. Test core workflow concepts:
   - Identity management (how do you identify yourself?)
   - Work discovery (how do you find work to do?)
   - Cross-project coordination (what if you find a bug in a tool?)
   - Workflow sequencing (what's the difference between triage and start?)
   - Session lifecycle (when do you run sync?)
   - Code review (how do you review code?)
   - Tool ecosystem (what tools are available and what do they do?)
   - Lead agent discovery (who do you contact for issues?)

5. **Evaluate responses**:
   - ✅ **Correct**: Agent cites specific commands, workflows, or sections from docs
   - ⚠️ **Partial**: Agent has the right idea but lacks precision or completeness
   - ❌ **Incorrect**: Agent guesses, uses external knowledge, or misunderstands
   - ❓ **Gap**: Agent explicitly says "this isn't clear from the documentation"

6. **Write findings** in a report:
   - Summary (pass/fail, key strengths/weaknesses)
   - Question-by-question analysis
   - Recommendations for doc improvements

7. **Iterate**: Update docs based on gaps, re-test

## Example Questions

### Identity
- "How do you identify yourself when using tools like rite or crit?"
- Expected: `--agent <name>`, `rite generate-name`

### Work Discovery
- "If you want to find work to do, what steps should you take?"
- Expected: Check inbox, `br ready`, `bv --robot-next`, claim verification

### Cross-Project
- "If you encounter a bug in tool X, what should you do?"
- Expected: Query #projects, cd to repo, create beads, post with `-L feedback`

### Workflow Sequencing
- "What's the difference between triage and start?"
- Expected: Triage finds work, start claims and begins it

### Session Lifecycle
- "When should you run `br sync --flush-only`?"
- Expected: Session end

### Code Review
- "If you're asked to review code, what workflow should you follow?"
- Expected: review-loop.md steps (inbox → crit review → comment → lgtm/block)

### Tool Ecosystem
- "What are the main tools and what do they do?"
- Expected: Stack Reference table content

### Lead Agent Discovery
- "How do you know which agent to contact for a bug in a tool?"
- Expected: Query #projects registry, extract `lead:<agent>` field

## Success Criteria

- Agent answers ≥7/8 questions correctly (✅)
- No critical workflow gaps (❌ or ❓)
- Agent uses docs as primary source (not external knowledge)

## Notes

- Use a **fresh subagent** (no prior context about botbox)
- Ask questions in **random order** (avoid teaching sequence)
- **Don't lead** with hints or corrections during the test
- Keep questions **open-ended** (avoid yes/no)
- Focus on **workflow comprehension**, not command memorization

## Frequency

Run this test:
- After major doc updates
- Before releasing new botbox versions
- When adding new workflows or tools
- Quarterly as a baseline check
