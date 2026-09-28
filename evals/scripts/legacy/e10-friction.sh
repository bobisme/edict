#!/usr/bin/env bash
set -euo pipefail

# E10 Friction Scoring Script
# Parses phase stdout logs and extracts automated friction metrics.
# Friction = wasted tool calls from tool failures, retries, --help lookups,
# sibling cancellations, and FALLBACK workarounds.
#
# Scoring (40 pts total, tiered):
#   40 = 0 wasted calls (zero friction)
#   30 = 1-5 wasted calls (minor friction)
#   20 = 6-15 wasted calls (moderate friction)
#   10 = 16-30 wasted calls (significant friction)
#    0 = 31+ wasted calls (severe friction)

source "${1:?Usage: e10-friction.sh <path-to-.eval-env>}"

ARTIFACTS="$EVAL_DIR/artifacts"

echo "=== E10 Friction Analysis ==="
echo "EVAL_DIR=$EVAL_DIR"
echo ""

# Counters
TOTAL_EXIT_FAILURES=0
TOTAL_SIBLING_CANCELLATIONS=0
TOTAL_HELP_LOOKUPS=0
TOTAL_FALLBACKS=0
TOTAL_RETRIES=0

# Per-phase results
declare -A PHASE_FAILURES
declare -A PHASE_SIBLINGS
declare -A PHASE_HELP
declare -A PHASE_FALLBACKS
declare -A PHASE_RETRIES

PHASES=(phase1 phase2 phase3 phase4 phase5 phase6 phase7 phase8)

for phase in "${PHASES[@]}"; do
  LOG="$ARTIFACTS/$phase.stdout.log"
  if [[ ! -f "$LOG" ]]; then
    echo "SKIP: $phase (no stdout log)"
    continue
  fi

  # Exit code failures: "Exit code 1" or "Exit code 2" from tool calls
  # These indicate the agent sent wrong flags/args to a tool
  EXIT_FAILS=$(grep -cP 'Exit code [12]\b' "$LOG" 2>/dev/null || true)

  # Sibling tool call cancellations: when one parallel tool call fails,
  # Claude Code cancels all sibling calls in the same batch
  SIBLINGS=$(grep -ci 'Sibling tool call errored' "$LOG" 2>/dev/null || true)

  # --help lookups mid-phase: agent didn't know the CLI and had to check
  HELP=$(grep -cP '\s--help\b' "$LOG" 2>/dev/null || true)

  # FALLBACK lines in orchestrator/script output
  FALLBACK=$(grep -ci 'FALLBACK' "$LOG" 2>/dev/null || true)

  # Command retries: same command base appearing multiple times with different flags
  # Heuristic: count lines where a tool command appears, then a nearly identical one
  # follows with added/changed flags. Simplified: count "error" → retry patterns.
  # We approximate by counting pairs of failed commands followed by the same command.
  RETRIES=0
  # Count sequences where an exit code failure is followed by the same command succeeding
  # This is a rough heuristic — manual review may be needed for precision
  if [[ "$EXIT_FAILS" -gt 0 ]]; then
    # Estimate: roughly 30% of exit failures trigger a retry with corrected flags
    RETRIES=$(( (EXIT_FAILS + 2) / 3 ))
  fi

  PHASE_FAILURES[$phase]=$EXIT_FAILS
  PHASE_SIBLINGS[$phase]=$SIBLINGS
  PHASE_HELP[$phase]=$HELP
  PHASE_FALLBACKS[$phase]=$FALLBACK
  PHASE_RETRIES[$phase]=$RETRIES

  TOTAL_EXIT_FAILURES=$((TOTAL_EXIT_FAILURES + EXIT_FAILS))
  TOTAL_SIBLING_CANCELLATIONS=$((TOTAL_SIBLING_CANCELLATIONS + SIBLINGS))
  TOTAL_HELP_LOOKUPS=$((TOTAL_HELP_LOOKUPS + HELP))
  TOTAL_FALLBACKS=$((TOTAL_FALLBACKS + FALLBACK))
  TOTAL_RETRIES=$((TOTAL_RETRIES + RETRIES))
done

TOTAL_WASTED=$((TOTAL_EXIT_FAILURES + TOTAL_SIBLING_CANCELLATIONS + TOTAL_HELP_LOOKUPS + TOTAL_RETRIES))

# --- Friction Score (tiered) ---
if [[ $TOTAL_WASTED -eq 0 ]]; then
  FRICTION_SCORE=40
elif [[ $TOTAL_WASTED -le 5 ]]; then
  FRICTION_SCORE=30
elif [[ $TOTAL_WASTED -le 15 ]]; then
  FRICTION_SCORE=20
elif [[ $TOTAL_WASTED -le 30 ]]; then
  FRICTION_SCORE=10
else
  FRICTION_SCORE=0
fi

# --- Per-phase Report ---
echo "--- Per-Phase Friction ---"
printf "%-10s %6s %8s %5s %8s %7s %6s\n" "Phase" "Exits" "Siblings" "Help" "Fallback" "Retries" "Total"
printf "%-10s %6s %8s %5s %8s %7s %6s\n" "-----" "-----" "--------" "----" "--------" "-------" "-----"

for phase in "${PHASES[@]}"; do
  exits=${PHASE_FAILURES[$phase]:-0}
  siblings=${PHASE_SIBLINGS[$phase]:-0}
  help=${PHASE_HELP[$phase]:-0}
  fallback=${PHASE_FALLBACKS[$phase]:-0}
  retries=${PHASE_RETRIES[$phase]:-0}
  total=$((exits + siblings + help + retries))
  clean="yes"
  if [[ $total -gt 0 ]]; then clean="NO"; fi
  printf "%-10s %6d %8d %5d %8d %7d %6d  %s\n" "$phase" "$exits" "$siblings" "$help" "$fallback" "$retries" "$total" "$clean"
done

echo ""

# --- Summary ---
echo "--- Friction Summary ---"
echo "Exit code failures:      $TOTAL_EXIT_FAILURES"
echo "Sibling cancellations:   $TOTAL_SIBLING_CANCELLATIONS"
echo "--help lookups:          $TOTAL_HELP_LOOKUPS"
echo "FALLBACK workarounds:    $TOTAL_FALLBACKS"
echo "Estimated retries:       $TOTAL_RETRIES"
echo "Total wasted calls:      $TOTAL_WASTED"
echo ""
echo "Friction score:          $FRICTION_SCORE / 40"

# Tier label
if [[ $FRICTION_SCORE -eq 40 ]]; then
  echo "Tier: ZERO FRICTION (perfect)"
elif [[ $FRICTION_SCORE -eq 30 ]]; then
  echo "Tier: MINOR FRICTION (1-5 wasted calls)"
elif [[ $FRICTION_SCORE -eq 20 ]]; then
  echo "Tier: MODERATE FRICTION (6-15 wasted calls)"
elif [[ $FRICTION_SCORE -eq 10 ]]; then
  echo "Tier: SIGNIFICANT FRICTION (16-30 wasted calls)"
else
  echo "Tier: SEVERE FRICTION (31+ wasted calls)"
fi

echo ""

# --- Clean phases ---
CLEAN_PHASES=0
for phase in "${PHASES[@]}"; do
  exits=${PHASE_FAILURES[$phase]:-0}
  siblings=${PHASE_SIBLINGS[$phase]:-0}
  help=${PHASE_HELP[$phase]:-0}
  retries=${PHASE_RETRIES[$phase]:-0}
  total=$((exits + siblings + help + retries))
  if [[ $total -eq 0 ]]; then
    CLEAN_PHASES=$((CLEAN_PHASES + 1))
  fi
done
echo "Clean phases: $CLEAN_PHASES / ${#PHASES[@]}"

# --- Top friction sources ---
echo ""
echo "--- Top Friction Sources ---"
if [[ $TOTAL_WASTED -eq 0 ]]; then
  echo "(none — all phases clean)"
else
  # Sort by total wasted per phase, descending
  for phase in "${PHASES[@]}"; do
    exits=${PHASE_FAILURES[$phase]:-0}
    siblings=${PHASE_SIBLINGS[$phase]:-0}
    help=${PHASE_HELP[$phase]:-0}
    retries=${PHASE_RETRIES[$phase]:-0}
    total=$((exits + siblings + help + retries))
    if [[ $total -gt 0 ]]; then
      echo "  $phase: $total wasted calls (${exits} exits, ${siblings} siblings, ${help} help, ${retries} retries)"
    fi
  done
fi

# --- Save structured output ---
{
  echo "friction_score=$FRICTION_SCORE"
  echo "total_wasted=$TOTAL_WASTED"
  echo "exit_failures=$TOTAL_EXIT_FAILURES"
  echo "sibling_cancellations=$TOTAL_SIBLING_CANCELLATIONS"
  echo "help_lookups=$TOTAL_HELP_LOOKUPS"
  echo "fallbacks=$TOTAL_FALLBACKS"
  echo "retries=$TOTAL_RETRIES"
  echo "clean_phases=$CLEAN_PHASES"
  echo "total_phases=${#PHASES[@]}"
  for phase in "${PHASES[@]}"; do
    exits=${PHASE_FAILURES[$phase]:-0}
    siblings=${PHASE_SIBLINGS[$phase]:-0}
    help=${PHASE_HELP[$phase]:-0}
    retries=${PHASE_RETRIES[$phase]:-0}
    total=$((exits + siblings + help + retries))
    echo "${phase}_total=$total"
  done
} > "$ARTIFACTS/friction-report.env"

echo ""
echo "Structured report saved to: $ARTIFACTS/friction-report.env"
echo ""
echo "=== Friction Analysis Complete ==="
