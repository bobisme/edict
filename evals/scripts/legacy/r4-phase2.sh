#!/usr/bin/env bash
set -euo pipefail

# R4 Phase 2: Reviewer — Review the code
# Uses the proven R1 v2 prompt (clippy, web search, severity levels, evidence-grounding).
# Discovers review via crit inbox, comments, and votes (block or LGTM).

cd "${EVAL_DIR:?Set EVAL_DIR first (source .eval-env)}"
source .eval-env

echo "=== R4 Phase 2: Reviewer ==="
echo "REVIEWER=$REVIEWER"
echo "EVAL_DIR=$EVAL_DIR"

PROMPT="You are security reviewer agent \"${REVIEWER}\" for project \"r4-eval\".
Use --agent ${REVIEWER} on ALL crit and botbus commands.

Review workflow:
1. Check botbus inbox: botbus inbox --agent ${REVIEWER} --channels r4-eval --mark-read
2. Check crit inbox: crit inbox --agent ${REVIEWER}
3. For each pending review:
   a. Read the review and diff: crit review <id>, crit diff <id>
   b. Read the full source files changed in the diff
   c. Read Cargo.toml for edition and dependency versions
   d. Run static analysis: cargo clippy 2>&1 — cite any warnings in your comments
   e. If unsure about framework or library behavior, use web search to verify before commenting
4. For each issue found, comment with a severity level:
   - CRITICAL: Security vulnerabilities, data loss, crashes in production
   - HIGH: Correctness bugs, race conditions, resource leaks
   - MEDIUM: Error handling gaps, missing validation at boundaries
   - LOW: Code quality, naming, structure
   - INFO: Suggestions, style preferences, minor improvements
   Use: crit comment <id> \"SEVERITY: <feedback>\" --file <path> --line <line-or-range>
5. Vote:
   - crit block <id> --reason \"<reason>\" if any CRITICAL or HIGH issues exist
   - crit lgtm <id> if no CRITICAL or HIGH issues
6. Announce: botbus send --agent ${REVIEWER} r4-eval \"Review complete: <id>\" -L mesh -L review-done

Focus on security and correctness. Ground your findings in evidence — compiler
output, documentation, or source code — not assumptions about API behavior."

claude ${CLAUDE_MODEL:+--model "$CLAUDE_MODEL"} \
  --dangerously-skip-permissions --allow-dangerously-skip-permissions \
  -p "$PROMPT"

echo ""
echo "=== Phase 2 Complete ==="
echo "Check: was the review blocked or LGTMed?"
echo "If LGTM: skip Phase 3+4, go to Phase 5 (auto-award 25 pts)"
echo "If blocked: run Phase 3 next"
