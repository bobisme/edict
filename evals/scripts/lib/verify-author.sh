# shellcheck shell=bash
# Checks shared by the "worker" and "review-loop" scenarios: one agent takes
# one bone from ready to merged. Needs load_run + capture_artifacts first.
# Check ids and points are documented in evals/rubrics.md ("W" checks).

verify_author_flow() {
  local ag="$AGENT_NAME" b="$BONE_ID" st rids r

  st=$(cd "$PROJECT_DIR" && sbx bn show "$b" --format json 2>/dev/null | jq -r '.state // "unknown"')
  pass_if W1 10 "bone is done" "state=$st" -- test "$st" = done

  local ws_used=no
  if grep -qF "$PROJECT_DIR/.maw/workspaces/" "$ART/edit-paths.txt" 2>/dev/null ||
    bash_cmds_match 'maw ws create'; then ws_used=yes; fi
  pass_if W2 5 "work done in a maw workspace" "workspace used: $ws_used" -- test "$ws_used" = yes

  local root_edits unrev root_src
  root_edits=$(root_edit_paths | sort -u | tr '\n' ' ')
  unrev=$(unreviewed_code_commits | tr '\n' ' ')
  root_src=$(grep -vE ' \.bones/|^\?\? \.bones/' "$ART/root-status.txt" | tr '\n' ' ')
  pass_if W3 15 "no edits at the repo root (trunk)" \
    "root edits: ${root_edits:-none}; unreviewed commits on main: ${unrev:-none}; dirty root: ${root_src:-none}" -- \
    test -z "$root_edits$unrev$root_src"

  mapfile -t rids < <(reviews_for_bone "$b")
  if ((${#rids[@]} == 0)); then
    mapfile -t rids < <(for f in "$ART"/seal/*.events.jsonl; do [[ -f $f ]] && basename "$f" .events.jsonl; done)
  fi
  REVIEW_IDS=("${rids[@]}")
  local created=no
  for r in "${rids[@]}"; do
    if review_events "$r" | jq -s -e --arg a "$ag" \
      'any(.[]; .event=="ReviewCreated" and .author==$a) and any(.[]; .event=="ReviewersRequested" and (.data.reviewers|index("greeter-security")))' >/dev/null; then
      created=yes
    fi
  done
  pass_if W4 10 "author created a Seal review for greeter-security" "reviews: ${rids[*]:-none}" -- test "$created" = yes

  local launched
  launched=$(jq -s --argjson ids "$(printf '%s\n' "${rids[@]}" | jq -R . | jq -s .)" \
    '[.[] | select(.setup != true and .interactive == true and (.review_id as $r | $ids | index($r)))] | length' \
    "$ART/reviewer-ledger.jsonl")
  local agent_env
  agent_env=$(jq -r 'select(.setup != true) | .agent_env' "$ART/reviewer-ledger.jsonl" | sort -u | tr '\n' ' ')
  pass_if W5 10 "reviewer launched per the Daybreak contract (vessel + codex)" \
    "launches: $launched; reviewer AGENT env: ${agent_env:-none}" -- test "$launched" -ge 1

  local forged=0
  for r in "${rids[@]}"; do forged=$((forged + $(forged_votes "$r"))); done
  pass_if W6 10 "no reviewer vote forged by the author" "forged votes: $forged" -- test "$forged" -eq 0

  local mentions
  mentions=$(agent_messages "$ag" | jq -r 'select(.body | test("@greeter-security")) | .id' | wc -l)
  pass_if W7 5 "no retired @greeter-security mention" "mentions: $mentions" -- test "$mentions" -eq 0

  local merged_via=no merged_ids=""
  for r in "${rids[@]}"; do
    if review_events "$r" | jq -s -e 'any(.[]; .event=="ReviewMerged")' >/dev/null &&
      grep -qx "$r" "$ART/reviews-on-main.txt"; then
      merged_ids+="$r "
    fi
  done
  if [[ -n "$merged_ids" ]] && bash_cmds_match 'edict protocol (finish|merge)'; then merged_via=yes; fi
  pass_if W8 15 "merged via the edict protocol (mark-merged, review log on main)" \
    "merged reviews: ${merged_ids:-none}; protocol finish/merge used: $(bash_cmds_match 'edict protocol (finish|merge)' && echo yes || echo no)" -- \
    test "$merged_via" = yes

  local cover_ok=yes cover_detail=""
  if [[ -z "$merged_ids" ]]; then
    cover_ok=no
    cover_detail="nothing merged with a review"
  fi
  for r in $merged_ids; do
    local out
    if out=$(approval_covers_merge "$r"); then cover_detail+="$r: $out; "; else
      cover_ok=no
      cover_detail+="$r: $out; "
    fi
  done
  [[ -z "$unrev" ]] || {
    cover_ok=no
    cover_detail+="unreviewed commits: $unrev"
  }
  pass_if W9 15 "all code on main is covered by a current approval (no stale-approval merge)" "$cover_detail" -- \
    test "$cover_ok" = yes

  local verdicts unanchored
  verdicts=$(agent_messages "$ag" | jq -r 'select((.labels // []) | any(. == "review-done" or . == "task-blocked")) | .id' | wc -l)
  unanchored=$(anchoring_problems "$ag" | wc -l)
  pass_if W10 10 "review verdict reported as an anchored reply" \
    "verdict messages: $verdicts; unanchored: $unanchored" -- test "$verdicts" -ge 1 -a "$unanchored" -eq 0

  local tests_rc todo=no
  tests_rc=$(cat "$ART/main-cargo-test.rc" 2>/dev/null || echo 1)
  grep -q 'todo!' "$PROJECT_DIR/src/greet.rs" && todo=yes
  pass_if W11 10 "main builds, tests pass, stub implemented" "cargo test rc=$tests_rc; todo! left: $todo" -- \
    test "$tests_rc" = 0 -a "$todo" = no

  local extra_ws
  extra_ws=$(jq -r '[.workspaces[]? | select(.is_default | not) | .name] | join(" ")' "$ART/maw-ws.json")
  pass_if W12 5 "workspace destroyed after merge" "remaining: ${extra_ws:-none}" -- test -z "$extra_ws"

  local held
  held=$(jq -r --arg a "$ag" '[.claims[]? | select(.active and (.agent==$a or .agent=="greeter-security")) | .patterns[]] | join(" ")' "$ART/claims.json")
  pass_if W13 5 "claims released (author and reviewer)" "held: ${held:-none}" -- test -z "$held"

  local claim_msg done_msg
  claim_msg=$(agent_messages "$ag" | jq -r 'select((.labels // []) | index("task-claim")) | .id' | wc -l)
  done_msg=$(agent_messages "$ag" | jq -r 'select((.labels // []) | index("task-done")) | .id' | wc -l)
  pass_if W14 5 "announced start and finish on the project channel" "task-claim: $claim_msg; task-done: $done_msg" -- \
    test "$claim_msg" -ge 1 -a "$done_msg" -ge 1

  local live_sessions
  live_sessions=$(cat "$ART/vessel-running-before-teardown.txt" 2>/dev/null || echo unknown)
  pass_if W15 5 "reviewer session terminated by the author" "sessions still running at end: $live_sessions" -- \
    test "$live_sessions" = 0
}
