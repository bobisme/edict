#!/usr/bin/env bash
set -euo pipefail

# E11-L4 Mission Eval — Orchestrator
# Sends !mission message to futil channel, polls mission lifecycle
# (decomposition → worker dispatch → checkpoint → synthesis), captures artifacts.
#
# Expected flow:
# 1. Router hook fires → respond.mjs → routes !mission
# 2. respond.mjs creates mission bone → execs into dev-loop with BOTBOX_MISSION
# 3. Dev-loop decomposes mission into child bones
# 4. Dev-loop dispatches workers (futil-dev/<random>) for independent children
# 5. Workers implement subcommands in parallel
# 6. Dev-loop monitors via checkpoints
# 7. Dev-loop synthesizes and closes mission

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OVERALL_TIMEOUT=${E11_TIMEOUT:-1800}  # 30 minutes default
POLL_INTERVAL=30                       # seconds between status checks
STUCK_THRESHOLD=300                    # 5 minutes without progress = stuck

echo "=== E11-L4 Mission Eval ==="
echo "Starting at $(date)"
echo "Overall timeout: ${OVERALL_TIMEOUT}s"
echo ""

# --- Phase 0: Setup ---
echo "--- Running setup ---"
SETUP_OUTPUT=$("$SCRIPT_DIR/e11-l4-setup.sh" 2>&1) || {
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

FUTIL_ROUTER_OK=false
if echo "$HOOKS" | grep -qi "futil.*claim\|claim.*futil"; then
  FUTIL_ROUTER_OK=true
  echo "  Futil router hook: OK"
fi

if ! $FUTIL_ROUTER_OK; then echo "WARNING: No futil router hook found"; fi
echo "--- hooks: OK ---"
echo ""

# --- Build mission spec ---
MISSION_SPEC='!mission Implement all futil subcommands and shared error handling
Outcome: A working futil CLI where all three subcommands (stats, search, convert) produce correct output with all flags working, consistent error handling via the shared error module, and comprehensive unit tests.

## Architecture

The project has 4 source files with a dependency structure:
- src/error.rs — shared FutilError type + helper functions (ALL subcommands depend on this)
- src/stats.rs — file statistics with multiple output modes (depends on error.rs)
- src/search.rs — regex search with context, modes, and multi-file support (depends on error.rs)
- src/convert.rs — format conversion between JSON, CSV, JSONL with field ops (depends on error.rs)
- src/main.rs — clap dispatch (already wired with all flags, do NOT modify)

Each subcommand module has a todo!() stub. The error module has the type skeleton but needs three helper functions implemented. Read the doc comments in each file for the full specification.

## Component specs

### 1. src/error.rs — shared error handling and utilities (implement FIRST)
The FutilError enum is defined with all variants. Implement three helper functions:
  - validate_file(path) → Result<String, FutilError>: check file exists, read to string, return contents
  - detect_format(path) → Result<&str, FutilError>: check extension (.json→"json", .csv→"csv", .jsonl→"jsonl")
  - write_output(content, output_path) → Result<(), FutilError>: write to file if Some, or print to stdout
Add unit tests for all three functions (happy path + error cases).

### 2. src/stats.rs — futil stats [OPTIONS] <paths...>
Comprehensive file statistics for one or more files. Requirements:
  - Count lines, words, bytes for each file
  - With multiple files: per-file rows PLUS a "total:" summary row
  - --json: output as JSON array of objects with path/lines/words/bytes/chars fields
  - --chars: include character count (count Unicode chars, distinct from byte count)
  - --top-words N: show N most frequent words (case-insensitive), sorted by frequency desc
    In plain mode: append "top words: word1(N), word2(N), ..." after stats
    In JSON mode: add "top_words": [{"word":"the","count":5},...] array
  - Use error::validate_file for all file loading
  - Add unit tests: basic counts, multi-file totals, JSON output, top-words, UTF-8 chars

### 3. src/search.rs — futil search [OPTIONS] <pattern> <paths...>
Regex search with grep-like features across one or more files. Requirements:
  - Basic: print matching lines with line numbers "N: <line>"
  - Multi-file: prefix with filename "<path>:N: <line>"
  - -A N: show N lines of context after each match
  - -B N: show N lines of context before each match
  - -C N: shorthand for -A N -B N
  - Context lines printed as "N- <line>" (dash not colon)
  - Non-adjacent match groups separated by "--" line
  - -i: case-insensitive matching (wrap pattern in (?i))
  - -c: count-only mode — print "<path>: N matches" per file
  - -l: files-only mode — print just filenames containing matches
  - -v: invert match — print lines that do NOT match
  - --json: output as JSON array [{"path","line_number","text","is_match"},...]
  - Use error::validate_file for file loading
  - Use FutilError::InvalidRegex for bad patterns (catch regex compilation error)
  - Add unit tests: basic match, context lines, case-insensitive, count, files-only, invert, multi-file

### 4. src/convert.rs — futil convert [OPTIONS] <input> --format <fmt>
Format conversion supporting JSON, CSV, and JSONL. Requirements:
  - Auto-detect input format from extension using error::detect_format
  - All 6 conversion pairs: json↔csv, json↔jsonl, csv↔jsonl
  - JSON→CSV: union of all object keys as headers, missing values as empty strings
  - CSV→JSON: each row becomes object, parse numeric strings to numbers automatically
  - JSONL→JSON and JSON→JSONL: collect/split line-delimited JSON
  - CSV↔JSONL: via intermediate representation
  - --fields f1,f2: select AND reorder output fields (error if field not in data)
  - --sort-by field: sort rows ascending by field value (string comparison, error if field missing)
  - --pretty: pretty-print JSON/JSONL output with indentation
  - --output path: write to file instead of stdout (use error::write_output)
  - Use error::validate_file for input loading, error::detect_format for format detection
  - Add unit tests: each format pair, field selection, sorting, pretty-print, numeric auto-conversion

## Dependencies between components
- error.rs BLOCKS stats.rs, search.rs, and convert.rs (they all use FutilError + helpers)
- stats.rs, search.rs, and convert.rs are INDEPENDENT of each other (can be done in parallel)
- Each subcommand is a substantial implementation (~100-200 lines) best done as separate tasks

## Test data
Sample data files in data/: sample.txt, words.txt, log.txt, sample.csv, sample.json, sample.jsonl, nested.json

Success metric: cargo test passes with comprehensive tests in each module; all subcommands produce correct output on sample data; all flags work as documented.
Constraints: Use existing dependencies only (clap, regex, serde, serde_json, csv, thiserror). Do not modify src/main.rs.
Stop criteria: All three subcommands fully working with all flags, comprehensive unit tests passing.'

# --- Send !mission message (triggers router hook → respond.mjs → dev-loop) ---
echo "--- Sending !mission to futil channel ($(date +%H:%M:%S)) ---"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus send --agent setup futil \
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

  DEV_RUNNING=$(echo "$BOTTY_JSON" | jq -r ".agents[] | select(.id == \"$FUTIL_DEV\") | .id" 2>/dev/null || echo "")
  RESPOND_RUNNING=$(echo "$BOTTY_JSON" | jq -r '.agents[] | select(.id | test("respond")) | .id' 2>/dev/null || echo "")

  if [[ -n "$DEV_RUNNING" ]]; then
    echo "  futil-dev: RUNNING"
    if [[ -z "$DEV_SPAWN_TIME" ]]; then
      DEV_SPAWN_TIME=$ELAPSED
      PHASE_TIMES+="dev_spawn=${DEV_SPAWN_TIME}s\n"
    fi
  else
    echo "  futil-dev: not running"
  fi

  if [[ -n "$RESPOND_RUNNING" ]]; then
    echo "  respond: RUNNING ($RESPOND_RUNNING)"
  fi

  # Discover workers (hierarchical names: futil-dev/<random>)
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
  # KNOWN_WORKERS keys are sanitized (/ → _), values are original worker IDs
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
        AD_RUNNING=$(echo "$BOTTY_FINAL" | jq -r ".agents[] | select(.id == \"$FUTIL_DEV\") | .id" 2>/dev/null || echo "")
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
  MSG_COUNT=$(BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 2>/dev/null | wc -l || echo "0")
  echo "  Channel messages: $MSG_COUNT"

  if [[ "$MSG_COUNT" -gt "$LAST_MSG_COUNT" ]]; then
    LAST_ACTIVITY_TIME=$(date +%s)
    LAST_MSG_COUNT=$MSG_COUNT
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
echo "--- Final status: futil-dev=$FINAL_STATUS_DEV ($(date +%H:%M:%S)) ---"
echo ""

# --- Capture artifacts ---
echo "--- Capturing artifacts ---"

# Dev agent log
vessel tail "$FUTIL_DEV" -n 500 > "$ARTIFACTS/agent-${FUTIL_DEV}.log" 2>/dev/null || \
  echo "(agent already exited, no tail available)" > "$ARTIFACTS/agent-${FUTIL_DEV}.log"
echo "  log: $ARTIFACTS/agent-${FUTIL_DEV}.log"

# Respond agent logs
for RESP_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]? | select(.id | test("respond")) | .id' 2>/dev/null || true); do
  vessel tail "$RESP_NAME" -n 500 > "$ARTIFACTS/agent-${RESP_NAME}.log" 2>/dev/null || true
  echo "  respond log: $ARTIFACTS/agent-${RESP_NAME}.log"
done
# Try common respond names that may have exited
for RNAME in "futil-respond" "respond"; do
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
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 > "$ARTIFACTS/channel-futil-history.log" 2>/dev/null || \
  echo "(no history)" > "$ARTIFACTS/channel-futil-history.log"
BOTBUS_DATA_DIR="$BOTBUS_DATA_DIR" bus history futil -n 200 --format json > "$ARTIFACTS/channel-futil-history.json" 2>/dev/null || \
  echo '{"messages":[]}' > "$ARTIFACTS/channel-futil-history.json"
echo "  channel: $ARTIFACTS/channel-futil-history.log"

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

# Final status file
WORKER_NAMES=""
if [[ $KNOWN_WORKER_COUNT -gt 0 ]]; then
  WORKER_NAMES=$(for wkey in "${!KNOWN_WORKERS[@]}"; do echo "${KNOWN_WORKERS[$wkey]}"; done | sort)
fi
cat > "$ARTIFACTS/final-status.txt" << EOF
FUTIL_DEV_STATUS=$FINAL_STATUS_DEV
MISSION_BONE=${MISSION_BONE:-none}
CHILD_COUNT=$CHILD_COUNT
CHILDREN_CLOSED=$CHILDREN_CLOSED
WORKER_NAMES=$WORKER_NAMES
EOF

echo -e "$PHASE_TIMES" > "$ARTIFACTS/phase-times.log" 2>/dev/null || true

echo ""

# --- Kill remaining agents ---
echo "--- Cleaning up agents ---"
vessel kill "$FUTIL_DEV" 2>/dev/null || true
for AGENT_NAME in $(vessel list --format json 2>/dev/null | jq -r '.agents[]?.id // empty' 2>/dev/null || true); do
  vessel kill "$AGENT_NAME" 2>/dev/null || true
done
echo "  All agents stopped."
echo ""

# --- Verification ---
echo "--- Running verify ($(date +%H:%M:%S)) ---"
"$SCRIPT_DIR/e11-l4-verify.sh" "$EVAL_DIR/.eval-env" || true

# --- Summary ---
echo ""
echo "========================================="
echo "=== E11-L4 Complete ($(date +%H:%M:%S)) ==="
echo "========================================="
echo ""
echo "Final status:"
echo "  futil-dev: $FINAL_STATUS_DEV"
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
echo "  cat $ARTIFACTS/agent-${FUTIL_DEV}.log"
echo "  cat $ARTIFACTS/channel-futil-history.log"
echo ""
