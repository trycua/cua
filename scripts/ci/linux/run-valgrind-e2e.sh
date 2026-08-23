#!/usr/bin/env bash
set -euo pipefail

driver=${1:?usage: run-valgrind-e2e.sh /path/to/cua-driver}
driver=$(realpath "$driver")
artifact_dir=${CUA_VALGRIND_ARTIFACT_DIR:-"${RUNNER_TEMP:-/tmp}/cua-driver-valgrind"}
rm -rf "$artifact_dir"
mkdir -p "$artifact_dir"

smoke_dir=$(mktemp -d)
socket="$smoke_dir/cua-driver.sock"
server_pid=""
server_waited=0

export HOME="$smoke_dir/home"
export XDG_CACHE_HOME="$smoke_dir/cache"
export XDG_CONFIG_HOME="$smoke_dir/config"
export XDG_DATA_HOME="$smoke_dir/data"
export XDG_RUNTIME_DIR="$smoke_dir/runtime"
mkdir -p "$HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

cleanup() {
  local status=$?
  set +e
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    timeout 10s "$driver" stop --socket "$socket" >>"$artifact_dir/client.log" 2>&1
    for _ in {1..40}; do
      kill -0 "$server_pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$server_pid" 2>/dev/null; then
      kill -TERM "$server_pid" 2>/dev/null
    fi
  fi
  if [[ -n "$server_pid" && $server_waited -eq 0 ]]; then
    wait "$server_pid"
  fi
  cp -a "$smoke_dir/." "$artifact_dir/smoke-dir" 2>/dev/null
  rm -rf "$smoke_dir"
  exit "$status"
}
trap cleanup EXIT INT TERM

server_command=(
  valgrind
  --leak-check=full
  --gen-suppressions=all
  --num-callers=40
  --show-leak-kinds=definite,possible
  --errors-for-leak-kinds=definite,possible
  --error-exitcode=99
  --log-file="$artifact_dir/valgrind.log"
  "$driver"
  serve
  --socket "$socket"
  --no-overlay
  --dangerously-bypass-approvals
)
if [[ ${CUA_DRIVER_MEMCHECK_DISABLE:-0} == 1 ]]; then
  server_command=("$driver" serve --socket "$socket" --no-overlay --dangerously-bypass-approvals)
fi

printf 'server command:' >"$artifact_dir/commands.log"
printf ' %q' "${server_command[@]}" >>"$artifact_dir/commands.log"
printf '\n' >>"$artifact_dir/commands.log"
"${server_command[@]}" >"$artifact_dir/server.stdout" 2>"$artifact_dir/server.stderr" &
server_pid=$!

ready=0
for _ in {1..120}; do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid" || true
    server_waited=1
    echo "cua-driver server exited before readiness" >&2
    exit 1
  fi
  if timeout 3s "$driver" status --socket "$socket" >"$artifact_dir/status.txt" 2>>"$artifact_dir/client.log"; then
    ready=1
    break
  fi
  sleep 0.5
done
[[ $ready -eq 1 ]] || { echo "cua-driver server did not become ready within 60 seconds" >&2; exit 1; }

grep -qi 'running' "$artifact_dir/status.txt"
printf '%s\n' \
  "$driver status --socket $socket" \
  "$driver sessions list --json --socket $socket" \
  "$driver call get_config '{}' --socket $socket" \
  "$driver call set_config '{\"key\":\"max_image_dimension\",\"value\":640}' --socket $socket" \
  "$driver call check_permissions '{}' --socket $socket" \
  "$driver mcp --socket $socket" \
  "$driver stop --socket $socket" >>"$artifact_dir/commands.log"

timeout 10s "$driver" sessions list --json --socket "$socket" >"$artifact_dir/sessions-before.json"
timeout 10s "$driver" call get_config '{}' --socket "$socket" >"$artifact_dir/config-before.json"
timeout 10s "$driver" call set_config '{"key":"max_image_dimension","value":640}' --socket "$socket" >"$artifact_dir/config-set.json"
timeout 10s "$driver" call get_config '{}' --socket "$socket" >"$artifact_dir/config-after.json"
timeout 10s "$driver" call check_permissions '{}' --socket "$socket" >"$artifact_dir/permissions.json"

cat >"$smoke_dir/mcp-requests.jsonl" <<'JSONL'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"valgrind-e2e","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_config","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"compatibility/unknown","params":{}}
JSONL
timeout 30s "$driver" mcp --socket "$socket" <"$smoke_dir/mcp-requests.jsonl" >"$artifact_dir/mcp-responses.jsonl" 2>>"$artifact_dir/client.log"
for _ in {1..40}; do
  timeout 10s "$driver" sessions list --json --socket "$socket" >"$artifact_dir/sessions-after.json"
  if python3 - "$artifact_dir/sessions-after.json" <<'PY'
import json
import pathlib
import sys

raise SystemExit(0 if json.loads(pathlib.Path(sys.argv[1]).read_text()).get("count") == 0 else 1)
PY
  then
    break
  fi
  sleep 0.25
done

python3 - "$artifact_dir" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])


def load(name):
    return json.loads((root / name).read_text())


before = load("config-before.json")
after = load("config-after.json")
permissions = load("permissions.json")
sessions_before = load("sessions-before.json")
sessions_after = load("sessions-after.json")

assert before["platform"] == "linux"
assert after["max_image_dimension"] == 640
assert after["max_image_dimension"] != before["max_image_dimension"]
assert permissions["x11"] is True
assert sessions_before == {"count": 0, "sessions": []}
assert sessions_after == {"count": 0, "sessions": []}

responses = [json.loads(line) for line in (root / "mcp-responses.jsonl").read_text().splitlines()]
by_id = {response.get("id"): response for response in responses}
assert by_id[1]["result"]["serverInfo"]["name"] == "cua-driver"
tools = by_id[2]["result"]["tools"]
tool_names = {tool["name"] for tool in tools}
required = {"get_config", "set_config", "list_apps", "list_windows", "get_window_state", "click", "type_text", "press_key"}
assert required <= tool_names, f"missing tools: {sorted(required - tool_names)}"
assert by_id[3]["result"].get("isError", False) is not True
assert by_id[3]["result"]["structuredContent"]["max_image_dimension"] == 640
assert by_id[4]["error"] == {"code": -32601, "message": "Unknown method: compatibility/unknown"}
PY

timeout 10s "$driver" stop --socket "$socket" >>"$artifact_dir/client.log" 2>&1
set +e
wait "$server_pid"
memcheck_status=$?
set -e
server_waited=1

cat "$artifact_dir/server.stderr"
if [[ -f "$artifact_dir/valgrind.log" ]]; then
  cat "$artifact_dir/valgrind.log"
fi
if ((memcheck_status != 0)); then
  echo "Valgrind-wrapped cua-driver server exited with status $memcheck_status" >&2
  exit "$memcheck_status"
fi

grep -q 'Cua Driver daemon shutting down.' "$artifact_dir/server.stderr"
if [[ ${CUA_DRIVER_MEMCHECK_DISABLE:-0} != 1 ]]; then
  grep -q 'definitely lost: 0 bytes in 0 blocks' "$artifact_dir/valgrind.log"
  grep -q 'possibly lost: 0 bytes in 0 blocks' "$artifact_dir/valgrind.log"
  grep -q 'ERROR SUMMARY: 0 errors from 0 contexts' "$artifact_dir/valgrind.log"
fi
