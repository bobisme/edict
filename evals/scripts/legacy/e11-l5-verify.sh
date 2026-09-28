#!/usr/bin/env bash
set -euo pipefail

# E11-L5v2 Verification Script
# Automated scoring for the coordination mission eval.
# Checks all L4 categories (mission recognition, decomposition, worker dispatch,
# monitoring, synthesis, code correctness, friction) PLUS REDESIGNED coordination
# checks (~40 pts): multi-worker Record contributions, bidirectional bus communication,
# co-evolving shared types, cross-stage field discovery.
#
# KEY DIFFERENCE FROM L5v1:
# Coordination checks verify that MULTIPLE workers independently modified record.rs
# and pipeline.rs. The old taskr eval gave coordination credit for implicit compilation
# success — this eval requires evidence of bidirectional coordination because the
# Record struct must have fields from ALL three stages.

source "${1:?Usage: e11-l5-verify.sh <path-to-.eval-env>}"

echo "=== E11-L5v2 Verification ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "FLOWLOG_DEV=$FLOWLOG_DEV"
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

# Load artifacts
DEV_LOG=$(cat "$ARTIFACTS/agent-${FLOWLOG_DEV}.log" 2>/dev/null || echo "")
CHANNEL_HISTORY=$(cat "$ARTIFACTS/channel-flowlog-history.log" 2>/dev/null || echo "")
FINAL_STATUS=$(cat "$ARTIFACTS/final-status.txt" 2>/dev/null || echo "")
CHANNEL_JSON=$(cat "$ARTIFACTS/channel-flowlog-history.json" 2>/dev/null || echo '{"messages":[]}')
CHANNEL_LABELS=$(echo "$CHANNEL_JSON" | jq -r '[.messages[].labels // [] | .[]] | .[]' 2>/dev/null || echo "")
COORD_MESSAGES=$(cat "$ARTIFACTS/coord-messages.log" 2>/dev/null || echo "")
RECORD_FINAL=$(cat "$ARTIFACTS/record-final.rs" 2>/dev/null || echo "")
PIPELINE_FINAL=$(cat "$ARTIFACTS/pipeline-final.rs" 2>/dev/null || echo "")

# Load all worker logs into a combined variable for coordination checks
ALL_WORKER_LOGS=""
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  ALL_WORKER_LOGS+=$(cat "$wlog" 2>/dev/null || echo "")
  ALL_WORKER_LOGS+=$'\n'
done

# Extract mission bone from final status
MISSION_BONE=$(echo "$FINAL_STATUS" | grep -oP 'MISSION_BONE=\K[^ ]+' || echo "none")
CHILD_COUNT_FINAL=$(echo "$FINAL_STATUS" | grep -oP 'CHILD_COUNT=\K\d+' || echo "0")
CHILDREN_CLOSED_FINAL=$(echo "$FINAL_STATUS" | grep -oP 'CHILDREN_CLOSED=\K\d+' || echo "0")

cd "$PROJECT_DIR"

# ============================================================
# Critical Fail: Mission never created
# ============================================================
echo "=== Critical Fail Check ==="
echo ""
if [[ "$MISSION_BONE" == "none" || -z "$MISSION_BONE" ]]; then
  echo "CRITICAL FAIL: Mission bone was never created"
  echo ""
  echo "SCORE: 0 / 0 (critical fail)"
  echo "RESULT: CRITICAL FAIL — mission never created"
  exit 0
fi
echo "Mission bone: $MISSION_BONE"
echo ""

# ============================================================
# Mission Recognition (15 pts)
# ============================================================
echo "=== Mission Recognition (15 pts) ==="
echo ""

# Check 1: Bead with mission label (5 pts)
echo "--- Check 1: Mission bone with label ---"
MISSION_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$MISSION_BONE" --format json 2>/dev/null || echo "[]")
HAS_MISSION_LABEL=false
if echo "$MISSION_JSON" | jq -r '.[0].labels // [] | .[]' 2>/dev/null | grep -q "mission"; then
  HAS_MISSION_LABEL=true
fi
check "Mission bone has 'mission' label" "$($HAS_MISSION_LABEL && echo 0 || echo 1)" 5

# Check 2: Structured description (5 pts)
echo ""
echo "--- Check 2: Structured description ---"
MISSION_DESC=$(echo "$MISSION_JSON" | jq -r '.[0].description // ""' 2>/dev/null || echo "")
HAS_OUTCOME=false
if echo "$MISSION_DESC" | grep -qi "outcome\|success.*metric\|constraints\|stop.*crit"; then
  HAS_OUTCOME=true
fi
check "Mission bone has structured description (Outcome/Success/Constraints)" "$($HAS_OUTCOME && echo 0 || echo 1)" 5

# Check 3: Dev-loop identified mission context (5 pts)
echo ""
echo "--- Check 3: Dev-loop identified mission ---"
DEV_MISSION_CTX=false
if echo "$DEV_LOG" | grep -qi "BOTBOX_MISSION\|mission.*${MISSION_BONE}\|Level 4\|mission.*decompos"; then
  DEV_MISSION_CTX=true
fi
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*${MISSION_BONE}\|mission.*creat"; then
  DEV_MISSION_CTX=true
fi
check "Dev-loop identified mission context" "$($DEV_MISSION_CTX && echo 0 || echo 1)" 5

# ============================================================
# Decomposition (25 pts)
# ============================================================
echo ""
echo "=== Decomposition (25 pts) ==="
echo ""

# Get children
CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_BONE" --format json 2>/dev/null || echo '[]')
# Normalize JSON shape
ACTUAL_CHILD_COUNT=$(echo "$CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")

# Check 4: 3+ children (5 pts)
echo "--- Check 4: Child bone count ---"
check "3+ child bones created (actual=$ACTUAL_CHILD_COUNT)" "$([ "$ACTUAL_CHILD_COUNT" -ge 3 ] && echo 0 || echo 1)" 5

# Check 5: mission:<id> labels (5 pts)
echo ""
echo "--- Check 5: Mission labels on children ---"
LABELED_COUNT=0
CHILD_IDS=$(echo "$CHILDREN_JSON" | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null || echo "")
for cid in $CHILD_IDS; do
  CHILD_LABELS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].labels // [] | .[]' 2>/dev/null || echo "")
  if echo "$CHILD_LABELS" | grep -q "mission:"; then
    LABELED_COUNT=$((LABELED_COUNT + 1))
  fi
done
check "Children have mission:<id> labels ($LABELED_COUNT/$ACTUAL_CHILD_COUNT)" "$([ "$LABELED_COUNT" -ge 3 ] && echo 0 || echo 1)" 5

# Check 6: Parent dependencies (5 pts)
echo ""
echo "--- Check 6: Parent dependencies ---"
HAS_PARENT_DEP=false
if echo "$DEV_LOG" | grep -qi "bn triage dep add\|dep.*add"; then
  HAS_PARENT_DEP=true
fi
for cid in $CHILD_IDS; do
  CHILD_DEPS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].dependencies // [] | length' 2>/dev/null || echo "0")
  if [[ "$CHILD_DEPS" -gt 0 ]]; then
    HAS_PARENT_DEP=true
    break
  fi
done
check "Dependencies wired between children or to parent" "$($HAS_PARENT_DEP && echo 0 || echo 1)" 5

# Check 7: Inter-child dependency (5 pts)
echo ""
echo "--- Check 7: Inter-child dependency ---"
HAS_INTER_DEP=false
DEP_ADD_COUNT=$(echo "$DEV_LOG" | grep -ci "bn triage dep add" 2>/dev/null) || DEP_ADD_COUNT=0
if [[ "$DEP_ADD_COUNT" -ge 1 ]]; then
  HAS_INTER_DEP=true
fi
check "Inter-child dependency exists (dep add count=$DEP_ADD_COUNT)" "$($HAS_INTER_DEP && echo 0 || echo 1)" 5

# Check 8: Clear titles (5 pts)
echo ""
echo "--- Check 8: Clear child titles ---"
CLEAR_TITLES=false
if [[ "$ACTUAL_CHILD_COUNT" -gt 0 ]]; then
  CLEAR_TITLES=true
  for cid in $CHILD_IDS; do
    CHILD_TITLE=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$cid" --format json 2>/dev/null | jq -r '.[0].title // ""' 2>/dev/null || echo "")
    if [[ ${#CHILD_TITLE} -lt 5 ]]; then
      CLEAR_TITLES=false
      warn "Child $cid has unclear title: '$CHILD_TITLE'"
    fi
  done
fi
check "Child bones have clear titles" "$($CLEAR_TITLES && echo 0 || echo 1)" 5

# ============================================================
# Worker Dispatch (25 pts)
# ============================================================
echo ""
echo "=== Worker Dispatch (25 pts) ==="
echo ""

# Extract worker info from final status
WORKER_COUNT=$(echo "$FINAL_STATUS" | grep -oP 'WORKER_COUNT=\K\d+' || echo "0")
WORKER_NAMES=$(echo "$FINAL_STATUS" | sed -n '/^WORKER_NAMES_START$/,/^WORKER_NAMES_END$/p' | grep -v '^WORKER_NAMES_' | grep -v '^$' || echo "")

# Check 9: Workers spawned (5 pts)
echo "--- Check 9: Workers spawned ---"
WORKERS_SPAWNED=false
if [[ "$WORKER_COUNT" -ge 1 ]]; then
  WORKERS_SPAWNED=true
fi
if echo "$DEV_LOG" | grep -i "vessel spawn" | grep -q "/"; then
  WORKERS_SPAWNED=true
fi
check "Workers spawned ($WORKER_COUNT discovered)" "$($WORKERS_SPAWNED && echo 0 || echo 1)" 5

# Critical fail: no workers → cap score at 30%
if ! $WORKERS_SPAWNED; then
  echo ""
  echo "CRITICAL: No workers spawned — capping total score at 30%"
  echo ""
fi

# Check 10: 2+ workers (5 pts)
echo ""
echo "--- Check 10: Multiple workers ---"
check "2+ workers spawned ($WORKER_COUNT)" "$([ "$WORKER_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 11: Workspace per worker (5 pts)
echo ""
echo "--- Check 11: Workspace per worker ---"
WS_CREATE_COUNT=$(echo "$DEV_LOG" | grep -ci "maw ws create" 2>/dev/null) || WS_CREATE_COUNT=0
WORKER_WS=false
if [[ "$WS_CREATE_COUNT" -ge 2 ]]; then
  WORKER_WS=true
fi
check "Workspaces created for workers ($WS_CREATE_COUNT ws creates)" "$($WORKER_WS && echo 0 || echo 1)" 5

# Check 12: Mission env vars (5 pts)
echo ""
echo "--- Check 12: Mission env vars ---"
HAS_MISSION_ENV=false
if echo "$DEV_LOG" | grep -qi "BOTBOX_MISSION\|BOTBOX_SIBLINGS\|BOTBOX_MISSION_OUTCOME"; then
  HAS_MISSION_ENV=true
fi
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*context\|mission.*bn-"; then
  HAS_MISSION_ENV=true
fi
for cid in $CHILD_IDS; do
  CHILD_COMMENTS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn bone comment list "$cid" 2>/dev/null || echo "")
  if echo "$CHILD_COMMENTS" | grep -qi "mission.*context\|BOTBOX_MISSION"; then
    HAS_MISSION_ENV=true
    break
  fi
done
check "Mission env vars set on workers (BOTBOX_MISSION, etc.)" "$($HAS_MISSION_ENV && echo 0 || echo 1)" 5

# Check 13: Claims staked for workers (5 pts)
echo ""
echo "--- Check 13: Claims staked for workers ---"
CLAIM_COUNT=$(echo "$DEV_LOG" | grep -ci "bus claims stake" 2>/dev/null) || CLAIM_COUNT=0
HAS_WORKER_CLAIMS=false
if [[ "$CLAIM_COUNT" -ge 3 ]]; then
  HAS_WORKER_CLAIMS=true
fi
check "Claims staked for workers ($CLAIM_COUNT total stakes)" "$($HAS_WORKER_CLAIMS && echo 0 || echo 1)" 5

# ============================================================
# Monitoring (15 pts)
# ============================================================
echo ""
echo "=== Monitoring (15 pts) ==="
echo ""

# Check 14: Checkpoint message (5 pts)
echo "--- Check 14: Checkpoint message ---"
HAS_CHECKPOINT=false
if echo "$CHANNEL_HISTORY" | grep -qi "checkpoint\|mission.*done\|active\|progress.*mission"; then
  HAS_CHECKPOINT=true
fi
if echo "$DEV_LOG" | grep -qi "checkpoint"; then
  HAS_CHECKPOINT=true
fi
check "Checkpoint message posted" "$($HAS_CHECKPOINT && echo 0 || echo 1)" 5

# Check 15: Count/status info (5 pts)
echo ""
echo "--- Check 15: Count/status info ---"
HAS_COUNT_INFO=false
if echo "$CHANNEL_HISTORY" | grep -qiE "[0-9]+.*done|[0-9]+.*closed|[0-9]+.*active|[0-9]+/[0-9]+"; then
  HAS_COUNT_INFO=true
fi
if echo "$DEV_LOG" | grep -qiE "children.*closed|children.*status|[0-9]+.*done.*[0-9]+.*total"; then
  HAS_COUNT_INFO=true
fi
check "Count/status info in checkpoint" "$($HAS_COUNT_INFO && echo 0 || echo 1)" 5

# Check 16: Worker completion detected (5 pts)
echo ""
echo "--- Check 16: Worker completion detected ---"
COMPLETION_DETECTED=false
if echo "$DEV_LOG" | grep -qi "worker.*finish\|worker.*exit\|child.*closed\|maw ws merge.*--destroy"; then
  MERGE_COUNT=$(echo "$DEV_LOG" | grep -ci "maw ws merge" 2>/dev/null) || MERGE_COUNT=0
  if [[ "$MERGE_COUNT" -ge 2 ]]; then
    COMPLETION_DETECTED=true
  fi
fi
if echo "$DEV_LOG" | grep -qi "worker.*complet\|child.*done"; then
  COMPLETION_DETECTED=true
fi
check "Worker completion detected" "$($COMPLETION_DETECTED && echo 0 || echo 1)" 5

# ============================================================
# Synthesis (15 pts)
# ============================================================
echo ""
echo "=== Synthesis (15 pts) ==="
echo ""

# Check 17: All children closed (5 pts)
echo "--- Check 17: All children closed ---"
ALL_CHILDREN_CLOSED=false
CLOSED_COUNT=0
if [[ "$ACTUAL_CHILD_COUNT" -gt 0 ]]; then
  CLOSED_COUNT=$(echo "$CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")
  if [[ "$CLOSED_COUNT" -ge "$ACTUAL_CHILD_COUNT" ]]; then
    ALL_CHILDREN_CLOSED=true
  fi
  echo "  Children closed: $CLOSED_COUNT/$ACTUAL_CHILD_COUNT"
fi
check "All children closed ($CLOSED_COUNT/$ACTUAL_CHILD_COUNT)" "$($ALL_CHILDREN_CLOSED && echo 0 || echo 1)" 5

# Check 18: Mission bone closed (5 pts)
echo ""
echo "--- Check 18: Mission bone closed ---"
MISSION_STATUS=$(echo "$MISSION_JSON" | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
MISSION_CLOSED=false
[[ "$MISSION_STATUS" == "done" ]] && MISSION_CLOSED=true
check "Mission bone closed (status=$MISSION_STATUS)" "$($MISSION_CLOSED && echo 0 || echo 1)" 5

# Check 19: Synthesis comment (5 pts)
echo ""
echo "--- Check 19: Synthesis comment ---"
HAS_SYNTHESIS=false
MISSION_COMMENTS=$(echo "$MISSION_JSON" | jq -r '.[0].comments // [] | .[].body // .[].content // empty' 2>/dev/null || echo "")
if echo "$MISSION_COMMENTS" | grep -qi "mission.*complete\|synthesis\|children.*closed\|all.*done\|key.*decision\|what.*worked"; then
  HAS_SYNTHESIS=true
fi
if echo "$CHANNEL_HISTORY" | grep -qi "mission.*complete\|all.*children.*done"; then
  HAS_SYNTHESIS=true
fi
check "Synthesis comment on mission bone" "$($HAS_SYNTHESIS && echo 0 || echo 1)" 5

# ============================================================
# Code Correctness (20 pts)
# ============================================================
echo ""
echo "=== Code Correctness (20 pts) ==="
echo ""

# Check 20: cargo check (5 pts)
echo "--- Check 20: cargo check ---"
CARGO_OK=false
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo check 2>/dev/null; then
  CARGO_OK=true
fi
check "cargo check passes" "$($CARGO_OK && echo 0 || echo 1)" 5

# Check 21: 2+ subcommands implemented (5 pts)
echo ""
echo "--- Check 21: Subcommands implemented ---"
IMPL_COUNT=0
for mod_name in ingest transform emit; do
  MOD_FILE="$PROJECT_DIR/ws/default/src/commands/${mod_name}.rs"
  if [[ -f "$MOD_FILE" ]] && ! grep -q 'todo!' "$MOD_FILE" 2>/dev/null; then
    IMPL_COUNT=$((IMPL_COUNT + 1))
  fi
done
check "2+ subcommands implemented (found=$IMPL_COUNT)" "$([ "$IMPL_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 22: Shared types implemented (5 pts)
echo ""
echo "--- Check 22: Shared types implemented ---"
RECORD_RS_FILE="$PROJECT_DIR/ws/default/src/record.rs"
PIPELINE_RS_FILE="$PROJECT_DIR/ws/default/src/pipeline.rs"
SHARED_IMPL_COUNT=0
# Check record.rs: no todo!() at module level, has more than starter fields
if [[ -f "$RECORD_RS_FILE" ]]; then
  RECORD_TODOS=$(grep -c 'todo!' "$RECORD_RS_FILE" 2>/dev/null) || RECORD_TODOS=0
  if [[ "$RECORD_TODOS" -eq 0 ]]; then
    SHARED_IMPL_COUNT=$((SHARED_IMPL_COUNT + 1))
  fi
fi
# Check pipeline.rs: no todo!() at module level, has PipelineStage impls
if [[ -f "$PIPELINE_RS_FILE" ]]; then
  PIPELINE_TODOS=$(grep -c 'todo!' "$PIPELINE_RS_FILE" 2>/dev/null) || PIPELINE_TODOS=0
  if [[ "$PIPELINE_TODOS" -eq 0 ]]; then
    SHARED_IMPL_COUNT=$((SHARED_IMPL_COUNT + 1))
  fi
fi
check "Shared types implemented ($SHARED_IMPL_COUNT/2: record.rs + pipeline.rs)" "$([ "$SHARED_IMPL_COUNT" -ge 1 ] && echo 0 || echo 1)" 5

# Check 23: Subcommands work on sample data (5 pts)
echo ""
echo "--- Check 23: Subcommands work on sample data ---"
WORKING_COUNT=0
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo build --quiet 2>/dev/null; then
  # Try ingest on sample.csv
  INGEST_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- ingest data/sample.csv 2>/dev/null || echo "")
  if echo "$INGEST_OUT" | grep -qiE "record|ingest|Alice|5.*record"; then
    WORKING_COUNT=$((WORKING_COUNT + 1))
  fi
  # Try ingest --json on sample.csv
  INGEST_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- ingest data/sample.csv --json 2>/dev/null || echo "")
  if echo "$INGEST_JSON" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null || \
     echo "$INGEST_JSON" | head -1 | python3 -c "import sys,json;json.loads(sys.stdin.read())" 2>/dev/null; then
    WORKING_COUNT=$((WORKING_COUNT + 1))
  fi
  # Try transform (requires ingested records as input, so pipe or use temp file)
  if [[ -n "$INGEST_JSON" ]]; then
    echo "$INGEST_JSON" > /tmp/flowlog-test-records.json 2>/dev/null || true
    TRANSFORM_OUT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- transform data/rules.json --input /tmp/flowlog-test-records.json 2>/dev/null || echo "")
    if echo "$TRANSFORM_OUT" | grep -qiE "transform|record|rule|validation|error"; then
      WORKING_COUNT=$((WORKING_COUNT + 1))
    fi
  fi
fi
check "Subcommands working on sample data ($WORKING_COUNT/3)" "$([ "$WORKING_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# Check 23b: Feature flags work (5 pts) — bonus
echo ""
echo "--- Check 23b: Feature flags work ---"
FLAGS_WORKING=0
if BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo build --quiet 2>/dev/null; then
  # ingest --json produces valid JSON
  IJ=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- ingest data/sample.json --json 2>/dev/null || echo "")
  if echo "$IJ" | python3 -c "import sys,json;json.load(sys.stdin)" 2>/dev/null || \
     echo "$IJ" | head -1 | python3 -c "import sys,json;json.loads(sys.stdin.read())" 2>/dev/null; then
    FLAGS_WORKING=$((FLAGS_WORKING + 1))
  fi
  # ingest --format csv (explicit format override)
  IC=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- ingest data/sample.csv --format csv 2>/dev/null || echo "")
  if echo "$IC" | grep -qiE "record|ingest|Alice"; then
    FLAGS_WORKING=$((FLAGS_WORKING + 1))
  fi
  # transform --strict on strict rules should reject some records
  if [[ -f /tmp/flowlog-test-records.json ]]; then
    TS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- transform data/strict-rules.json --input /tmp/flowlog-test-records.json --strict 2>/dev/null || echo "")
    if echo "$TS" | grep -qiE "reject|error|fail|strict|validation"; then
      FLAGS_WORKING=$((FLAGS_WORKING + 1))
    fi
  fi
  # emit --format summary
  if [[ -f /tmp/flowlog-test-records.json ]]; then
    ES=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- cargo run --quiet -- emit /tmp/flowlog-test-output.json --input /tmp/flowlog-test-records.json --format summary 2>/dev/null || echo "")
    if echo "$ES" | grep -qiE "record|count|source|summary"; then
      FLAGS_WORKING=$((FLAGS_WORKING + 1))
    fi
  fi
fi
TOTAL=$((TOTAL + 5))
if [[ "$FLAGS_WORKING" -ge 3 ]]; then
  echo "PASS (5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$FLAGS_WORKING" -ge 2 ]]; then
  echo "PARTIAL (3/5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$FLAGS_WORKING" -ge 1 ]]; then
  echo "PARTIAL (1/5 pts): $FLAGS_WORKING/4 feature flags work"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): No feature flags work ($FLAGS_WORKING/4)"
  FAIL=$((FAIL + 1))
fi

# ============================================================
# Coordination (40 pts) — REDESIGNED for L5v2
# ============================================================
echo ""
echo "=== Coordination (40 pts) — L5v2 CO-EVOLUTION ==="
echo ""

# --- Record Co-Evolution (15 pts) ---
# These checks verify that MULTIPLE workers contributed fields to Record.
# This is the core test: can workers co-evolve a shared type?

# Check 24: Record has fields beyond starter (5 pts)
echo "--- Check 24: Record has fields beyond starter ---"
RECORD_FIELD_COUNT=0
if [[ -f "$RECORD_RS_FILE" ]]; then
  # Count pub fields in Record struct (beyond id and data)
  RECORD_FIELD_COUNT=$(grep -cE '^\s+pub\s+\w+' "$RECORD_RS_FILE" 2>/dev/null) || RECORD_FIELD_COUNT=0
fi
echo "  Record has $RECORD_FIELD_COUNT pub fields (starter=2)"
RECORD_EXTENDED=false
if [[ "$RECORD_FIELD_COUNT" -ge 4 ]]; then
  RECORD_EXTENDED=true
fi
check "Record has 4+ fields (workers added to starter struct)" "$($RECORD_EXTENDED && echo 0 || echo 1)" 5

# Check 25: Record has fields from 2+ stages (5 pts)
echo ""
echo "--- Check 25: Record has fields from 2+ stages ---"
STAGE_FIELD_MATCHES=0
if [[ -f "$RECORD_RS_FILE" ]]; then
  # Extract only struct field lines (pub ...) to avoid matching doc comments
  RECORD_FIELDS=$(grep -E '^\s+pub\s+\w+' "$RECORD_RS_FILE" 2>/dev/null || echo "")
  # Check for provenance-related fields (ingest stage)
  if echo "$RECORD_FIELDS" | grep -qiE 'source|provenance|origin|ingested|format|raw_size|ingest'; then
    STAGE_FIELD_MATCHES=$((STAGE_FIELD_MATCHES + 1))
    echo "  Found provenance/ingest fields"
  fi
  # Check for transformation-related fields (transform stage)
  if echo "$RECORD_FIELDS" | grep -qiE 'transform|validation|rules_applied|is_valid|integrity|rule_outcome'; then
    STAGE_FIELD_MATCHES=$((STAGE_FIELD_MATCHES + 1))
    echo "  Found transformation/validation fields"
  fi
  # Check for lineage/emission-related fields (emit stage)
  if echo "$RECORD_FIELDS" | grep -qiE 'lineage|emitted|emission|output_format|destination|emit'; then
    STAGE_FIELD_MATCHES=$((STAGE_FIELD_MATCHES + 1))
    echo "  Found lineage/emission fields"
  fi
fi
echo "  Stages represented in Record: $STAGE_FIELD_MATCHES/3"
check "Record has fields from 2+ stages ($STAGE_FIELD_MATCHES/3)" "$([ "$STAGE_FIELD_MATCHES" -ge 2 ] && echo 0 || echo 1)" 5

# Check 26: Record has fields from ALL 3 stages (5 pts)
echo ""
echo "--- Check 26: Record has fields from all 3 stages ---"
check "Record has fields from all 3 stages ($STAGE_FIELD_MATCHES/3)" "$([ "$STAGE_FIELD_MATCHES" -ge 3 ] && echo 0 || echo 1)" 5

# --- Pipeline Co-Evolution (10 pts) ---

# Check 27: PipelineError has stage-specific variants (5 pts)
echo ""
echo "--- Check 27: PipelineError has stage-specific variants ---"
ERROR_VARIANT_COUNT=0
if [[ -f "$PIPELINE_RS_FILE" ]]; then
  # Count error variants beyond Io and Other
  ERROR_VARIANT_COUNT=$(grep -cE '^\s+#\[error' "$PIPELINE_RS_FILE" 2>/dev/null) || ERROR_VARIANT_COUNT=0
fi
echo "  PipelineError has $ERROR_VARIANT_COUNT #[error] variants (starter=2)"
PIPELINE_EXTENDED=false
if [[ "$ERROR_VARIANT_COUNT" -ge 4 ]]; then
  PIPELINE_EXTENDED=true
fi
check "PipelineError has 4+ variants (workers added stage-specific errors)" "$($PIPELINE_EXTENDED && echo 0 || echo 1)" 5

# Check 28: 2+ PipelineStage implementations (5 pts)
echo ""
echo "--- Check 28: PipelineStage implementations ---"
STAGE_IMPL_COUNT=0
# Check each command file for PipelineStage impl
for mod_name in ingest transform emit; do
  MOD_FILE="$PROJECT_DIR/ws/default/src/commands/${mod_name}.rs"
  if [[ -f "$MOD_FILE" ]]; then
    if grep -qE 'impl.*PipelineStage|fn process|fn name' "$MOD_FILE" 2>/dev/null; then
      STAGE_IMPL_COUNT=$((STAGE_IMPL_COUNT + 1))
    fi
  fi
done
# Also check pipeline.rs itself for impls
if [[ -f "$PIPELINE_RS_FILE" ]]; then
  PIPELINE_IMPLS=$(grep -cE 'impl.*PipelineStage' "$PIPELINE_RS_FILE" 2>/dev/null) || PIPELINE_IMPLS=0
  STAGE_IMPL_COUNT=$((STAGE_IMPL_COUNT + PIPELINE_IMPLS))
fi
check "2+ PipelineStage implementations ($STAGE_IMPL_COUNT found)" "$([ "$STAGE_IMPL_COUNT" -ge 2 ] && echo 0 || echo 1)" 5

# --- Bus Communication (15 pts) ---

# Check 29: coord:interface messages posted by workers (5 pts)
echo ""
echo "--- Check 29: coord:interface messages posted ---"
COORD_MSG_COUNT=0
# Count coord:interface in channel labels
COORD_MSG_COUNT=$(echo "$CHANNEL_LABELS" | grep -ci "coord:interface" 2>/dev/null) || COORD_MSG_COUNT=0
# Also check channel text
if [[ "$COORD_MSG_COUNT" -eq 0 ]]; then
  COORD_MSG_COUNT=$(echo "$CHANNEL_HISTORY" | grep -ci "coord:interface\|coord.*interface" 2>/dev/null) || COORD_MSG_COUNT=0
fi
# Check coord-messages artifact
if [[ "$COORD_MSG_COUNT" -eq 0 ]]; then
  COORD_MSG_COUNT=$(echo "$COORD_MESSAGES" | grep -ci "coord" 2>/dev/null) || COORD_MSG_COUNT=0
fi
# Check worker and dev logs for bus send with coord
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log "$ARTIFACTS/agent-${FLOWLOG_DEV}.log"; do
  [[ -f "$wlog" ]] || continue
  WC=$(grep -ci "coord:interface\|bus send.*coord\|-L coord" "$wlog" 2>/dev/null) || WC=0
  COORD_MSG_COUNT=$((COORD_MSG_COUNT + WC))
done
echo "  coord:interface signals found: $COORD_MSG_COUNT"
HAS_COORD_MSG=false
if [[ "$COORD_MSG_COUNT" -ge 1 ]]; then
  HAS_COORD_MSG=true
fi
check "coord:interface messages posted ($COORD_MSG_COUNT found)" "$($HAS_COORD_MSG && echo 0 || echo 1)" 5

# Check 30: MULTIPLE agents posted coord messages (5 pts)
# This is the bidirectionality test — not just one agent posting
# IMPORTANT: only count agents that actually SENT coord messages (bus send with -L coord),
# not agents whose logs merely contain "coord:interface" as text (e.g., from monitoring)
echo ""
echo "--- Check 30: Multiple agents posted coordination messages ---"
COORD_AGENTS=0
# Check if dev-loop actually sent coord messages (look for bus send command, not just text)
if echo "$DEV_LOG" | grep -qi 'bus send.*-L coord\|bus send.*coord:interface'; then
  COORD_AGENTS=$((COORD_AGENTS + 1))
  echo "  Dev-loop sent coord messages"
fi
# Check each worker log for bus send with coord label
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  if grep -qi 'bus send.*-L coord\|bus send.*coord:interface' "$wlog" 2>/dev/null; then
    COORD_AGENTS=$((COORD_AGENTS + 1))
    echo "  Worker $(basename "$wlog") sent coord messages"
  fi
done
# Fallback: check channel JSON for coord messages from different agents
if [[ "$COORD_AGENTS" -lt 2 ]]; then
  COORD_SENDERS=$(echo "$CHANNEL_JSON" | jq -r '[.messages[] | select(.labels // [] | any(. == "coord:interface" or startswith("coord:"))) | .agent // .from // "unknown"] | unique | length' 2>/dev/null || echo "0")
  if [[ "$COORD_SENDERS" -ge 2 ]]; then
    COORD_AGENTS=$COORD_SENDERS
    echo "  Channel JSON shows $COORD_SENDERS distinct coord senders"
  fi
fi
echo "  Agents posting coord messages: $COORD_AGENTS"
MULTI_COORD=false
if [[ "$COORD_AGENTS" -ge 2 ]]; then
  MULTI_COORD=true
fi
check "2+ agents posted coordination messages ($COORD_AGENTS)" "$($MULTI_COORD && echo 0 || echo 1)" 5

# Check 31: Workers read bus for sibling updates (5 pts)
# Must show evidence of coordination-specific reads, not just routine inbox checks.
# Look for: bus history with coord/sibling/record/pipeline keywords, or bus search for coord.
echo ""
echo "--- Check 31: Workers read bus for sibling updates ---"
WORKER_COORD_READ_COUNT=0
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  # Coordination-specific: bus history/search mentioning coord, sibling, record, pipeline, shared
  if grep -qi 'bus history.*coord\|bus history.*sibling\|bus search.*coord\|bus search.*record\|bus search.*pipeline\|bus search.*shared\|coord:interface' "$wlog" 2>/dev/null; then
    WORKER_COORD_READ_COUNT=$((WORKER_COORD_READ_COUNT + 1))
  fi
done
echo "  Workers with coordination bus reads: $WORKER_COORD_READ_COUNT"
WORKERS_READ_BUS=false
# Also count if workers read bus history at all (weaker signal but still meaningful)
WORKER_ANY_HISTORY_COUNT=0
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  if grep -qi 'bus history' "$wlog" 2>/dev/null; then
    WORKER_ANY_HISTORY_COUNT=$((WORKER_ANY_HISTORY_COUNT + 1))
  fi
done
if [[ "$WORKER_COORD_READ_COUNT" -ge 1 ]]; then
  WORKERS_READ_BUS=true
elif [[ "$WORKER_ANY_HISTORY_COUNT" -ge 2 ]]; then
  # If 2+ workers read bus history at all, give credit (they're at least trying)
  WORKERS_READ_BUS=true
  echo "  Fallback: $WORKER_ANY_HISTORY_COUNT workers read bus history (not coord-specific)"
fi
check "Workers read bus for sibling updates ($WORKER_COORD_READ_COUNT coord-specific, $WORKER_ANY_HISTORY_COUNT any history)" "$($WORKERS_READ_BUS && echo 0 || echo 1)" 5

# ============================================================
# Friction Efficiency (10 pts)
# ============================================================
echo ""
echo "=== Friction Efficiency (10 pts) ==="
echo ""

echo "Analyzing agent logs for friction signals..."
echo ""

_count_tool_errors() {
  local log="$1"
  local count=0
  local tue; tue=$(grep -c "tool_use_error" "$log" 2>/dev/null) || tue=0
  count=$((count + tue))
  local ec; ec=$(awk '
    /Exit code [12][^0-9]/ || /Exit code [12]$/ {
      if (prev ~ /cargo run/ || prev ~ /\.\/target\// || (prev ~ /^> Bash/ && prev ~ /"command":"cd /)) next
      count++
    }
    { prev = $0 }
    END { print count+0 }
  ' "$log" 2>/dev/null) || ec=0
  count=$((count + ec))
  echo "$count"
}

# Dev + worker friction
DEV_ERRORS=$(_count_tool_errors "$ARTIFACTS/agent-${FLOWLOG_DEV}.log")
DEV_HELP=$(grep -c "\-\-help" "$ARTIFACTS/agent-${FLOWLOG_DEV}.log" 2>/dev/null) || DEV_HELP=0
DEV_RETRIES=$(grep -c "retry\|again\|Retrying" "$ARTIFACTS/agent-${FLOWLOG_DEV}.log" 2>/dev/null) || DEV_RETRIES=0

WORKER_ERRORS=0
WORKER_HELP=0
WORKER_RETRIES=0
for wlog in "$ARTIFACTS"/agent-${FLOWLOG_DEV}_*.log; do
  [[ -f "$wlog" ]] || continue
  WE=$(_count_tool_errors "$wlog")
  WH=$(grep -c "\-\-help" "$wlog" 2>/dev/null) || WH=0
  WR=$(grep -c "retry\|again\|Retrying" "$wlog" 2>/dev/null) || WR=0
  WORKER_ERRORS=$((WORKER_ERRORS + WE))
  WORKER_HELP=$((WORKER_HELP + WH))
  WORKER_RETRIES=$((WORKER_RETRIES + WR))
done

TOTAL_ERRORS=$((DEV_ERRORS + WORKER_ERRORS))
TOTAL_HELP=$((DEV_HELP + WORKER_HELP))
TOTAL_RETRIES=$((DEV_RETRIES + WORKER_RETRIES))

echo "Dev: $DEV_ERRORS errors, $DEV_HELP --help, $DEV_RETRIES retries"
echo "Workers: $WORKER_ERRORS errors, $WORKER_HELP --help, $WORKER_RETRIES retries"
echo "Total: $TOTAL_ERRORS errors, $TOTAL_HELP --help, $TOTAL_RETRIES retries"
echo ""

# Check 32: Tool errors (5 pts)
echo "--- Check 32: Tool errors ---"
TOTAL=$((TOTAL + 5))
if [[ "$TOTAL_ERRORS" -eq 0 ]]; then
  echo "PASS (5 pts): Zero tool errors"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 5 ]]; then
  echo "PARTIAL (3/5 pts): $TOTAL_ERRORS tool errors (threshold: <=5)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$TOTAL_ERRORS" -le 15 ]]; then
  echo "PARTIAL (1/5 pts): $TOTAL_ERRORS tool errors (threshold: <=15)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $TOTAL_ERRORS tool errors"
  FAIL=$((FAIL + 1))
fi
echo ""

# Check 33: --help + retries (5 pts)
echo "--- Check 33: --help and retries ---"
TOTAL=$((TOTAL + 5))
HELP_RETRY_TOTAL=$((TOTAL_HELP + TOTAL_RETRIES))
if [[ "$HELP_RETRY_TOTAL" -eq 0 ]]; then
  echo "PASS (5 pts): Zero --help lookups and retries"
  SCORE=$((SCORE + 5)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 3 ]]; then
  echo "PARTIAL (3/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=3)"
  SCORE=$((SCORE + 3)); PASS=$((PASS + 1))
elif [[ "$HELP_RETRY_TOTAL" -le 8 ]]; then
  echo "PARTIAL (1/5 pts): $HELP_RETRY_TOTAL --help/retries (threshold: <=8)"
  SCORE=$((SCORE + 1)); PASS=$((PASS + 1))
else
  echo "FAIL (0/5 pts): $HELP_RETRY_TOTAL --help/retries"
  FAIL=$((FAIL + 1))
fi
echo ""

# ============================================================
# Critical Fail: No workers spawned → cap at 30%
# ============================================================
if ! $WORKERS_SPAWNED; then
  MAX_SCORE=$(( TOTAL * 30 / 100 ))
  if [[ "$SCORE" -gt "$MAX_SCORE" ]]; then
    echo "CAPPING: No workers spawned — score capped from $SCORE to $MAX_SCORE (30% of $TOTAL)"
    SCORE=$MAX_SCORE
  fi
fi

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

# Category breakdown
echo "=== Category Breakdown ==="
echo "Mission Recognition:     15 pts"
echo "Decomposition:           25 pts"
echo "Worker Dispatch:         25 pts"
echo "Monitoring:              15 pts"
echo "Synthesis:               15 pts"
echo "Code Correctness:        25 pts"
echo "Coordination (L5v2):     40 pts"
echo "Friction Efficiency:     10 pts"
echo "                        ─────────"
echo "Total possible:         ~170 pts"
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
echo "MISSION_BONE=$MISSION_BONE"
echo ""
echo "Run 'BOTBUS_DATA_DIR=$BOTBUS_DATA_DIR bus history flowlog -n 50' for channel messages"
echo "Run 'cat $ARTIFACTS/agent-${FLOWLOG_DEV}.log' for dev output"
echo "Run 'ls $ARTIFACTS/agent-${FLOWLOG_DEV}_*.log' for worker logs"
echo "Run 'cat $ARTIFACTS/coord-messages.log' for coordination messages"
echo "Run 'cat $ARTIFACTS/record-final.rs' for final Record struct"
echo "Run 'cat $ARTIFACTS/pipeline-final.rs' for final PipelineError enum"
echo ""
echo "=== Verification Complete ==="
