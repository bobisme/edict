#!/usr/bin/env bash
# Thin wrapper: see run.sh for the environment it reads.
exec "$(dirname "$0")/run.sh" lead-merge "$@"
