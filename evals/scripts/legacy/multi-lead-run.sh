#!/usr/bin/env bash
set -euo pipefail

# Multi-Lead Eval — Orchestrator
# Sends TWO independent !mission messages to test concurrent lead orchestrators.
# Tracks two missions, multiple lead slots, worker dispatch, and merge serialization.
#
# Expected flow:
# 1. Send Mission A (error.rs + stats) → router spawns lead slot 0
# 2. Send Mission B (search + convert) → router spawns lead slot 1
# 3. Both leads decompose their missions into child bones
# 4. Both leads dispatch workers in parallel
# 5. Merge mutex serializes all merges into default
# 6. Both missions complete independently

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${ML_TIMEOUT:-2400}  # 40 minutes (two concurrent missions)
POLL_INTERVAL=30
STUCK_THRESHOLD=300

echo "=== Multi-Lead Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/multi-lead-setup.sh" 2>&1) || {
  echo "$SETUP_OUTPUT"
  echo "FATAL: Setup failed"
  exit 1
}
echo "$SETUP_OUTPUT"

EVAL_DIR=$(echo "$SETUP_OUTPUT" | grep -oP 'EVAL_DIR=\K.*' | head -1)
if [[ -z "$EVAL_DIR" || ! -f "$EVAL_DIR/.eval-env" ]]; then
  echo "FATAL: Setup completed but could not find .eval-env"
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

if echo "$HOOKS" | grep -qi "futil.*claim\|claim.*futil\|router"; then
  echo "  Router hook: OK"
else
  echo "WARNING: No router hook found"
fi
echo "--- hooks: OK ---"
echo ""

# --- Mission A spec (error.rs + stats) ---
MISSION_A_SPEC='!mission Implement shared error handling and stats subcommand
Outcome: Working error.rs helper functions and a fully functional stats subcommand.

## Component specs

### 1. src/error.rs — shared error handling (implement FIRST)
The FutilError enum is defined. Implement three helper functions:
  - validate_file(path) → Result<String, FutilError>: check file exists, read to string
  - detect_format(path) → Result<&str, FutilError>: check extension (.json/.csv/.jsonl)
  - write_output(content, output_path) → Result<(), FutilError>: write to file or stdout

### 2. src/stats.rs — futil stats [OPTIONS] <paths...>
Count lines, words, bytes per file. Features:
  --json: output as JSON array
  --chars: include character count
  --top-words N: show N most frequent words
Multiple files: per-file + total summary.
Use error::validate_file for file loading.

## Test data
data/sample.txt, data/words.txt available.

Success metric: cargo test passes, stats subcommand works on sample data.
Constraints: Use existing deps only. Do not modify src/main.rs.
Stop criteria: error.rs helpers + stats fully working with tests.'

# --- Mission B spec (search + convert) ---
MISSION_B_SPEC='!mission Implement search and convert subcommands
Outcome: Working search and convert subcommands with all flags.

## Component specs

### 1. src/search.rs — futil search [OPTIONS] <pattern> <paths...>
Regex search with context lines. Features:
  -A/-B/-C N: context lines
  -i: case-insensitive
  -c: count-only mode
  -l: files-only mode
  -v: invert match
  --json: JSON output
Multi-file support with filename prefixes.
Use error::validate_file and FutilError::InvalidRegex.

### 2. src/convert.rs — futil convert [OPTIONS] <input> --format <fmt>
Format conversion: JSON, CSV, JSONL (6 pairs). Features:
  --fields f1,f2: select/reorder fields
  --sort-by field: sort rows ascending
  --pretty: pretty-print JSON
  --output path: write to file
Auto-detect input format from extension.
Use error::validate_file and error::detect_format.

## Test data
data/sample.txt, data/words.txt, data/log.txt, data/sample.csv, data/sample.json, data/sample.jsonl

Success metric: cargo test passes, both subcommands work on sample data.
Constraints: Use existing deps only. Do not modify src/main.rs. error.rs may already be implemented by another lead.
Stop criteria: search + convert fully working with tests.'

# --- Send Mission A ---
echo "--- Sending Mission A: error.rs + stats ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup futil \
  "$MISSION_A_SPEC" -L task-request
echo "Mission A sent."
echo ""

# --- Brief pause to let router process first message ---
sleep 5

# --- Send Mission B ---
echo "--- Sending Mission B: search + convert ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup futil \
  "$MISSION_B_SPEC" -L task-request
echo "Mission B sent."
echo ""

# --- Poll loop ---
echo "--- Polling for completion (timeout: ${OVERALL_TIMEOUT}s) ---"
START_TIME=$(date +%s)
LAST_ACTIVITY_TIME=$START_TIME
LAST_MSG_COUNT=0

FINAL_STATUS="unknown"

# Mission tracking
MISSION_A_BONE=""
MISSION_B_BONE=""
MISSION_A_CHILDREN=0
MISSION_B_CHILDREN=0
MISSION_A_CLOSED=0
MISSION_B_CLOSED=0
MISSION_A_STATUS="unknown"
MISSION_B_STATUS="unknown"

# Lead slot tracking
declare -A LEAD_SLOTS=()
LEAD_SLOT_COUNT=0

# Worker tracking
declare -A KNOWN_WORKERS=()
declare -A WORKER_LOG_CAPTURED=()
KNOWN_WORKER_COUNT=0

# Phase timing
FIRST_LEAD_TIME=""
SECOND_LEAD_TIME=""
FIRST_MISSION_FOUND_TIME=""
SECOND_MISSION_FOUND_TIME=""
FIRST_WORKER_TIME=""
ALL_MISSIONS_CLOSED_TIME=""
PHASE_TIMES=""

# Merge tracking
MERGE_COUNT=0
LAST_MERGE_CHECK=""

while true; do
  ELAPSED=$(( $(date +%s) - START_TIME ))

  if [[ $ELAPSED -ge $OVERALL_TIMEOUT ]]; then
    echo "TIMEOUT: Overall timeout reached (${OVERALL_TIMEOUT}s)"
    FINAL_STATUS="timeout"
    break
  fi

  sleep "$POLL_INTERVAL"
  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo ""
  echo "--- Poll (${ELAPSED}s / ${OVERALL_TIMEOUT}s) ---"

  # Check vessel — which agents are running?
  BOTTY_JSON=$(vessel list --format json 2>/dev/null || echo '{"agents":[]}')

  # Discover lead slots (futil-dev/0, futil-dev/1, etc.)
  LEAD_LIST=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id | test(\"^${FUTIL_DEV}/[0-9]+$\")) | .id" 2>/dev/null || echo "")
  if [[ -n "$LEAD_LIST" ]]; then
    while IFS= read -r lead_id; do
      [[ -z "$lead_id" ]] && continue
      lkey="${lead_id//\//_}"
      if [[ -z "${LEAD_SLOTS[$lkey]+_}" ]]; then
        LEAD_SLOTS["$lkey"]="$lead_id"
        LEAD_SLOT_COUNT=$((LEAD_SLOT_COUNT + 1))
        echo "  NEW LEAD: $lead_id (slot $((LEAD_SLOT_COUNT)))"
        LAST_ACTIVITY_TIME=$(date +%s)
        if [[ -z "$FIRST_LEAD_TIME" ]]; then
          FIRST_LEAD_TIME=$ELAPSED
          PHASE_TIMES+="first_lead=${FIRST_LEAD_TIME}s\n"
        elif [[ -z "$SECOND_LEAD_TIME" ]]; then
          SECOND_LEAD_TIME=$ELAPSED
          PHASE_TIMES+="second_lead=${SECOND_LEAD_TIME}s\n"
        fi
      else
        echo "  lead: $lead_id (running)"
      fi
    done <<< "$LEAD_LIST"
  fi

  # Also check for router instances
  ROUTER_RUNNING=$(echo "$BOTTY_JSON" | jq -r '.agents[] | select(.id | test("router")) | .id' 2>/dev/null || echo "")
  if [[ -n "$ROUTER_RUNNING" ]]; then
    echo "  router: RUNNING ($ROUTER_RUNNING)"
  fi

  # Also check for old-style futil-dev (non-slotted, in case single-lead fallback)
  DEV_RUNNING=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$FUTIL_DEV\") | .id" 2>/dev/null || echo "")
  if [[ -n "$DEV_RUNNING" ]]; then
    echo "  futil-dev (non-slotted): RUNNING"
  fi

  # Discover workers (hierarchical names with more than one /)
  WORKER_LIST=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id | test(\"^${FUTIL_DEV}/[0-9]+/\")) | .id" 2>/dev/null || echo "")
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

  # Capture logs for disappeared workers
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

  # Discover mission bones
  cd "$PROJECT_DIR"
  MISSION_BONES_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l mission --format json 2>/dev/null || echo '[]')
  MISSION_IDS=$(echo "$MISSION_BONES_JSON" | jq -r 'if type == "array" then .[].id elif .bones then .bones[].id else empty end' 2>/dev/null || echo "")

  for mid in $MISSION_IDS; do
    [[ -z "$mid" ]] && continue
    if [[ -z "$MISSION_A_BONE" ]]; then
      MISSION_A_BONE="$mid"
      echo "  MISSION A: $mid"
      if [[ -z "$FIRST_MISSION_FOUND_TIME" ]]; then
        FIRST_MISSION_FOUND_TIME=$ELAPSED
        PHASE_TIMES+="first_mission=${FIRST_MISSION_FOUND_TIME}s\n"
      fi
      LAST_ACTIVITY_TIME=$(date +%s)
    elif [[ "$mid" != "$MISSION_A_BONE" && -z "$MISSION_B_BONE" ]]; then
      MISSION_B_BONE="$mid"
      echo "  MISSION B: $mid"
      if [[ -z "$SECOND_MISSION_FOUND_TIME" ]]; then
        SECOND_MISSION_FOUND_TIME=$ELAPSED
        PHASE_TIMES+="second_mission=${SECOND_MISSION_FOUND_TIME}s\n"
      fi
      LAST_ACTIVITY_TIME=$(date +%s)
    fi
  done

  # Track children for each mission
  for mission_label in "A" "B"; do
    if [[ "$mission_label" == "A" ]]; then
      mbone="$MISSION_A_BONE"
    else
      mbone="$MISSION_B_BONE"
    fi
    [[ -z "$mbone" ]] && continue

    CHILDREN_JSON=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all -l "mission:$mbone" --format json 2>/dev/null || echo '[]')
    ccount=$(echo "$CHILDREN_JSON" | jq 'if type == "array" then length elif .bones then (.bones | length) else 0 end' 2>/dev/null || echo "0")
    cclosed=$(echo "$CHILDREN_JSON" | jq '[if type == "array" then .[] elif .bones then .bones[] else empty end | select(.state == "done")] | length' 2>/dev/null || echo "0")

    if [[ "$mission_label" == "A" ]]; then
      MISSION_A_CHILDREN=$ccount; MISSION_A_CLOSED=$cclosed
    else
      MISSION_B_CHILDREN=$ccount; MISSION_B_CLOSED=$cclosed
    fi

    # Check mission bone state
    mstate=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$mbone" --format json 2>/dev/null | jq -r '.[0].state // "unknown"' 2>/dev/null || echo "unknown")

    if [[ "$mission_label" == "A" ]]; then
      [[ "$mstate" != "$MISSION_A_STATUS" ]] && LAST_ACTIVITY_TIME=$(date +%s)
      MISSION_A_STATUS=$mstate
    else
      [[ "$mstate" != "$MISSION_B_STATUS" ]] && LAST_ACTIVITY_TIME=$(date +%s)
      MISSION_B_STATUS=$mstate
    fi

    echo "  Mission $mission_label ($mbone): $mstate — children $cclosed/$ccount"
  done

  # Check merge count (coord:merge messages)
  NEW_MERGE_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 2>/dev/null | grep -ci "coord:merge\|Merged" || true)
  NEW_MERGE_COUNT=${NEW_MERGE_COUNT:-0}
  if [[ "$NEW_MERGE_COUNT" -gt "$MERGE_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    MERGE_COUNT=$NEW_MERGE_COUNT
  fi
  echo "  Merges detected: $MERGE_COUNT"

  # Check if both missions are closed
  if [[ "$MISSION_A_STATUS" == "done" && "$MISSION_B_STATUS" == "done" ]]; then
    echo "  BOTH MISSIONS DONE — eval complete!"
    ALL_MISSIONS_CLOSED_TIME=$ELAPSED
    PHASE_TIMES+="all_closed=${ALL_MISSIONS_CLOSED_TIME}s\n"
    FINAL_STATUS="completed-still-running"

    # Grace period for agents to exit
    for WAIT_I in 1 2 3 4; do
      sleep 15
      REMAINING=$(vessel list --format json 2>/dev/null | jq '.agents | length' 2>/dev/null || echo "0")
      if [[ "$REMAINING" -eq 0 ]]; then
        FINAL_STATUS="completed"
        break
      fi
      echo "  Waiting for agents to exit... (${WAIT_I}/4)"
    done
    break
  fi

  # Check if at least one mission is closed (partial success)
  if [[ "$MISSION_A_STATUS" == "done" || "$MISSION_B_STATUS" == "done" ]]; then
    echo "  One mission closed, waiting for second..."
  fi

  # Check bus activity
  MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 2>/dev/null | wc -l || echo "0")
  if [[ "$MSG_COUNT" -gt "$LAST_MSG_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_MSG_COUNT=$MSG_COUNT
  fi

  # Stuck detection
  IDLE_TIME=$(( $(date +%s) - LAST_ACTIVITY_TIME ))
  if [[ $IDLE_TIME -ge $STUCK_THRESHOLD ]]; then
    echo "  WARNING: No activity for ${IDLE_TIME}s (threshold: ${STUCK_THRESHOLD}s)"
    ALL_AGENTS=$(echo "$BOTTY_JSON" | jq '.agents | length' 2>/dev/null || echo "0")
    if [[ "$ALL_AGENTS" -eq 0 ]]; then
      echo "  All agents exited — marking as agent-exited"
      FINAL_STATUS="agent-exited"
      break
    fi
  fi
done

echo ""
echo "--- Final status: $FINAL_STATUS ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Lead logs
for lkey in "${!LEAD_SLOTS[@]}"; do
  orig_id="${LEAD_SLOTS[$lkey]}"
  vessel tail "$orig_id" -n 500 > "$ARTIFACTS/agent-${lkey}.log" 2>/dev/null || \
    echo "(already exited)" > "$ARTIFACTS/agent-${lkey}.log"
  echo "  lead log: $ARTIFACTS/agent-${lkey}.log"
done

# Non-slotted dev log (if present)
vessel tail "$FUTIL_DEV" -n 500 > "$ARTIFACTS/agent-${FUTIL_DEV}.log" 2>/dev/null || true

# Router logs
for RNAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]? | select(.id | test("router")) | .id' 2>/dev/null || true); do
  SAFE="${RNAME//\//_}"
  vessel tail "$RNAME" -n 500 > "$ARTIFACTS/agent-${SAFE}.log" 2>/dev/null || true
  echo "  router log: $ARTIFACTS/agent-${SAFE}.log"
done

# Worker logs (capture remaining)
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

# Channel history
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 > "$ARTIFACTS/channel-futil-history.log" 2>/dev/null || true
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 --format json > "$ARTIFACTS/channel-futil-history.json" 2>/dev/null || true
echo "  channel: $ARTIFACTS/channel-futil-history.log"

# Mission bone states
cd "$PROJECT_DIR"
for mbone in "$MISSION_A_BONE" "$MISSION_B_BONE"; do
  [[ -z "$mbone" ]] && continue
  BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn show "$mbone" --format json > "$ARTIFACTS/mission-${mbone}-state.json" 2>/dev/null || true
done

# All bones
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- bn list --all --format json > "$ARTIFACTS/all-bones-state.json" 2>/dev/null || true

# Workspace state
maw ws list --format json > "$ARTIFACTS/workspace-state.json" 2>/dev/null || true

# Claims state
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus claims list --format json > "$ARTIFACTS/claims-state.json" 2>/dev/null || true

# jj log for divergent commit check
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" maw exec default -- jj log --no-graph -T 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' > "$ARTIFACTS/jj-log.txt" 2>/dev/null || true

# Final status file
cat > "$ARTIFACTS/final-status.txt" << EOF
FINAL_STATUS=$FINAL_STATUS
MISSION_A_BONE=${MISSION_A_BONE:-none}
MISSION_B_BONE=${MISSION_B_BONE:-none}
MISSION_A_STATUS=$MISSION_A_STATUS
MISSION_B_STATUS=$MISSION_B_STATUS
MISSION_A_CHILDREN=$MISSION_A_CHILDREN
MISSION_A_CLOSED=$MISSION_A_CLOSED
MISSION_B_CHILDREN=$MISSION_B_CHILDREN
MISSION_B_CLOSED=$MISSION_B_CLOSED
LEAD_SLOT_COUNT=$LEAD_SLOT_COUNT
WORKER_COUNT=$KNOWN_WORKER_COUNT
MERGE_COUNT=$MERGE_COUNT
EOF

# Lead slot names
{
  echo "LEAD_SLOTS:"
  for lkey in "${!LEAD_SLOTS[@]}"; do
    echo "  ${LEAD_SLOTS[$lkey]}"
  done
  echo "WORKERS:"
  for wkey in "${!KNOWN_WORKERS[@]}"; do
    echo "  ${KNOWN_WORKERS[$wkey]}"
  done
} >> "$ARTIFACTS/final-status.txt"

echo -e "$PHASE_TIMES" > "$ARTIFACTS/phase-times.log" 2>/dev/null || true

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
for AGENT_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  vessel kill "$AGENT_NAME" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/multi-lead-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== Multi-Lead Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status: $FINAL_STATUS"
echo "  Mission A ($MISSION_A_BONE): $MISSION_A_STATUS — children $MISSION_A_CLOSED/$MISSION_A_CHILDREN done"
echo "  Mission B ($MISSION_B_BONE): $MISSION_B_STATUS — children $MISSION_B_CLOSED/$MISSION_B_CHILDREN done"
echo "  Lead slots discovered: $LEAD_SLOT_COUNT"
echo "  Workers discovered: $KNOWN_WORKER_COUNT"
echo "  Merges detected: $MERGE_COUNT"
echo "Elapsed: $(( $(date +%s) - START_TIME ))s"
echo ""
echo "Phase timing:"
[[ -n "$FIRST_LEAD_TIME" ]] && echo "  first lead: ${FIRST_LEAD_TIME}s"
[[ -n "$SECOND_LEAD_TIME" ]] && echo "  second lead: ${SECOND_LEAD_TIME}s"
[[ -n "$FIRST_MISSION_FOUND_TIME" ]] && echo "  first mission found: ${FIRST_MISSION_FOUND_TIME}s"
[[ -n "$SECOND_MISSION_FOUND_TIME" ]] && echo "  second mission found: ${SECOND_MISSION_FOUND_TIME}s"
[[ -n "$FIRST_WORKER_TIME" ]] && echo "  first worker: ${FIRST_WORKER_TIME}s"
[[ -n "$ALL_MISSIONS_CLOSED_TIME" ]] && echo "  all missions closed: ${ALL_MISSIONS_CLOSED_TIME}s"
echo ""
echo "Artifacts: $ARTIFACTS/"
echo "EVAL_DIR=$EVAL_DIR"
echo ""
