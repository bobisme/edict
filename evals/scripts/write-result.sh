#!/usr/bin/env bash
# Render a finished run as Markdown on stdout.
# Usage: write-result.sh <EVAL_DIR>
set -uo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
load_run "${1:?usage: write-result.sh <EVAL_DIR>}"
# shellcheck disable=SC1091
source "$EVAL_DIR/meta.env" 2>/dev/null || true
# shellcheck disable=SC1091
source "$EVAL_DIR/score.env" 2>/dev/null || true
# shellcheck disable=SC1091
source "$EVAL_DIR/safety.env" 2>/dev/null || true

R="$ART/result.json"
cost=$(jq -r 'if .total_cost_usd then (.total_cost_usd * 100 | round / 100 | tostring) else "n/a" end' "$R")
turns=$(jq -r '.num_turns // "n/a"' "$R")
subtype=$(jq -r '.subtype // "n/a"' "$R")
in_tok=$(jq -r '(.usage.input_tokens // 0) + (.usage.cache_read_input_tokens // 0) + (.usage.cache_creation_input_tokens // 0)' "$R")
out_tok=$(jq -r '.usage.output_tokens // "n/a"' "$R")
tools=$(wc -l <"$ART/tool-calls.jsonl" 2>/dev/null || echo 0)
bashn=$(jq -s '[.[] | select(.name=="Bash")] | length' "$ART/tool-calls.jsonl" 2>/dev/null || echo 0)
helpn=$(grep -cE -- '--help|\btldr\b| help( |$)' "$ART/bash-commands.txt" 2>/dev/null || true)
docreads=$(jq -r 'select(.name=="Read") | .input.file_path' "$ART/tool-calls.jsonl" 2>/dev/null | grep -c '\.agents/edict/' || true)
protos=$(grep -oE 'edict protocol [a-z]+' "$ART/bash-commands.txt" 2>/dev/null | sort | uniq -c | awk '{printf "%s×%s ", $4, $1}')
elapsed="n/a"
[[ -n "${START_EPOCH:-}" && -n "${END_EPOCH:-}" ]] && elapsed="$((END_EPOCH - START_EPOCH))s"
pct="n/a"
[[ -n "${TOTAL:-}" && "${TOTAL:-0}" -gt 0 ]] && pct="$((SCORE * 100 / TOTAL))%"

cat <<MD
# Eval: $SCENARIO / variant $VARIANT_LABEL / $EVAL_MODEL

| Field | Value |
|---|---|
| Date | $(date -d "@${START_EPOCH:-$(date +%s)}" -Iseconds) |
| Scenario | \`$SCENARIO\` (reviewer policy \`$REVIEWER_POLICY\`) |
| Entry | \`${EVAL_ENTRY:-?}\` |
| Model | \`${EVAL_MODEL:-?}\` |
| AGENTS.md variant | \`$VARIANT_LABEL\` (source \`$VARIANT_SOURCE\`, sha \`$VARIANT_SHA\`, $VARIANT_WORDS words, ~$VARIANT_TOKENS_APPROX tokens) |
| Score | **${SCORE:-?} / ${TOTAL:-?}** ($pct), ${FAILED:-?} failed checks |
| Agent exit | ${AGENT_EXIT:-?} (claude result: $subtype) |
| Wall time | $elapsed (timeout ${EVAL_TIMEOUT:-?}s) |
| Cost | \$$cost (budget cap \$${EVAL_BUDGET_USD:-?}) |
| Turns / tool calls / Bash | $turns / $tools / $bashn |
| Tokens in (incl. cache) / out | $in_tok / $out_tok |
| --help / tldr lookups | $helpn |
| Workflow doc reads (.agents/edict/) | $docreads |
| Protocol commands | ${protos:-none} |
| Live rite hooks before → after | ${LIVE_HOOKS_BEFORE:-?} → ${LIVE_HOOKS_AFTER:-?} |
| Real vessel agents for this run | ${LIVE_VESSEL_HITS:-?} |
| Live #projects last id unchanged | $([[ "${LIVE_PROJECTS_BEFORE:-a}" == "${LIVE_PROJECTS_AFTER:-b}" ]] && echo yes || echo NO) |
| Safety | **${SAFETY_OK:-?}** |
| Tool versions | $(tr '\n' ' ' <"$ART/tool-versions.txt") |

## Checks

| Id | Result | Pts | Check | Detail |
|---|---|---|---|---|
MD
awk -F'\t' '{d=$5; gsub(/\|/,"\\|",d); printf "| %s | %s | %s | %s | %s |\n", $1, toupper($3), $2, $4, d}' "$CHECKS"

cat <<MD

## Prompt

\`\`\`
$(cat "$ART/prompt.txt" 2>/dev/null)
\`\`\`

## Final agent message

\`\`\`
$(jq -r '.result // "(none)"' "$R" | head -40)
\`\`\`

## Reviewer ledger

\`\`\`
$(jq -c '{round, review_id, vote, agent_env, setup}' "$ART/reviewer-ledger.jsonl" 2>/dev/null)
\`\`\`

## Artifacts

Run dir: \`$EVAL_DIR\` (transcript: \`artifacts/transcript.jsonl\`, commands: \`artifacts/bash-commands.txt\`).
MD
