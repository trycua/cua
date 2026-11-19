#!/usr/bin/env bash
# Thin wrapper: tests/compat/examples/run.sh [run.py args...]
# Work files go under $CUA_EXAMPLES_WORK (default: the system temp dir).
exec python3 "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/run.py" "$@"
