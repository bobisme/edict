#!/usr/bin/env bash
# Scenario "worker" (agent-loop / R4): one ready bone, one agent.
# The agent must pick the bone up, work in a maw workspace, request a Seal
# review, launch the reviewer as the project docs say, and merge through the
# edict protocol after the LGTM. The scripted reviewer votes LGTM at once.
#
# Env: EDICT_AGENTS_VARIANT (current|none|<file>), EVAL_BASE
# Prints EVAL_DIR=<dir> on success.
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"

POLICY="${EVAL_REVIEWER_POLICY:-lgtm}"
setup_base "${EVAL_SCENARIO_NAME:-worker}" todo "$POLICY"

BONE_ID=$(bone_create eval-setup "Implement greet::hello" \
  "src/greet.rs has a todo!() stub. Make hello(name) return \"hello, <name>\" so the existing tests pass.")
[[ "$BONE_ID" == bn-* ]] || die "bone create failed: $BONE_ID"
commit_seed "chore: seed bone $BONE_ID"
eval_env_set BONE_ID "$BONE_ID"

finish_setup greeter-dev "You are greeter-dev, working in the greeter project (the current directory). There is one ready bone. Pick it up and deliver it following this project's conventions. Stop when the bone is done and its work is merged, or when you are blocked."
