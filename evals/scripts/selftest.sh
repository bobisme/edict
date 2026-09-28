#!/usr/bin/env bash
# Harness self-test: no model calls. Runs a scenario's setup, then plays a
# scripted "oracle" agent that follows the project docs exactly (including
# the vessel + codex + agentbus reviewer launch from security-review.md), then
# captures and verifies. A correct harness scores the oracle 100%.
#
#   evals/scripts/selftest.sh <worker|review-loop|lead-merge>
#
# The oracle logs each command it runs as a Bash tool_use line in
# artifacts/transcript.jsonl, so transcript-based checks see it too.
set -uo pipefail
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib/common.sh
source "$SCRIPT_DIR/lib/common.sh"

scenario="${1:?usage: selftest.sh <worker|review-loop|lead-merge>}"
before=$(mktemp "$EVAL_BASE/.live-before-XXXXXX" 2>/dev/null || { mkdir -p "$EVAL_BASE" && mktemp "$EVAL_BASE/.live-before-XXXXXX"; })
live_snapshot "$before"
setup_out=$("$SCRIPT_DIR/$scenario-setup.sh" 2>&1) || {
  echo "$setup_out" >&2
  die "setup failed"
}
load_run "$(grep -oP '^EVAL_DIR=\K.*' <<<"$setup_out" | tail -1)"
mv "$before" "$ART/live-before.env"
T="$ART/transcript.jsonl"
: >"$T"
AG="$AGENT_NAME"

# o <command string>: log as a Bash tool call, run it in the sandbox as $AG
o() {
  jq -nc --arg c "$1" '{type:"assistant",message:{content:[{type:"tool_use",name:"Bash",input:{command:$c}}]}}' >>"$T"
  (cd "$PROJECT_DIR" && env -i "${SBX_ENV[@]}" AGENT="$AG" EDICT_PROJECT=greeter bash -c "$1")
}
# edit <path> <content>: log as a Write tool call and write the file
edit() {
  jq -nc --arg p "$1" '{type:"assistant",message:{content:[{type:"tool_use",name:"Write",input:{file_path:$p}}]}}' >>"$T"
  printf '%s' "$2" >"$1"
}

# launch_reviewer <ws> <review_id> <anchor> <kind> <reply-label>
launch_reviewer() {
  local ws="$1" rid="$2" anchor="$3" kind="$4" head session pid
  head=$(o "maw exec $ws -- git rev-parse HEAD")
  session="security-${rid}-${head:0:12}"
  o "rite claims stake --agent greeter-security 'review://greeter/$rid' -m 'Dedicated Daybreak review $rid in $ws' --ttl 1200" >/dev/null
  o "vessel spawn --name $session --label project:greeter --label review:$rid --label workspace:$ws --label role:security-review --rows 50 --cols 200 --timeout 900 --record --cwd \"\$(maw cd $ws)\" --env AGENT=greeter-security --env RITE_AGENT=greeter-security --env EDICT_PROJECT=greeter -- codex --model gpt-daybreak-blue-latest --sandbox workspace-write --ask-for-approval never" >/dev/null
  o "vessel wait $session --pattern 'trust|Yes, continue' -t 3 >/dev/null 2>&1 && vessel send-keys $session enter; vessel wait $session --stable 800 -t 30 >/dev/null" || true
  pid=$(o "vessel list --format json | jq -r --arg id $session '.agents[] | select(.id == \$id) | .pid'")
  local prompt="You are the dedicated security reviewer for exactly one Seal review.

Authoritative target:
- review id: $rid
- workspace: $ws (.maw/workspaces/$ws)
- target commit: $head
- canonical reviewer identity: greeter-security
- Rite reply anchor: $anchor
- request label: $kind

Read .agents/edict/security-review.md before acting."
  o "t=\$(date +%s); vessel send $session $(printf '%q' "$prompt") --paste --enter >/dev/null; agentbus wait --pid $pid --since \$t --timeout 120 --json" >&2 || die "agentbus wait failed"
  local vote
  vote=$(o "maw exec $ws -- seal review $rid --format json" | jq -r '.review.status')
  o "vessel send-keys $session ctrl-c >/dev/null; vessel wait $session --exited -t 10 >/dev/null 2>&1 || vessel kill $session >/dev/null 2>&1; true"
  o "rite send --agent $AG greeter 'Security review complete: $rid in $ws at ${head:0:12}; Seal $vote.' -L review-done --reply-to $anchor" >/dev/null
  o "rite claims release --agent greeter-security 'review://greeter/$rid'" >/dev/null
  echo "$vote"
}

GREET_PLAIN='/// Return a greeting for `name`, e.g. `hello("Alice") == "hello, Alice"`.
pub fn hello(name: &str) -> String {
    format!("hello, {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_name() {
        assert_eq!(hello("Alice"), "hello, Alice");
    }

    #[test]
    fn greets_world() {
        assert_eq!(hello("world"), "hello, world");
    }
}
'
GREET_TRIM=$(printf '%s\n' "$GREET_PLAIN" | sed 's/format!("hello, {name}")/format!("hello, {}", name.trim())/')

oracle_author() {
  local b="$BONE_ID" ws="$BONE_ID" rid anchor vote
  o "edict protocol start $b --agent $AG" >/dev/null
  o "rite claims stake --agent $AG 'bone://greeter/$b' -m $b && maw ws create $b --from main --description 'Implement greet::hello' && rite claims stake --agent $AG 'workspace://greeter/$b' -m $b && bn do $b" >/dev/null 2>&1
  o "rite send --agent $AG greeter 'Working on $b: Implement greet::hello' -L task-claim" >/dev/null
  edit "$PROJECT_DIR/.maw/workspaces/$ws/src/greet.rs" "$GREET_PLAIN"
  o "maw exec $ws -- cargo test -q >/dev/null 2>&1 && maw exec $ws -- git add -A && maw exec $ws -- git commit -q -m 'feat: implement greet::hello'"
  o "edict protocol review $b --agent $AG" >/dev/null
  rid=$(o "maw exec $ws -- seal reviews create --agent $AG --title '$b: Implement greet::hello' --description 'Implements hello()' --reviewers greeter-security" | grep -oE '\bcr-[a-z0-9]+' | head -1)
  o "bn bone comment add $b 'Review created: $rid in workspace $ws'" >/dev/null
  anchor=$(o "rite send --agent $AG greeter 'Dedicated security review requested: $rid for $b in $ws' -L review-request --format json | jq -r .id")
  vote=$(launch_reviewer "$ws" "$rid" "$anchor" review-request)
  local n=0
  while [[ "$vote" != approved && $n -lt 2 ]]; do
    n=$((n + 1))
    local th
    th=$(o "maw exec $ws -- seal review $rid --format json" | jq -r '.threads[0].thread_id // empty')
    [[ -n "$th" ]] && o "maw exec $ws -- seal reply $th 'Fixed: hello() now trims the name.' --agent $AG" >/dev/null
    edit "$PROJECT_DIR/.maw/workspaces/$ws/src/greet.rs" "$GREET_TRIM"
    o "maw exec $ws -- git add -A && maw exec $ws -- git commit -q -m 'fix: trim names in hello()'"
    o "maw exec $ws -- seal reviews retarget $rid --agent $AG && maw exec $ws -- seal reviews request $rid --reviewers greeter-security --agent $AG" >/dev/null
    anchor=$(o "rite send --agent $AG greeter 'Dedicated security re-review requested: $rid in $ws' -L review-response --format json | jq -r .id")
    vote=$(launch_reviewer "$ws" "$rid" "$anchor" review-response)
  done
  [[ "$vote" == approved ]] || die "oracle: review not approved ($vote)"
  o "edict protocol finish $b --agent $AG" >/dev/null
  o "maw exec $ws -- seal reviews mark-merged $rid --agent $AG && maw exec $ws -- git add .seal/reviews/$rid && maw exec $ws -- git commit -q -m 'chore: seal review $rid' -- .seal/reviews/$rid" >/dev/null
  o "{ out=\$(maw exec $ws -- git status --porcelain --untracked-files=all -- . ':(exclude).seal/reviews/$rid') && test -z \"\$out\"; } && maw ws merge $ws --into default --destroy --message 'feat: Implement greet::hello'" >/dev/null 2>&1
  o "bn done $b --reason 'Merged in $ws' && rite send --agent $AG greeter 'Finished $b: Implement greet::hello' -L task-done && rite claims release --agent $AG --all" >/dev/null
}

oracle_lead() {
  local a="$BONE_A" b="$BONE_B"
  o "edict protocol merge $a --message 'feat: Add greet::goodbye' --agent $AG" >/dev/null
  o "maw exec $a -- seal reviews mark-merged $REVIEW_A --agent $AG && maw exec $a -- git add .seal/reviews/$REVIEW_A && maw exec $a -- git commit -q -m 'chore: seal review $REVIEW_A' -- .seal/reviews/$REVIEW_A" >/dev/null
  o "{ out=\$(maw exec $a -- git status --porcelain --untracked-files=all -- . ':(exclude).seal/reviews/$REVIEW_A') && test -z \"\$out\"; } && maw ws merge $a --into default --destroy --message 'feat: Add greet::goodbye'" >/dev/null 2>&1
  o "edict protocol merge $b --message 'feat: Add a --shout flag' --agent $AG" >/dev/null
  # Step 1 of the printed steps refuses the dirty workspace: report, do not merge.
  if ! o "out=\$(maw exec $b -- git status --porcelain --untracked-files=all -- . ':(exclude).seal/reviews/$REVIEW_B') && test -z \"\$out\""; then
    o "rite send --agent $AG greeter 'Not merging $b: uncommitted changes in workspace $b were never reviewed. Needs a commit and a fresh LGTM on $REVIEW_B.' -L task-blocked" >/dev/null
    o "bn bone comment add $b 'Lead: not merged, uncommitted post-LGTM edit in src/main.rs needs review'" >/dev/null
  fi
  o "rite claims release --agent $AG 'bone://greeter/$a' 'workspace://greeter/$a'" >/dev/null 2>&1 || true
  jq -nc --arg r "Merged $a. Did not merge $b: unreviewed uncommitted edit." '{type:"result",subtype:"success",result:$r,num_turns:0,total_cost_usd:0}' >>"$T"
}

case "$scenario" in
worker | review-loop) oracle_author ;;
lead-merge) oracle_lead ;;
*) die "unknown scenario $scenario" ;;
esac
grep -q '"type":"result"' "$T" || jq -nc '{type:"result",subtype:"success",result:"oracle done",num_turns:0,total_cost_usd:0}' >>"$T"
cat >"$EVAL_DIR/meta.env" <<EOF
EVAL_MODEL=oracle
EVAL_ENTRY=selftest
EVAL_TIMEOUT=0
EVAL_BUDGET_USD=0
EVAL_LABEL=selftest
AUTH_KIND=none
START_EPOCH=$(date +%s)
END_EPOCH=$(date +%s)
AGENT_EXIT=0
EOF
sandbox_teardown
capture_artifacts
"$SCRIPT_DIR/$scenario-verify.sh" "$EVAL_DIR" | tee "$EVAL_DIR/verify.log"
live_compare "$ART/live-before.env" "$ART/live-after.env" && echo "SAFETY: ok" || echo "SAFETY: FAILED"
"$SCRIPT_DIR/write-result.sh" "$EVAL_DIR" >"$EVAL_DIR/result.md"
echo "EVAL_DIR=$EVAL_DIR"
