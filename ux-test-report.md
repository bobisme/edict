# UX Test Report: AGENTS.md Workflow Comprehension

**Date**: 2026-01-29
**Tester**: Claude (via subagent)
**Test Subject**: Fresh botbox-initialized repo (`/tmp/tmp.pLroWeHhze`)
**Method**: Spawn subagent, have them read AGENTS.md, answer 8 workflow questions

## Summary

**Result**: ✅ **Pass** — Subagent successfully understood all workflow concepts from AGENTS.md alone.

The agent correctly answered all 8 questions with specific, accurate responses including:
- Command syntax
- Workflow sequencing
- Cross-project feedback protocol
- Tool purposes and usage

No significant confusion or gaps were identified.

## Test Questions and Responses

### 1. Identity Management
**Question**: How do you identify yourself when using tools like rite or crit?

**Response**: ✅ Correct
- Cited `--agent <name>` requirement
- Showed `rite generate-name` usage
- Understood environment variable pattern

**Quality**: Excellent. Agent understood both generation and usage.

---

### 2. Finding Work
**Question**: If you want to find work to do, what steps should you take?

**Response**: ✅ Correct
- Cited Quick Start section (`rite generate-name`, `rite whoami`, `br ready`)
- Referenced triage.md for detailed flow
- Explained full sequence: inbox → create beads → ready → bv → claim check

**Quality**: Excellent. Agent showed multi-level understanding (quick start + detailed workflow).

---

### 3. Cross-Project Bug Reporting
**Question**: If you encounter a bug in "vessel" while working, what should you do?

**Response**: ✅ Correct
- Correctly identified report-issue.md workflow
- Showed all 5 steps with proper command syntax
- Understood #projects registry lookup
- Included `-L feedback` label and lead agent tagging

**Quality**: Excellent. Most complex workflow, answered perfectly.

---

### 4. Workflow Differentiation
**Question**: What's the difference between "triage" and "start" workflows?

**Response**: ✅ Correct
- Clear distinction: triage finds work, start claims it
- Correct input/output relationships
- Accurate description of each workflow's actions

**Quality**: Excellent. Showed clear conceptual understanding.

---

### 5. Session Protocol
**Question**: When should you run `br sync --flush-only`?

**Response**: ✅ Correct
- Cited "Beads Conventions" section
- Identified "session end" timing
- Also referenced CLAUDE.md session protocol checklist

**Quality**: Good. Agent found the answer in multiple places.

---

### 6. Code Review
**Question**: If you're asked to review code, what workflow should you follow?

**Response**: ✅ Correct
- Referenced review-loop.md
- Listed all 6 steps with command syntax
- Included quality guidance ("Be aggressive on security and correctness")

**Quality**: Excellent. Complete workflow with context.

---

### 7. Stack Overview
**Question**: What are the main tools in the stack and what do they do?

**Response**: ✅ Correct
- Reproduced entire Stack Reference table accurately
- Listed all 5 tools with purposes and key commands

**Quality**: Perfect. Verbatim from docs, formatted clearly.

---

### 8. Lead Agent Discovery
**Question**: How do you know which agent to contact if you find a bug in a tool?

**Response**: ✅ Correct
- Showed #projects registry query
- Explained message format parsing
- Cited default naming convention (`<project>-dev`)

**Quality**: Excellent. Connected to Q3 workflow.

---

## Strengths

1. **Self-contained documentation**: Agent answered all questions without external knowledge
2. **Command precision**: All bash examples were syntactically correct
3. **Cross-references**: Agent navigated between AGENTS.md managed section and detailed workflow docs
4. **Progressive detail**: Agent provided quick answers + deeper workflow context when appropriate
5. **Format parsing**: Agent understood structured message formats (#projects registry)

## Weaknesses

None identified. The agent demonstrated complete understanding of:
- Identity management
- Work triage
- Cross-project coordination
- Session lifecycle
- Tool ecosystem

## Recommendations

**No changes needed**. The documentation is clear, complete, and enables immediate productivity.

Optional enhancements for future consideration:
- Add a "Common Scenarios" section for quick lookup (e.g., "I'm stuck on a task, what do I do?")
- Consider a troubleshooting appendix for edge cases

## Conclusion

AGENTS.md successfully serves as a complete onboarding document for new agents. A fresh agent can read it and immediately understand:
- How to identify themselves
- How to find and start work
- How to report issues across projects
- How to participate in code review
- How to use the tool ecosystem

**Documentation quality**: Production-ready ✅
