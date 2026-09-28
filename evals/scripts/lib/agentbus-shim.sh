#!/usr/bin/env bash
# Eval agentbus shim. Setup copies this file to $EVAL_DIR/bin/agentbus.
# Only `agentbus wait` is supported: it blocks until the reviewer shim
# (reviewer-codex-shim.sh) publishes a result newer than --since whose pid is
# --pid or a descendant of it. Exit codes follow the real agentbus:
#   0 turn ended, 4 timeout, 1 usage/resolution error.
set -u

SELF=$(readlink -f "$0")
EVAL_DIR=$(cd "$(dirname "$SELF")/.." && pwd)
RDIR="$EVAL_DIR/reviewer"
mkdir -p "$RDIR/results"
printf '%s agentbus-shim %s\n' "$(date -Iseconds)" "$*" >>"$RDIR/agentbus.log"

cmd="${1:-}"
[[ $# -gt 0 ]] && shift
if [[ "$cmd" != "wait" ]]; then
  echo "agentbus (eval sandbox): only 'agentbus wait' is available here" >&2
  exit 1
fi

pid="" session="" cwd="" since="" timeout=600 json=0
while [[ $# -gt 0 ]]; do
  case "$1" in
  --pid) pid="$2"; shift 2 ;;
  --session) session="$2"; shift 2 ;;
  --cwd) cwd="$2"; shift 2 ;;
  --since) since="$2"; shift 2 ;;
  --timeout) timeout="$2"; shift 2 ;;
  --json) json=1; shift ;;
  *) shift ;;
  esac
done
if [[ -z "$pid$session$cwd" ]]; then
  echo "agentbus: wait needs --session, --pid or --cwd" >&2
  exit 1
fi
since="${since:-$(date +%s)}"
since="${since%%.*}"

# Is $2 equal to $1 or a descendant of it?
descends() {
  local want="$1" p="$2" i=0
  while [[ -n "$p" && "$p" != 0 && $i -lt 32 ]]; do
    [[ "$p" == "$want" ]] && return 0
    p=$(awk '{print $4}' "/proc/$p/stat" 2>/dev/null)
    i=$((i + 1))
  done
  return 1
}

deadline=$(($(date +%s) + ${timeout%%.*}))
while (($(date +%s) < deadline)); do
  for f in $(ls -1 "$RDIR/results/"*.json 2>/dev/null | sort); do
    ts=$(jq -r .ts "$f")
    fpid=$(jq -r .pid "$f")
    ((ts >= since)) || continue
    if [[ -n "$pid" ]] && ! descends "$pid" "$fpid"; then continue; fi
    msg=$(jq -r .message "$f")
    printf '%s agentbus-shim matched %s\n' "$(date -Iseconds)" "$f" >>"$RDIR/agentbus.log"
    if ((json)); then
      jq -c --arg sid "eval-shim-$fpid" \
        '{state:"done",session_id:$sid,pid:.pid,ended_at:.ts,last_message:.message,turn:{final_message:.message}}' "$f"
    else
      printf '%s\n' "$msg"
    fi
    exit 0
  done
  sleep 1
done
printf '%s agentbus-shim timeout\n' "$(date -Iseconds)" >>"$RDIR/agentbus.log"
((json)) && echo '{"state":"timeout"}'
exit 4
