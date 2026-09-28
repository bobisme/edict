#!/usr/bin/env bash
# Scenario "review-loop" (R4 with a blocking review): same project, bone and
# prompt as "worker", but the scripted reviewer BLOCKS the first round with a
# planted finding (hello() must trim whitespace) and LGTMs only once the fix
# is present. The agent must fix in the same workspace, retarget and
# re-request the SAME review, relaunch the reviewer, and merge only after the
# fresh LGTM.
set -euo pipefail
export EVAL_REVIEWER_POLICY=block-trim EVAL_SCENARIO_NAME=review-loop
exec "$(dirname "$0")/worker-setup.sh"
