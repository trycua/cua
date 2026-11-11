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
[[ -s "$root/cases.jsonl" ]] || fail 'missing cases.jsonl'
cells=$(jq -er '
  [.cell_id, .delivery, .action, .expected_behavior.kind] |
  if all(type == "string" and length > 0) then @tsv else error("malformed declaration") end
' "$root/cases.jsonl") || fail 'malformed cases.jsonl'
expect_recording() {
  local label="$1" trajectory
  matches=()
  for trajectory in "${trajectories[@]}"; do
    if jq -e --arg label "$label" '.label == $label' "$trajectory" >/dev/null; then
      matches+=("$(dirname "$trajectory")")
    fi
  done
  (( ${#matches[@]} == 1 )) || fail "expected one current recording for $label"
  directory="${matches[0]}"
}
expect_recording environment-preflight
for name in daemon.json health_report.json check_permissions.json; do
  [[ -s "$directory/$name" ]] || fail "missing preflight $name"
done
while IFS=$'\t' read -r label delivery action expected; do
  expect_recording "$label"
  case "$expected" in
    deliver) observed=Delivered ;;
    refuse) observed=Refused ;;
    *) fail "$label has unknown expected behavior: $expected" ;;
  esac
  jq -Rse --arg label "$label" --arg observed "$observed" '
    [split("\n")[] | gsub("\\\\\\|"; "\u0000") | split("|") |
      map(gsub("^\\s+|\\s+$"; "")) | select(.[1] == $label)] |
    length == 1 and (.[0][11] == $observed and .[0][12] == "Pass")
  ' "$root/summary.md" >/dev/null || fail "$label summary must observe $observed / Pass"
  required=(tool-calls.jsonl fixture-state.json)
  [[ "$delivery" == background ]] && required+=(sentinel-events.jsonl desktop-observation.txt)
  for name in "${required[@]}"; do
    [[ -s "$directory/$name" ]] || fail "missing $label/$name"
  done
  if [[ "$delivery" == background ]]; then
    jq -se 'any(.[]; .kind == "heartbeat")' "$directory/sentinel-events.jsonl" >/dev/null || fail "$label missing heartbeat"
  fi
  # Only click cells have a fixed journal marker and MCP tool; the other
  # actions rely on the runner's state oracle and any screenshot pair.
  case "$action" in
    left_click) tool=click ;;
    right_click | double_click) tool="$action" ;;
    *) tool= ;;
  esac
  if [[ -n "$tool" && "$expected" == deliver ]]; then
    jq -e --arg action "$action" \
      '[.. | strings | test("(^|[^[:alnum:]_])last_action=\($action)($|[^[:alnum:]_])")] | any' \
      "$directory/fixture-state.json" >/dev/null || fail "$label missing $action fixture marker"
  fi
  [[ "$label" == *-typed-sdk ]] && tool=
  paired=0
  for turn in "$directory"/turn-*; do
    [[ -s "$turn/before.png" && -s "$turn/after.png" ]] || continue
    if [[ -z "$tool" ]] || jq -e --arg tool "$tool" '.tool == $tool' "$turn/action.json" >/dev/null; then
      paired=1
    fi
  done
  (( paired == 1 )) || fail "$label missing screenshot pair${tool:+ ($tool required for MCP)}"
done <<< "$cells"
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
[[ -n "$pids" ]] || fail 'no owned PIDs recorded'
while IFS= read -r pid; do
  kill -0 "$pid" 2>/dev/null && fail "owned PID still alive after cleanup: $pid"
done <<< "$pids"
echo "Evidence complete; owned PIDs checked after cleanup: $(tr '\n' ' ' <<< "$pids")"
