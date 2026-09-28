#!/usr/bin/env bash
# Run one eval end to end: setup -> agent -> capture -> verify -> result.
#
#   evals/scripts/run.sh <worker|review-loop|lead-merge>
#
# Environment:
#   EDICT_AGENTS_VARIANT  current (default) | none | <file with managed block>
#   EVAL_MODEL            model for the agent under test (default: sonnet)
#   EVAL_ENTRY            claude (default: `claude -p` + terse prompt, only
#                         AGENTS.md carries the workflow) | worker-loop
#                         (`edict run worker-loop`, whose own prompt inlines
#                         the workflow; worker/review-loop only)
#   EVAL_TIMEOUT          seconds for the agent (default: 1500)
#   EVAL_BUDGET_USD       --max-budget-usd for claude -p (default: 6)
#   EVAL_PROBE            1 (default): first check with a cheap haiku call
#                         that Claude Code loads the eval project's AGENTS.md
#   EVAL_LABEL            results file label, e.g. smoke, managed-baseline
#                         (default: run)
#   EVAL_WRITE_RESULT     1 (default): write evals/results/<date>-<label>-*.md
#   EVAL_BASE             where run dirs go (default: /tmp/edict-evals)
#
# Exit: 0 run completed (whatever the score), 2 safety check failed,
#       1 harness error.
set -uo pipefail
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib/common.sh
source "$SCRIPT_DIR/lib/common.sh"

scenario="${1:?usage: run.sh <worker|review-loop|lead-merge>}"
[[ -x "$SCRIPT_DIR/$scenario-setup.sh" ]] || die "unknown scenario: $scenario"
EVAL_MODEL="${EVAL_MODEL:-sonnet}"
EVAL_ENTRY="${EVAL_ENTRY:-claude}"
EVAL_TIMEOUT="${EVAL_TIMEOUT:-1500}"
EVAL_BUDGET_USD="${EVAL_BUDGET_USD:-6}"
EVAL_LABEL="${EVAL_LABEL:-run}"
if [[ "$EVAL_ENTRY" == worker-loop && "$scenario" == lead-merge ]]; then
  die "EVAL_ENTRY=worker-loop does not apply to lead-merge"
fi

eval_require_tools
mkdir -p "$EVAL_BASE"
before=$(mktemp "$EVAL_BASE/.live-before-XXXXXX")
live_snapshot "$before"
log "live rite hooks before: $(grep -oP 'LIVE_HOOKS=\K.*' "$before")"

# Auth first: fail before spending setup time on an expired token.
auth_env $((EVAL_TIMEOUT + 300))

setup_out=$("$SCRIPT_DIR/$scenario-setup.sh" 2>&1) || {
  echo "$setup_out" >&2
  die "setup failed"
}
dir=$(grep -oP '^EVAL_DIR=\K.*' <<<"$setup_out" | tail -1)
load_run "$dir"
mv "$before" "$ART/live-before.env"
log "EVAL_DIR=$EVAL_DIR variant=$VARIANT_LABEL (~$VARIANT_TOKENS_APPROX tokens) model=$EVAL_MODEL entry=$EVAL_ENTRY"

if [[ "${EVAL_PROBE:-1}" == 1 ]]; then
  if probe_instructions; then
    log "probe: Claude Code loads $PROJECT_DIR/AGENTS.md"
  else
    sandbox_teardown
    live_compare "$ART/live-before.env" "$ART/live-after.env"
    die "probe: AGENTS.md is NOT loaded in the sandbox (see $ART/probe.json); an A/B would be meaningless"
  fi
fi

cat >"$EVAL_DIR/meta.env" <<EOF2
EVAL_MODEL=$EVAL_MODEL
EVAL_ENTRY=$EVAL_ENTRY
EVAL_TIMEOUT=$EVAL_TIMEOUT
EVAL_BUDGET_USD=$EVAL_BUDGET_USD
EVAL_LABEL=$EVAL_LABEL
AUTH_KIND=$AUTH_KIND
START_EPOCH=$(date +%s)
EOF2

log "running agent ($EVAL_ENTRY, timeout ${EVAL_TIMEOUT}s)..."
case "$EVAL_ENTRY" in
claude) run_claude "$AGENT_NAME" "$(cat "$EVAL_DIR/prompt.txt")" ;;
worker-loop) run_worker_loop "$AGENT_NAME" ;;
*) die "unknown EVAL_ENTRY: $EVAL_ENTRY" ;;
esac
echo "END_EPOCH=$(date +%s)" >>"$EVAL_DIR/meta.env"
echo "AGENT_EXIT=$(cat "$ART/agent-exit-code")" >>"$EVAL_DIR/meta.env"
log "agent exited ($(cat "$ART/agent-exit-code")); capturing artifacts"

sandbox_teardown
capture_artifacts
"$SCRIPT_DIR/$scenario-verify.sh" "$EVAL_DIR" | tee "$EVAL_DIR/verify.log"

safety=0
live_compare "$ART/live-before.env" "$ART/live-after.env" || safety=2
log "live rite hooks after: $(grep -oP 'LIVE_HOOKS_AFTER=\K.*' "$EVAL_DIR/safety.env") (safety ok: $(grep -oP 'SAFETY_OK=\K.*' "$EVAL_DIR/safety.env"))"

"$SCRIPT_DIR/write-result.sh" "$EVAL_DIR" >"$EVAL_DIR/result.md"
if [[ "${EVAL_WRITE_RESULT:-1}" == 1 ]]; then
  out="$EVAL_REPO_DIR/evals/results/$(date +%F)-$EVAL_LABEL-$scenario-$VARIANT_LABEL-$EVAL_MODEL-$(basename "$EVAL_DIR" | sed 's/.*-//').md"
  cp "$EVAL_DIR/result.md" "$out"
  log "result: $out"
fi
log "run dir kept for inspection: $EVAL_DIR"
exit $safety
