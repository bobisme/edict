#!/usr/bin/env bash
# Render the AGENTS.md managed block that a given edict build produces, for
# use as EDICT_AGENTS_VARIANT. Runs that build's `edict init` in a throwaway
# sandbox (same isolation as the evals), so nothing reaches live rite.
#
#   evals/scripts/render-variant.sh <edict-binary> <out.md>
#   e.g. cargo build --release (in the trimmed workspace), then
#        evals/scripts/render-variant.sh target/release/edict /tmp/trimmed.md
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "$0")/lib/common.sh"
bin=$(readlink -f "${1:?usage: render-variant.sh <edict-binary> <out.md>}")
out="${2:?usage: render-variant.sh <edict-binary> <out.md>}"
[[ -x "$bin" ]] || die "not executable: $bin"

EVAL_DIR=$(eval_new_dir render)
sbx_write_env "$EVAL_DIR"
sbx_load "$EVAL_DIR"
sbx_install_tools "$EVAL_DIR"
p="$EVAL_DIR/greeter"
mkdir -p "$p"
scaffold_greeter "$p" todo
(
  cd "$p"
  sbx git init -q -b main
  sbx git add -A
  sbx git commit -q -m init
  sbx "$bin" init --no-interactive --name greeter --type cli \
    --tools bones,maw,seal,rite,vessel --reviewers security \
    --check-command "cargo test" --no-seed-work --no-commit
) >"$ART/render.log" 2>&1 || {
  cat "$ART/render.log" >&2
  die "init with $bin failed"
}
for id in $(sbx rite hooks list --format json 2>/dev/null | jq -r '.hooks[]?.id'); do
  (cd "$p" && sbx rite hooks remove "$id" >/dev/null 2>&1) || true
done
perl -0ne 'print $1 if /<!-- edict:managed-start -->(.*?)<!-- edict:managed-end -->/s' "$p/AGENTS.md" >"$out"
[[ -s "$out" ]] || die "no managed block rendered"
echo "wrote $out ($(wc -w <"$out") words, ~$(($(wc -c <"$out") / 4)) tokens) from $("$bin" --version)"
echo "sandbox left at $EVAL_DIR"
