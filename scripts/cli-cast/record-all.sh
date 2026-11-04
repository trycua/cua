#!/usr/bin/env bash
# Re-record every golden cast against the locally installed CLI.
#
# Run this when a new Claude Code release changes the rendering. The goldens it
# overwrites are inputs, not expectations -- regenerate the expected JSON-UI
# afterwards with `swift test --filter Golden` and the CUA_TRANSCRIPT_RECORD=1
# environment variable, and read the diff before committing it.
#
#   ./record-all.sh /path/to/scratch-project
#
# The project directory must be a scratch checkout. Never point this at a live
# Space or at anything holding credentials: the recorder scrubs known secret
# shapes, but the only reliable defence is not recording them.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PROJECT="${1:?usage: record-all.sh <scratch project dir>}"
OUT="$HERE/../../libs/spaces-sdk-swift/Tests/CuaSpacesTranscriptTests/Goldens"

mkdir -p "$OUT"
for script in "$HERE"/scripts/claude-*.json; do
    name="$(basename "$script" .json)"
    echo "== $name"
    python3 "$HERE/record.py" --script "$script" --cwd "$PROJECT" \
        --out "$OUT/$name.cast"
    ( cd "$PROJECT" && git checkout -- . 2>/dev/null || true; git clean -fd 2>/dev/null || true )
done
