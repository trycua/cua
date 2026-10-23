#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob
fail() { echo "Invalid evidence: $*" >&2; exit 1; }
[[ $# == 1 && -d "$1" ]] || fail 'usage: check-evidence.sh EVIDENCE_DIRECTORY'
root="$1"
[[ -s "$root/summary.md" ]] || fail 'missing summary.md'
trajectories=("$root"/recordings/*/trajectory.json)
(( ${#trajectories[@]} > 0 )) || fail 'no current-attempt trajectories or owned PIDs'
for trajectory in "${trajectories[@]}"; do
  jq -e '.label | type == "string" and length > 0' "$trajectory" >/dev/null || fail "missing label: $trajectory"
done
for label in linux-electron-left-click-ax-background linux-electron-left-click-ax-background-typed-sdk environment-preflight; do
  matches=()
  for trajectory in "${trajectories[@]}"; do
    if jq -e --arg label "$label" '.label == $label' "$trajectory" >/dev/null; then
      matches+=("$(dirname "$trajectory")")
    fi
  done
  (( ${#matches[@]} == 1 )) || fail "expected one current recording for $label"
  directory="${matches[0]}"
  if [[ "$label" == environment-preflight ]]; then
    for name in daemon.json health_report.json check_permissions.json; do
      [[ -s "$directory/$name" ]] || fail "missing preflight $name"
    done
    continue
  fi
  jq -Rse --arg label "$label" '
    [split("\n")[] | gsub("\\\\\\|"; "\u0000") | split("|") |
      map(gsub("^\\s+|\\s+$"; "")) | select(.[1] == $label)] |
    length == 1 and (.[0][11] == "Delivered" and .[0][12] == "Pass")
  ' "$root/summary.md" >/dev/null || fail "$label summary must observe Delivered / Pass"
  for name in tool-calls.jsonl fixture-state.json sentinel-events.jsonl desktop-observation.txt; do
    [[ -s "$directory/$name" ]] || fail "missing $label/$name"
  done
  jq -e '[.. | strings | test("(^|[^[:alnum:]_])last_action=left_click($|[^[:alnum:]_])")] | any' \
    "$directory/fixture-state.json" >/dev/null || fail "$label missing left_click fixture marker"
  jq -se 'any(.[]; .kind == "heartbeat")' "$directory/sentinel-events.jsonl" >/dev/null || fail "$label missing heartbeat"
  paired=0
  for turn in "$directory"/turn-*; do
    [[ -s "$turn/before.png" && -s "$turn/after.png" ]] || continue
    if [[ "$label" == *-typed-sdk ]] || jq -e '.tool == "click"' "$turn/action.json" >/dev/null; then
      paired=1
    fi
  done
  (( paired == 1 )) || fail "$label missing screenshot pair (click required for MCP)"
done
pids=$(jq -r '.daemon_pid | select(type == "number" and . > 0 and . == floor)' "${trajectories[@]}") || fail 'invalid trajectory PID extraction'
transcripts=("$root"/recordings/*/tool-calls.jsonl)
if (( ${#transcripts[@]} > 0 )); then
  fixture_pids=$(jq -r '
    select(.request.params.name == "list_windows") |
    .response.result.structuredContent.windows[]? |
    select(.title | if type == "string" then startswith("CuaTestHarness") else false end) |
    .pid | select(type == "number" and . > 0 and . == floor)
  ' "${transcripts[@]}") || fail 'invalid transcript PID extraction'
  pids+=$'\n'"$fixture_pids"
fi
pids=$(printf '%s\n' "$pids" | sort -nu | grep -v '^$') || fail 'no owned PIDs recorded'
while IFS= read -r pid; do
  if kill -0 "$pid" 2>/dev/null; then
    fail "owned PID still alive after cleanup: $pid"
  fi
done <<< "$pids"
echo "Evidence complete; owned PIDs checked after cleanup: $(tr '\n' ' ' <<< "$pids")"
