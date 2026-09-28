#!/usr/bin/env bash
set -euo pipefail

# E12 Rust E2E Verification Script
# Automated scoring for the Rust binary end-to-end eval.
#
# Categories (100 pts total):
#   Init (25 pts)      — project structure, config, hooks, maw layout
#   Sync (15 pts)      — docs rendered, idempotent, --check mode
#   Doctor (15 pts)    — passes, --strict, JSON output
#   Status (10 pts)    — runs, JSON output
#   Hooks (10 pts)     — audit, install, hooks registered
#   Run (15 pts)       — all subcommands have --help
#   Resilience (10 pts) — re-init works, doctor passes after

source "${1:?Usage: e12-rust-e2e-verify.sh <path-to-.eval-env>}"

echo "=== E12 Rust E2E Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "RUST_BINARY=$RUST_BINARY"
echo ""

PASS=0
FAIL=0
WARN=0
SCORE=0
TOTAL=0
ARTIFACTS="$EVAL_DIR/artifacts"

check() {
  local label="$1"
  local result="$2"  # 0 = pass, 1 = fail
  local pts="${3:-0}"
  TOTAL=$((TOTAL + pts))
  if [[ "$result" -eq 0 ]]; then
    echo "PASS ($pts pts): $label"
    PASS=$((PASS + 1))
    SCORE=$((SCORE + pts))
  else
    echo "FAIL (0/$pts pts): $label"
    FAIL=$((FAIL + 1))
  fi
}

warn() {
  echo "WARN: $1"
  WARN=$((WARN + 1))
}

cd "$PROJECT_DIR"

# ============================================================
# Init (25 pts)
# ============================================================
echo "=== Init (25 pts) ==="
echo ""

# Check 1: Init succeeded (exit code 0) — 5 pts
echo "--- Check 1: Init exit code ---"
INIT_OUTPUT=$(cat "$ARTIFACTS/phase1-init.txt" 2>/dev/null || echo "")
INIT_OK=false
# If the file exists and doesn't contain "FATAL" or "Error:", it succeeded
if [[ -n "$INIT_OUTPUT" ]] && ! echo "$INIT_OUTPUT" | grep -qi "^FATAL\|^Error:.*failed\|panicked"; then
  INIT_OK=true
fi
check "botbox init succeeded" "$($INIT_OK && echo 0 || echo 1)" 5

# Check 2: .botbox.json created with correct fields — 5 pts
echo ""
echo "--- Check 2: .botbox.json valid ---"
CONFIG_OK=false
CONFIG_FILE="$PROJECT_DIR/.botbox.json"
if [[ -f "$CONFIG_FILE" ]]; then
  # Must have project.name, tools section, agents section
  HAS_NAME=$(jq -r '.project.name // empty' "$CONFIG_FILE" 2>/dev/null || echo "")
  HAS_TOOLS=$(jq -r '.tools | length' "$CONFIG_FILE" 2>/dev/null || echo "0")
  HAS_AGENTS=$(jq -r '.agents | length' "$CONFIG_FILE" 2>/dev/null || echo "0")
  if [[ "$HAS_NAME" == "testproj" ]] && [[ "$HAS_TOOLS" -gt 0 ]] && [[ "$HAS_AGENTS" -gt 0 ]]; then
    CONFIG_OK=true
  else
    warn "Config missing fields: name=$HAS_NAME tools=$HAS_TOOLS agents=$HAS_AGENTS"
  fi
else
  warn ".botbox.json not found at $CONFIG_FILE"
fi
check ".botbox.json created with name/tools/agents" "$($CONFIG_OK && echo 0 || echo 1)" 5

# Check 3: .agents/botbox/ directory created with docs — 5 pts
echo ""
echo "--- Check 3: .agents/botbox/ directory ---"
AGENTS_DIR_OK=false
# Could be at project root or ws/default/
AGENTS_DIR=""
if [[ -d "$PROJECT_DIR/.agents/botbox" ]]; then
  AGENTS_DIR="$PROJECT_DIR/.agents/botbox"
elif [[ -d "$PROJECT_DIR/ws/default/.agents/botbox" ]]; then
  AGENTS_DIR="$PROJECT_DIR/ws/default/.agents/botbox"
fi
if [[ -n "$AGENTS_DIR" ]]; then
  # Should have at least some .md files (workflow docs)
  MD_COUNT=$(find "$AGENTS_DIR" -name "*.md" 2>/dev/null | wc -l)
  if [[ "$MD_COUNT" -ge 3 ]]; then
    AGENTS_DIR_OK=true
  else
    warn ".agents/botbox/ has only $MD_COUNT .md files (expected 3+)"
  fi
else
  warn ".agents/botbox/ directory not found"
fi
check ".agents/botbox/ created with workflow docs ($MD_COUNT .md files)" "$($AGENTS_DIR_OK && echo 0 || echo 1)" 5

# Check 4: AGENTS.md exists and has managed section — 5 pts
echo ""
echo "--- Check 4: AGENTS.md with managed section ---"
AGENTSMD_OK=false
AGENTSMD_PATH=""
for p in "$PROJECT_DIR/AGENTS.md" "$PROJECT_DIR/ws/default/AGENTS.md"; do
  if [[ -f "$p" ]]; then
    AGENTSMD_PATH="$p"
    break
  fi
done
if [[ -n "$AGENTSMD_PATH" ]]; then
  if grep -q "botbox:managed-start" "$AGENTSMD_PATH" 2>/dev/null; then
    AGENTSMD_OK=true
  else
    warn "AGENTS.md missing botbox:managed-start marker"
  fi
fi
check "AGENTS.md exists with managed section" "$($AGENTSMD_OK && echo 0 || echo 1)" 5

# Check 5: Botbus hooks registered — 5 pts
echo ""
echo "--- Check 5: Botbus hooks registered ---"
HOOKS_OK=false
HOOKS_JSON=$(cat "$ARTIFACTS/phase1-hooks.json" 2>/dev/null || echo '{"hooks":[]}')
HOOK_COUNT=$(echo "$HOOKS_JSON" | jq '.hooks | length' 2>/dev/null || echo "0")
if [[ "$HOOK_COUNT" -ge 1 ]]; then
  HOOKS_OK=true
  echo "  $HOOK_COUNT hook(s) registered"
else
  warn "No botbus hooks registered after init"
fi
check "Botbus hooks registered ($HOOK_COUNT)" "$($HOOKS_OK && echo 0 || echo 1)" 5

# ============================================================
# Sync (15 pts)
# ============================================================
echo ""
echo "=== Sync (15 pts) ==="
echo ""

# Check 6: First sync succeeds — 5 pts
echo "--- Check 6: Sync succeeds ---"
SYNC_OK=false
SYNC_OUTPUT=$(cat "$ARTIFACTS/phase2-sync-first.txt" 2>/dev/null || echo "MISSING")
if [[ "$SYNC_OUTPUT" != "MISSING" ]] && ! echo "$SYNC_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  SYNC_OK=true
fi
check "botbox sync succeeds" "$($SYNC_OK && echo 0 || echo 1)" 5

# Check 7: sync --check exits 0 (up to date) — 5 pts
echo ""
echo "--- Check 7: sync --check exit code ---"
SYNC_CHECK_OK=false
SYNC_CHECK_OUTPUT=$(cat "$ARTIFACTS/phase2-sync-check.txt" 2>/dev/null || echo "MISSING")
# If the output doesn't contain error messages and doesn't indicate staleness
if [[ "$SYNC_CHECK_OUTPUT" != "MISSING" ]] && ! echo "$SYNC_CHECK_OUTPUT" | grep -qi "stale\|out.of.date\|Error:.*failed\|panicked"; then
  SYNC_CHECK_OK=true
fi
# Also check: file should not be missing
if [[ "$SYNC_CHECK_OUTPUT" == "MISSING" ]]; then
  SYNC_CHECK_OK=false
fi
check "sync --check reports up-to-date" "$($SYNC_CHECK_OK && echo 0 || echo 1)" 5

# Check 8: Second sync is idempotent — 5 pts
echo ""
echo "--- Check 8: Sync idempotent ---"
SYNC_IDEM_OK=false
SYNC2_OUTPUT=$(cat "$ARTIFACTS/phase2-sync-second.txt" 2>/dev/null || echo "MISSING")
if [[ "$SYNC2_OUTPUT" != "MISSING" ]] && ! echo "$SYNC2_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  SYNC_IDEM_OK=true
fi
check "Second sync is idempotent" "$($SYNC_IDEM_OK && echo 0 || echo 1)" 5

# ============================================================
# Doctor (15 pts)
# ============================================================
echo ""
echo "=== Doctor (15 pts) ==="
echo ""

# Check 9: Doctor passes — 5 pts
echo "--- Check 9: Doctor passes ---"
DOCTOR_OK=false
DOCTOR_OUTPUT=$(cat "$ARTIFACTS/phase3-doctor.txt" 2>/dev/null || echo "MISSING")
if [[ "$DOCTOR_OUTPUT" != "MISSING" ]] && ! echo "$DOCTOR_OUTPUT" | grep -qi "^Error:.*failed\|panicked\|CRITICAL"; then
  DOCTOR_OK=true
fi
check "botbox doctor passes" "$($DOCTOR_OK && echo 0 || echo 1)" 5

# Check 10: Doctor --strict passes — 5 pts
echo ""
echo "--- Check 10: Doctor --strict ---"
DOCTOR_STRICT_OK=false
DOCTOR_STRICT_OUTPUT=$(cat "$ARTIFACTS/phase3-doctor-strict.txt" 2>/dev/null || echo "MISSING")
if [[ "$DOCTOR_STRICT_OUTPUT" != "MISSING" ]] && ! echo "$DOCTOR_STRICT_OUTPUT" | grep -qi "^Error:.*failed\|panicked\|CRITICAL"; then
  DOCTOR_STRICT_OK=true
fi
check "botbox doctor --strict passes" "$($DOCTOR_STRICT_OK && echo 0 || echo 1)" 5

# Check 11: Doctor --format json is valid JSON — 5 pts
echo ""
echo "--- Check 11: Doctor JSON output ---"
DOCTOR_JSON_OK=false
DOCTOR_JSON=$(cat "$ARTIFACTS/phase3-doctor-json.txt" 2>/dev/null || echo "")
if [[ -n "$DOCTOR_JSON" ]] && echo "$DOCTOR_JSON" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null; then
  DOCTOR_JSON_OK=true
fi
check "doctor --format json produces valid JSON" "$($DOCTOR_JSON_OK && echo 0 || echo 1)" 5

# ============================================================
# Status (10 pts)
# ============================================================
echo ""
echo "=== Status (10 pts) ==="
echo ""

# Check 12: Status runs — 5 pts
echo "--- Check 12: Status runs ---"
STATUS_OK=false
STATUS_OUTPUT=$(cat "$ARTIFACTS/phase4-status.txt" 2>/dev/null || echo "MISSING")
if [[ "$STATUS_OUTPUT" != "MISSING" ]] && ! echo "$STATUS_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  STATUS_OK=true
fi
check "botbox status runs" "$($STATUS_OK && echo 0 || echo 1)" 5

# Check 13: Status --format json is valid JSON — 5 pts
echo ""
echo "--- Check 13: Status JSON output ---"
STATUS_JSON_OK=false
STATUS_JSON=$(cat "$ARTIFACTS/phase4-status-json.txt" 2>/dev/null || echo "")
if [[ -n "$STATUS_JSON" ]] && echo "$STATUS_JSON" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null; then
  STATUS_JSON_OK=true
fi
check "status --format json produces valid JSON" "$($STATUS_JSON_OK && echo 0 || echo 1)" 5

# ============================================================
# Hooks (10 pts)
# ============================================================
echo ""
echo "=== Hooks (10 pts) ==="
echo ""

# Check 14: hooks audit runs — 5 pts
echo "--- Check 14: Hooks audit ---"
AUDIT_OK=false
AUDIT_OUTPUT=$(cat "$ARTIFACTS/phase5-hooks-audit.txt" 2>/dev/null || echo "MISSING")
if [[ "$AUDIT_OUTPUT" != "MISSING" ]] && ! echo "$AUDIT_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  AUDIT_OK=true
fi
check "botbox hooks audit runs" "$($AUDIT_OK && echo 0 || echo 1)" 5

# Check 15: hooks install idempotent — 5 pts
echo ""
echo "--- Check 15: Hooks install idempotent ---"
INSTALL_OK=false
INSTALL_OUTPUT=$(cat "$ARTIFACTS/phase5-hooks-install.txt" 2>/dev/null || echo "MISSING")
if [[ "$INSTALL_OUTPUT" != "MISSING" ]] && ! echo "$INSTALL_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  INSTALL_OK=true
fi
# Also verify hooks are still present after install
HOOKS_AFTER=$(cat "$ARTIFACTS/phase5-hooks-after.json" 2>/dev/null || echo '{"hooks":[]}')
HOOKS_AFTER_COUNT=$(echo "$HOOKS_AFTER" | jq '.hooks | length' 2>/dev/null || echo "0")
if [[ "$HOOKS_AFTER_COUNT" -lt 1 ]]; then
  INSTALL_OK=false
  warn "Hooks disappeared after hooks install ($HOOKS_AFTER_COUNT remaining)"
fi
check "botbox hooks install is idempotent ($HOOKS_AFTER_COUNT hooks)" "$($INSTALL_OK && echo 0 || echo 1)" 5

# ============================================================
# Run Subcommands (15 pts)
# ============================================================
echo ""
echo "=== Run Subcommands (15 pts) ==="
echo ""

# Check 16: All 7 run subcommands have --help (15 pts total)
# Split: 3 pts for agent loops (dev, worker, reviewer), 2 pts each for others
SUBCMDS=(agent dev-loop worker-loop reviewer-loop responder triage iteration-start)
RUN_HELP_PASS=0
RUN_HELP_TOTAL=${#SUBCMDS[@]}

for subcmd in "${SUBCMDS[@]}"; do
  HELP_FILE="$ARTIFACTS/phase6-run-${subcmd}-help.txt"
  if [[ -f "$HELP_FILE" ]] && ! grep -qi "error\|panicked" "$HELP_FILE" 2>/dev/null; then
    RUN_HELP_PASS=$((RUN_HELP_PASS + 1))
  else
    warn "run $subcmd --help failed"
  fi
done

echo "--- Check 16: Run subcommands --help ---"
echo "  $RUN_HELP_PASS/$RUN_HELP_TOTAL subcommands have working --help"

# Award proportional points (15 pts max)
TOTAL=$((TOTAL + 15))
if [[ "$RUN_HELP_PASS" -eq "$RUN_HELP_TOTAL" ]]; then
  echo "PASS (15 pts): All run subcommands have --help"
  SCORE=$((SCORE + 15)); PASS=$((PASS + 1))
elif [[ "$RUN_HELP_PASS" -ge 5 ]]; then
  local_pts=$(( 15 * RUN_HELP_PASS / RUN_HELP_TOTAL ))
  echo "PARTIAL ($local_pts/15 pts): $RUN_HELP_PASS/$RUN_HELP_TOTAL subcommands pass"
  SCORE=$((SCORE + local_pts)); PASS=$((PASS + 1))
elif [[ "$RUN_HELP_PASS" -ge 1 ]]; then
  echo "PARTIAL (5/15 pts): Only $RUN_HELP_PASS/$RUN_HELP_TOTAL subcommands pass"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
else
  echo "FAIL (0/15 pts): No run subcommands have --help"
  FAIL=$((FAIL + 1))
fi

# ============================================================
# Resilience (10 pts)
# ============================================================
echo ""
echo "=== Resilience (10 pts) ==="
echo ""

# Check 17: Re-init with --force succeeds — 5 pts
echo "--- Check 17: Re-init --force ---"
REINIT_OK=false
REINIT_OUTPUT=$(cat "$ARTIFACTS/phase7-reinit.txt" 2>/dev/null || echo "MISSING")
if [[ "$REINIT_OUTPUT" != "MISSING" ]] && ! echo "$REINIT_OUTPUT" | grep -qi "^Error:.*failed\|panicked"; then
  REINIT_OK=true
fi
check "botbox init --force on existing project" "$($REINIT_OK && echo 0 || echo 1)" 5

# Check 18: Doctor passes after re-init — 5 pts
echo ""
echo "--- Check 18: Doctor after re-init ---"
POST_REINIT_OK=false
POST_REINIT_OUTPUT=$(cat "$ARTIFACTS/phase7-doctor-post-reinit.txt" 2>/dev/null || echo "MISSING")
if [[ "$POST_REINIT_OUTPUT" != "MISSING" ]] && ! echo "$POST_REINIT_OUTPUT" | grep -qi "^Error:.*failed\|panicked\|CRITICAL"; then
  POST_REINIT_OK=true
fi
check "Doctor passes after re-init" "$($POST_REINIT_OK && echo 0 || echo 1)" 5

# ============================================================
# Summary
# ============================================================
echo ""
echo "=== Verification Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "WARN: $WARN"
echo "SCORE: $SCORE / $TOTAL"
echo ""

if [[ "$FAIL" -eq 0 ]]; then
  echo "RESULT: ALL CHECKS PASSED ($SCORE/$TOTAL)"
elif [[ "$SCORE" -ge $(( TOTAL * 85 / 100 )) ]]; then
  echo "RESULT: EXCELLENT ($SCORE/$TOTAL) — $FAIL checks failed"
elif [[ "$SCORE" -ge $(( TOTAL * 70 / 100 )) ]]; then
  echo "RESULT: PASS ($SCORE/$TOTAL) — $FAIL checks failed"
else
  echo "RESULT: FAIL ($SCORE/$TOTAL) — $FAIL checks failed"
fi

echo ""
echo "=== Forensics ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "RUST_BINARY=$RUST_BINARY"
echo ""
echo "Key artifacts:"
echo "  cat $ARTIFACTS/phase1-init.txt       # init output"
echo "  cat $ARTIFACTS/phase1-config.json    # .botbox.json"
echo "  cat $ARTIFACTS/phase3-doctor-strict.txt  # doctor --strict"
echo "  cat $ARTIFACTS/phase4-status-json.txt    # status JSON"
echo "  ls $ARTIFACTS/"
echo ""
echo "=== Verification Complete ==="
