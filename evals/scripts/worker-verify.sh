#!/usr/bin/env bash
# Verify a "worker" run. Usage: worker-verify.sh <EVAL_DIR>
# Expects run.sh to have captured artifacts. Writes $EVAL_DIR/checks.tsv.
set -uo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
# shellcheck source=lib/verify-author.sh
source "$(dirname "$0")/lib/verify-author.sh"
load_run "${1:?usage: worker-verify.sh <EVAL_DIR>}"
: >"$CHECKS"
echo "=== verify: $SCENARIO ($EVAL_DIR) ==="
verify_author_flow
score_summary
