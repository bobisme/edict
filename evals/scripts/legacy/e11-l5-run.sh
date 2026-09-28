#!/usr/bin/env bash
set -euo pipefail

# E11-L5v2 Coordination Eval — Orchestrator
# Sends !mission message to flowlog channel, polls mission lifecycle
# (decomposition → worker dispatch → checkpoint → synthesis), captures artifacts.
# Additionally captures coordination-specific artifacts (coord:interface messages,
# bus history calls in worker logs).
#
# KEY DIFFERENCE FROM L5v1 (taskr):
# The mission spec uses DOMAIN language, not code language. It says "track data
# provenance" not "add source: String to Record". Workers must make implementation
# decisions themselves, add fields to the shared Record struct, and coordinate
# via bus about what they added. No single worker can define Record upfront.
#
# Expected flow:
# 1. Router hook fires → respond.mjs → routes !mission
# 2. respond.mjs creates mission bone → execs into dev-loop with BOTBOX_MISSION
# 3. Dev-loop decomposes mission into child bones
# 4. Dev-loop dispatches workers (flowlog-dev/<random>) for children
# 5. Workers implement stages, each ADDING fields to shared Record struct
# 6. Workers post coord:interface when they modify record.rs or pipeline.rs
# 7. Workers read bus for sibling changes to shared types
# 8. Dev-loop monitors via checkpoints, synthesizes results

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E11_TIMEOUT:-1800}  # 30 minutes default
POLL_INTERVAL=30                       # seconds between status checks
STUCK_THRESHOLD=300                    # 5 minutes without progress = stuck

echo "=== E11-L5v2 Coordination Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e11-l5-setup.sh" 2>&1) || {
  echo "$SETUP_OUTPUT"
  echo "FATAL: Setup failed"
  exit 1
}
echo "$SETUP_OUTPUT"

EVAL_DIR=$(echo "$SETUP_OUTPUT" | grep -oP 'EVAL_DIR=\K.*' | head -1)
if [[ -z "$EVAL_DIR" || ! -f "$EVAL_DIR/.eval-env" ]]; then
  echo "FATAL: Setup completed but could not find .eval-env"
  echo "Output was:"
  echo "$SETUP_OUTPUT"
  exit 1
fi

source "$EVAL_DIR/.eval-env"
ARTIFACTS="$EVAL_DIR/artifacts"
echo "--- setup: OK (EVAL_DIR=$EVAL_DIR) ---"
echo ""

# --- Verify hooks ---
echo "--- Verifying hooks ---"
HOOKS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus hooks list 2>&1)
echo "$HOOKS"

FLOWLOG_ROUTER_OK=false
if echo "$HOOKS" | grep -qi "flowlog.*claim\|claim.*flowlog"; then
  FLOWLOG_ROUTER_OK=true
  echo "  flowlog router hook: OK"
fi

if ! $FLOWLOG_ROUTER_OK; then echo "WARNING: No flowlog router hook found"; fi
echo "--- hooks: OK ---"
echo ""

# --- Build mission spec ---
# CRITICAL: Uses domain language, NOT code language.
# Says "track provenance" not "add source: String". Workers decide field names.
MISSION_SPEC='!mission Implement all flowlog pipeline stages with co-evolving shared types
Outcome: A working flowlog CLI where all three pipeline stages (ingest, transform, emit) produce correct output with all flags working, processing the sample data files end-to-end through the full pipeline.

## Architecture — CO-EVOLVING SHARED TYPES (CRITICAL)

The project has two shared modules that ALL pipeline stages depend on and MUST EXTEND:

- src/record.rs — Record struct: currently has ONLY id + data fields. Each stage MUST add fields for its domain concerns. Do NOT modify src/main.rs.
- src/pipeline.rs — PipelineStage trait + PipelineError enum: currently minimal stubs. Each stage MUST add error variants and implement the trait.
- src/commands/ingest.rs — flowlog ingest: read source data, track data provenance
- src/commands/transform.rs — flowlog transform: apply rules, verify data integrity
- src/commands/emit.rs — flowlog emit: write output, record data lineage
- src/main.rs — clap dispatch (already wired, do NOT modify)

**CO-EVOLUTION REQUIREMENT**: Record starts nearly empty. Each worker MUST add fields to Record for their stage'"'"'s domain concerns. The full Record shape cannot be known until all stages have added their fields. Workers MUST:
1. Add fields to Record in record.rs for their stage'"'"'s needs — the specs describe WHAT to track (provenance, integrity, lineage) not HOW (no field names given)
2. Add error variants to PipelineError in pipeline.rs for their stage'"'"'s failure modes
3. Implement PipelineStage trait for their stage
4. Post bus messages with -L coord:interface announcing what fields/variants they added to shared types
5. Read bus history for sibling coord:interface messages to discover what fields siblings added
6. If a sibling already added fields you need to read (e.g., emit reads provenance fields that ingest added), coordinate on field names via bus

## Stage specs (DOMAIN language — workers decide field names and types)

### 1. src/commands/ingest.rs — Data ingestion with provenance tracking

Read data from source files (CSV or JSON) and track where each record came from. Every ingested record must carry enough context to answer: "Where did this data come from?"

Domain requirements:
- Detect source format automatically from file extension (CSV vs JSON), with --format override
- CSV: each row becomes a record, headers become data field names
- JSON: each object becomes a record (supports both JSON arrays and line-delimited JSON)
- Track data provenance: the source file path, the detected/specified format, the time of ingestion, and the raw byte size of the source
- Generate unique IDs for each record
- --json flag: output ingested records as a JSON array to stdout (one record per line for piping)
- Without --json: print summary ("N records ingested from <source>")

Uses and EXTENDS: record.rs (add provenance fields), pipeline.rs (add error variants, implement trait)

### 2. src/commands/transform.rs — Data transformation with integrity verification

Apply transformation rules to records and verify data integrity. Every transformed record must carry enough context to answer: "What happened to this data?"

Domain requirements:
- Read records from --input file (JSON, one record per line — as output by ingest --json)
- Load rules from a JSON rules file (see data/rules.json for format)
- Supported rule actions: uppercase, lowercase, validate_range, validate_pattern, default
- Track transformation history: which rules were applied to each record and their outcomes
- Track validation state: whether each record passed all validation rules, and what failed
- --strict mode: reject records entirely if any validation fails
- Without --strict: keep records but mark them as having validation issues
- Write transformed records to --output file (or stdout if not specified)
- Print summary: "N records transformed, M validation errors"

Uses and EXTENDS: record.rs (add transformation/validation fields), pipeline.rs (add error variants, implement trait)

### 3. src/commands/emit.rs — Data emission with lineage tracking

Write records to output destination with full data lineage. Every emitted record must carry enough context to answer: "What is the complete history of this data?"

Domain requirements:
- Read records from --input file (JSON, one record per line)
- Output formats: json (records as JSON array), csv (data fields as CSV), summary (human-readable aggregate stats)
- Track data lineage: assemble the complete provenance-to-emission chain for each record
- --lineage flag: include full lineage chain in JSON output; for CSV, write companion .lineage.json file
- Summary format: record count, source breakdown (how many from each source), transformation stats, validation pass rate
- Record emission metadata: when emitted, output format, destination path

Uses and EXTENDS: record.rs (add emission/lineage fields), pipeline.rs (add error variants, implement trait)

## Why coordination is mandatory

Record starts with ONLY id + data. Each stage adds its own fields:
- Ingest adds provenance fields (source tracking, format, timing, size)
- Transform adds integrity fields (rule outcomes, validation state, history)
- Emit adds lineage fields (assembly of full chain, emission metadata)

These fields are NOT specified — workers must decide names, types, and structure. When emit needs to read provenance fields that ingest added, it must discover what ingest named them. This REQUIRES reading bus history for coord:interface messages.

## Test data
Sample data in data/: sample.csv (5 rows), sample.json (same data as JSON array), rules.json (4 rules: uppercase, range, pattern, default), strict-rules.json (strict validation), bad-records.json (malformed records for error handling).

End-to-end pipeline test: ingest sample.csv --json | transform rules.json --input /dev/stdin --output /tmp/transformed.json | emit /tmp/output.json --input /tmp/transformed.json --lineage

Success metric: all three subcommands work on sample data, pipeline runs end-to-end, --json/--lineage/--strict/--format flags work, shared Record struct has fields from all stages, PipelineError has variants from all stages.
Constraints: Use existing dependencies only (clap, serde, serde_json, csv, chrono, thiserror). Do not modify src/main.rs.
Stop criteria: All three stages fully working with all flags, Record has fields from all 3 stages, pipeline runs end-to-end on sample data.'

# --- Send !mission message (triggers router hook → respond.mjs → dev-loop) ---
echo "--- Sending !mission to flowlog channel ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup flowlog \
  "$MISSION_SPEC" \
  -L task-request
echo "Mission spec sent. Router hook should fire shortly."
echo ""

# --- Poll loop ---
echo "--- Polling for completion (timeout: ${OVERALL_TIMEOUT}s) ---"
START_TIME=$(date +%s)
LAST_ACTIVITY_TIME=$START_TIME
LAST_MSG_COUNT=0

FINAL_STATUS_DEV="unknown"

# Mission tracking
MISSION_BONE=""
CHILD_COUNT=0
CHILDREN_CLOSED=0

# Worker tracking
declare -A KNOWN_WORKERS=()
declare -A WORKER_LOG_CAPTURED=()
KNOWN_WORKER_COUNT=0

# Phase timing
DEV_SPAWN_TIME=""
MISSION_FOUND_TIME=""
FIRST_CHILD_TIME=""
FIRST_WORKER_TIME=""
MISSION_CLOSED_TIME=""
PHASE_TIMES=""

# Previous state for incremental capture
PREV_DEV_RUNNING=""

while true; do
  ELAPSED=$(( $(date +%s) - START_TIME ))

  if [[ $ELAPSED -ge $OVERALL_TIMEOUT ]]; then
    echo "TIMEOUT: Overall timeout reached (${OVERALL_TIMEOUT}s)"
    FINAL_STATUS_DEV="timeout"
    break
  fi

  sleep "$POLL_INTERVAL"
  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo ""
  echo "--- Poll (${ELAPSED}s / ${OVERALL_TIMEOUT}s) ---"

  # Check vessel — which agents are running?
  BOTTY_JSON=$(vessel list --format json 2>/dev/null || echo '{"agents":[]}')

  DEV_RUNNING=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$FLOWLOG_DEV\") | .id" 2>/dev/null || echo "")
  RESPOND_RUNNING=$(echo "$BOTTY_JSON" | jq -r '.agents[] | select(.id | test("respond")) | .id' 2>/dev/null || echo "")

  if [[ -n "$DEV_RUNNING" ]]; then
    echo "  flowlog-dev: RUNNING"
    if [[ -z "$DEV_SPAWN_TIME" ]]; then
      DEV_SPAWN_TIME=$ELAPSED
      PHASE_TIMES+="dev_spawn=${DEV_SPAWN_TIME}s\n"
    fi
  else
    echo "  flowlog-dev: not running"
  fi

  if [[ -n "$RESPOND_RUNNING" ]]; then
    echo "  respond: RUNNING ($RESPOND_RUNNING)"
  fi

  # Discover workers (hierarchical names: flowlog-dev/<random>)
  WORKER_LIST=$(echo "$BOTTY_JSON" | jq -r '.agents[] | select(.id | test("/")) | .id' 2>/dev/null || echo "")
  if [[ -n "$WORKER_LIST" ]]; then
    while IFS= read -r worker_id; do
      [[ -z "$worker_id" ]] && continue
      wkey="${worker_id//\//_}"
      if [[ -z "${KNOWN_WORKERS[$wkey]+_}" ]]; then
        KNOWN_WORKERS["$wkey"]="$worker_id"
        WORKER_LOG_CAPTURED["$wkey"]=0
        KNOWN_WORKER_COUNT=$((KNOWN_WORKER_COUNT + 1))
        echo "  NEW WORKER: $worker_id"
        LAST_ACTIVITY_TIME=$(date +%s)
        if [[ -z "$FIRST_WORKER_TIME" ]]; then
          FIRST_WORKER_TIME=$ELAPSED
          PHASE_TIMES+="first_worker=${FIRST_WORKER_TIME}s\n"
        fi
      else
        echo "  worker: $worker_id (running)"
      fi
    done <<< "$WORKER_LIST"
  fi

  # Check for workers that disappeared — capture their logs
  if [[ ${#KNOWN_WORKERS[@]} -gt 0 ]]; then
    for wkey in "${!KNOWN_WORKERS[@]}"; do
      orig_id="${KNOWN_WORKERS[$wkey]}"
      if ! echo "$WORKER_LIST" | grep -qF "$orig_id" 2>/dev/null; then
        if [[ "${WORKER_LOG_CAPTURED[$wkey]}" -eq 0 ]]; then
          vessel tail "$orig_id" -n 500 > "$ARTIFACTS/agent-${wkey}.log" 2>/dev/null || true
          WORKER_LOG_CAPTURED["$wkey"]=1
          echo "  worker EXITED: $orig_id (log captured)"
          LAST_ACTIVITY_TIME=$(date +%s)
        fi
      fi
    done
  fi

  # Discover mission bone
  cd "$PROJECT_DIR"
  if [[ -z "$MISSION_BONE" ]]; then
    MISSION_BONES=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l mission --format json 2>/dev/null || echo '[]')
    MISSION_BONE=$(echo "$MISSION_BONES" | jq -r '.[0].id // empty' 2>/dev/null || echo "")
    if [[ -z "$MISSION_BONE" ]]; then
      # Try alternative JSON shape
      MISSION_BONE=$(echo "$MISSION_BONES" | jq -r '.bones[0].id // empty' 2>/dev/null || echo "")
    fi
    if [[ -n "$MISSION_BONE" ]]; then
      echo "  MISSION BONE: $MISSION_BONE"
      MISSION_FOUND_TIME=$ELAPSED
      PHASE_TIMES+="mission_found=${MISSION_FOUND_TIME}s\n"
      LAST_ACTIVITY_TIME=$(date +%s)
    fi
  fi

  # Track children if mission exists
  if [[ -n "$MISSION_BONE" ]]; then
    CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$MISSION_BONE" --format json 2>/dev/null || echo '[]')
    # Try both JSON shapes
    NEW_CHILD_COUNT=$(echo "$CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")
    NEW_CLOSED=$(echo "$CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")

    if [[ "$NEW_CHILD_COUNT" -gt "$CHILD_COUNT" ]]; then
      LAST_ACTIVITY_TIME=$(date +%s)
      if [[ "$CHILD_COUNT" -eq 0 && "$NEW_CHILD_COUNT" -gt 0 && -z "$FIRST_CHILD_TIME" ]]; then
        FIRST_CHILD_TIME=$ELAPSED
        PHASE_TIMES+="first_child=${FIRST_CHILD_TIME}s\n"
      fi
    fi
    CHILD_COUNT=$NEW_CHILD_COUNT

    if [[ "$NEW_CLOSED" -gt "$CHILDREN_CLOSED" ]]; then
      LAST_ACTIVITY_TIME=$(date +%s)
    fi
    CHILDREN_CLOSED=$NEW_CLOSED

    echo "  Children: $CHILDREN_CLOSED/$CHILD_COUNT done"

    # Check mission bone state
    MISSION_STATE=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$MISSION_BONE" --format json 2>/dev/null | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")
    echo "  Mission $MISSION_BONE: $MISSION_STATE"

    if [[ "$MISSION_STATE" == "done" ]]; then
      echo "  Mission is DONE — workflow completed!"
      MISSION_CLOSED_TIME=$ELAPSED
      PHASE_TIMES+="mission_closed=${MISSION_CLOSED_TIME}s\n"
      FINAL_STATUS_DEV="completed-still-running"

      # Grace period for agents to exit
      for WAIT_I in 1 2 3 4; do
        sleep 15
        BOTTY_FINAL=$(vessel list --format json 2>/dev/null || echo '{"agents":[]}')
        AD_RUNNING=$(echo "$BOTTY_FINAL" | jq -r ".agents[] | select(.id == \"$FLOWLOG_DEV\") | .id" 2>/dev/null || echo "")
        [[ -z "$AD_RUNNING" ]] && FINAL_STATUS_DEV="completed"
        REMAINING=$(echo "$BOTTY_FINAL" | jq '.agents | length' 2>/dev/null || echo "0")
        if [[ "$REMAINING" -eq 0 ]]; then
          FINAL_STATUS_DEV="completed"
          break
        fi
        echo "  Waiting for agents to exit... (${WAIT_I}/4)"
      done
      break
    fi
  fi

  # Check bus activity
  MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history flowlog -n 200 2>/dev/null | wc -l || echo "0")
  echo "  Channel messages: $MSG_COUNT"

  if [[ "$MSG_COUNT" -gt "$LAST_MSG_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_MSG_COUNT=$MSG_COUNT
  fi

  # Check for coordination messages (L5-specific)
  COORD_MSGS=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history flowlog -n 200 2>/dev/null | grep -ci "coord:interface\|coord:blocker\|record.*field\|pipeline.*error\|shared.*type" || echo "0")
  if [[ "$COORD_MSGS" -gt 0 ]]; then
    echo "  Coordination messages detected: $COORD_MSGS"
  fi

  # Stuck detection
  IDLE_TIME=$(( $(date +%s) - LAST_ACTIVITY_TIME ))
  if [[ $IDLE_TIME -ge $STUCK_THRESHOLD ]]; then
    echo "  WARNING: No activity for ${IDLE_TIME}s (threshold: ${STUCK_THRESHOLD}s)"
    # If ALL agents have exited without closing mission, it's a failure
    ALL_AGENTS=$(echo "$BOTTY_JSON" | jq '.agents | length' 2>/dev/null || echo "0")
    if [[ "$ALL_AGENTS" -eq 0 ]]; then
      echo "  All agents exited without completing mission — marking as agent-exited"
      FINAL_STATUS_DEV="agent-exited"
      break
    fi
  fi
done

echo ""
echo "--- Final status: flowlog-dev=$FINAL_STATUS_DEV ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Dev agent log
vessel tail "$FLOWLOG_DEV" -n 500 > "$ARTIFACTS/agent-${FLOWLOG_DEV}.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-${FLOWLOG_DEV}.log"
echo "  log: $ARTIFACTS/agent-${FLOWLOG_DEV}.log"

# Respond agent logs
for RESP_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]? | select(.id | test("respond")) | .id' 2>/dev/null || true); do
  vessel tail "$RESP_NAME" -n 500 > "$ARTIFACTS/agent-${RESP_NAME}.log" 2>/dev/null || true
  echo "  respond log: $ARTIFACTS/agent-${RESP_NAME}.log"
done
# Try common respond names that may have exited
for RNAME in "flowlog-respond" "respond"; do
  if [[ ! -f "$ARTIFACTS/agent-${RNAME}.log" ]]; then
    vessel tail "$RNAME" -n 500 > "$ARTIFACTS/agent-${RNAME}.log" 2>/dev/null || true
  fi
done

# Worker logs (capture any remaining)
if [[ ${#KNOWN_WORKERS[@]} -gt 0 ]]; then
  for wkey in "${!KNOWN_WORKERS[@]}"; do
    orig_id="${KNOWN_WORKERS[$wkey]}"
    if [[ ! -f "$ARTIFACTS/agent-${wkey}.log" ]] || [[ "${WORKER_LOG_CAPTURED[$wkey]}" -eq 0 ]]; then
      vessel tail "$orig_id" -n 500 > "$ARTIFACTS/agent-${wkey}.log" 2>/dev/null || true
      echo "  worker log: $ARTIFACTS/agent-${wkey}.log"
    fi
  done
fi

# Any other agents we missed
for AGENT_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  SAFE_NAME="${AGENT_NAME//\//_}"
  if [[ ! -f "$ARTIFACTS/agent-${SAFE_NAME}.log" ]]; then
    vessel tail "$AGENT_NAME" -n 500 > "$ARTIFACTS/agent-${SAFE_NAME}.log" 2>/dev/null || true
    echo "  extra log: $ARTIFACTS/agent-${SAFE_NAME}.log"
  fi
done

# Channel history (text + JSON)
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history flowlog -n 200 > "$ARTIFACTS/channel-flowlog-history.log" 2>/dev/null || \
  echo "(no history)" > "$ARTIFACTS/channel-flowlog-history.log"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history flowlog -n 200 --format json > "$ARTIFACTS/channel-flowlog-history.json" 2>/dev/null || \
  echo '{"messages":[]}' > "$ARTIFACTS/channel-flowlog-history.json"
echo "  channel: $ARTIFACTS/channel-flowlog-history.log"

# L5-specific: extract coordination messages
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus search "coord" -c flowlog > "$ARTIFACTS/coord-messages.log" 2>/dev/null || \
  echo "(no coord messages)" > "$ARTIFACTS/coord-messages.log"
echo "  coord messages: $ARTIFACTS/coord-messages.log"

# Mission bone state
cd "$PROJECT_DIR"
if [[ -n "$MISSION_BONE" ]]; then
  BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$MISSION_BONE" --format json > "$ARTIFACTS/mission-bone-state.json" 2>/dev/null || \
    echo "[]" > "$ARTIFACTS/mission-bone-state.json"
  echo "  mission bone: $ARTIFACTS/mission-bone-state.json"
fi

# All bones state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --format json > "$ARTIFACTS/all-bones-state.json" 2>/dev/null || \
  echo '[]' > "$ARTIFACTS/all-bones-state.json"
echo "  all bones: $ARTIFACTS/all-bones-state.json"

# Workspace state
maw ws list --format json > "$ARTIFACTS/workspace-state.json" 2>/dev/null || \
  echo "{}" > "$ARTIFACTS/workspace-state.json"

# Claims state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list > "$ARTIFACTS/claims-state.txt" 2>/dev/null || \
  echo "(no claims)" > "$ARTIFACTS/claims-state.txt"

# L5v2-specific: capture record.rs and pipeline.rs final state
RECORD_RS="$PROJECT_DIR/ws/default/src/record.rs"
PIPELINE_RS="$PROJECT_DIR/ws/default/src/pipeline.rs"
cp "$RECORD_RS" "$ARTIFACTS/record-final.rs" 2>/dev/null || echo "(not found)" > "$ARTIFACTS/record-final.rs"
cp "$PIPELINE_RS" "$ARTIFACTS/pipeline-final.rs" 2>/dev/null || echo "(not found)" > "$ARTIFACTS/pipeline-final.rs"
echo "  record.rs: $ARTIFACTS/record-final.rs"
echo "  pipeline.rs: $ARTIFACTS/pipeline-final.rs"

# Final status file
WORKER_NAMES_LIST=""
if [[ $KNOWN_WORKER_COUNT -gt 0 ]]; then
  WORKER_NAMES_LIST=$(for wkey in "${!KNOWN_WORKERS[@]}"; do echo "${KNOWN_WORKERS[$wkey]}"; done | sort)
fi
{
  echo "FLOWLOG_DEV_STATUS=$FINAL_STATUS_DEV"
  echo "MISSION_BONE=${MISSION_BONE:-none}"
  echo "CHILD_COUNT=$CHILD_COUNT"
  echo "CHILDREN_CLOSED=$CHILDREN_CLOSED"
  echo "WORKER_COUNT=$KNOWN_WORKER_COUNT"
  echo "WORKER_NAMES_START"
  if [[ -n "$WORKER_NAMES_LIST" ]]; then
    echo "$WORKER_NAMES_LIST"
  fi
  echo "WORKER_NAMES_END"
} > "$ARTIFACTS/final-status.txt"

echo -e "$PHASE_TIMES" > "$ARTIFACTS/phase-times.log" 2>/dev/null || true

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
vessel kill "$FLOWLOG_DEV" 2>/dev/null || true
for AGENT_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  vessel kill "$AGENT_NAME" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e11-l5-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== E11-L5v2 Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status:"
echo "  flowlog-dev: $FINAL_STATUS_DEV"
echo "  Mission bone: ${MISSION_BONE:-none}"
echo "  Children: $CHILDREN_CLOSED/$CHILD_COUNT done"
echo "  Workers discovered: $KNOWN_WORKER_COUNT"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Phase timing:"
[[ -n "$DEV_SPAWN_TIME" ]] && echo "  dev spawn: ${DEV_SPAWN_TIME}s"
[[ -n "$MISSION_FOUND_TIME" ]] && echo "  mission found: ${MISSION_FOUND_TIME}s"
[[ -n "$FIRST_CHILD_TIME" ]] && echo "  first child: ${FIRST_CHILD_TIME}s"
[[ -n "$FIRST_WORKER_TIME" ]] && echo "  first worker: ${FIRST_WORKER_TIME}s"
[[ -n "$MISSION_CLOSED_TIME" ]] && echo "  mission closed: ${MISSION_CLOSED_TIME}s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
echo "To inspect results:"
echo "  source $EVAL_DIR/.eval-env"
echo "  ls $ARTIFACTS/"
echo "  cat $ARTIFACTS/agent-${FLOWLOG_DEV}.log"
echo "  cat $ARTIFACTS/channel-flowlog-history.log"
echo "  cat $ARTIFACTS/coord-messages.log"
echo "  cat $ARTIFACTS/record-final.rs"
echo ""
