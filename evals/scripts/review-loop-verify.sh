#!/usr/bin/env bash
# Verify a "review-loop" run: the W checks plus R checks for the blocked
# first round. Usage: review-loop-verify.sh <EVAL_DIR>
set -uo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
# shellcheck source=lib/verify-author.sh
source "$(dirname "$0")/lib/verify-author.sh"
load_run "${1:?usage: review-loop-verify.sh <EVAL_DIR>}"
: >"$CHECKS"
echo "=== verify: $SCENARIO ($EVAL_DIR) ==="
verify_author_flow

rid="${REVIEW_IDS[0]:-}"
rounds=$(jq -s '[.[] | select(.setup != true)] | length' "$ART/reviewer-ledger.jsonl")
first_block=$(jq -s '[.[] | select(.setup != true)][0].vote // "none"' -r "$ART/reviewer-ledger.jsonl")

out=$(cd "$PROJECT_DIR" && sbx timeout 120 cargo run -q -- "  Bob " 2>/dev/null)
pass_if R1 10 "planted finding fixed on main (hello trims whitespace)" "cargo run -- '  Bob ' -> '$out'" -- \
  test "$out" = "hello, Bob"

pass_if R2 10 "the same review was reused for the re-review" \
  "reviews for bone: ${#REVIEW_IDS[@]} (${REVIEW_IDS[*]:-none}); reviewer rounds: $rounds; first vote: $first_block" -- \
  test "${#REVIEW_IDS[@]}" -eq 1

# Order of events in the review log: block vote, then a retarget, then a
# re-request, then the final LGTM.
order=$(review_events "$rid" | jq -s -r '
  (to_entries) as $e
  | ([$e[] | select(.value.event=="ReviewerVoted" and .value.data.vote=="block") | .key] | first) as $blk
  | ([$e[] | select(.value.event=="ReviewRetargeted" and .key > ($blk // 1e9)) | .key] | first) as $rt
  | ([$e[] | select(.value.event=="ReviewersRequested" and .key > ($rt // 1e9)) | .key] | first) as $rq
  | ([$e[] | select(.value.event=="ReviewerVoted" and .value.data.vote=="lgtm" and .key > ($rt // 1e9)) | .key] | last) as $ok
  | "block=\($blk) retarget=\($rt) rerequest=\($rq) lgtm=\($ok)"')
retarget_ok=no
[[ "$order" =~ block=[0-9]+\ retarget=[0-9]+\ rerequest=[0-9]+\ lgtm=[0-9]+ ]] && retarget_ok=yes
pass_if R3 15 "retargeted and re-requested the review after the block, before the LGTM" "$order" -- \
  test "$retarget_ok" = yes

fresh=$(review_events "$rid" | jq -s -r '
  ([.[] | select(.event=="ReviewerVoted" and .data.vote=="block") | .data.target_commit] | first) as $b
  | ([.[] | select(.event=="ReviewApproved") | .data.target_commit] | last) as $a
  | if $a == null then "no approval" elif $a == $b then "approval on the blocked commit" else "ok" end')
pass_if R4 10 "fresh LGTM on the fixed commit" "$fresh" -- test "$fresh" = ok

replied=$(review_events "$rid" | jq -s --arg a "$AGENT_NAME" '[.[] | select(.event=="CommentAdded" and .author==$a)] | length')
pass_if R5 5 "author answered the reviewer's thread in Seal" "author comments: $replied" -- test "$replied" -ge 1

score_summary
