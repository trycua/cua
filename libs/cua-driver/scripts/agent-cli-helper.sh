#!/usr/bin/env bash
set -euo pipefail

CUA_DRIVER_BIN="${CUA_DRIVER_BIN:-cua-driver}"
SHOT_DIR="${CUA_SCREENSHOT_DIR:-/tmp/cua-screenshots}"

usage() {
  cat <<'EOF'
Usage: cua-driver-agent <command> [args]

CLI-first helper for agentic Cua Driver workflows. Install by putting this
script on PATH as `cua-driver-agent` or `cua`.

Commands:
  driver <args...>              Pass through to cua-driver
  call <tool> [json] [flags...] Pass through to cua-driver call
  serve                         Start one headless Cua Driver daemon
  status                        Show version, config, permissions, process state
  fast                          Automation mode: cursor off, capture_mode=ax
  demo                          Visible cursor mode: cursor on, fast motion
  ax | vision | som             Set capture_mode
  apps                          cua-driver call list_apps '{}'
  windows <pid>                 cua-driver call list_windows '{"pid":pid}'
  state <pid> <window_id> [query]
                                AX get_window_state; optional query filter
  state-shot <pid> <window_id> [out.png]
                                som get_window_state with screenshot_out_file, then restore ax
  screenshot <window_id> [out.png]
                                Window screenshot. Tries CUA, falls back to macOS screencapture -l
  screen [out.png]              Full-display screenshot via macOS screencapture
  calc                          Launch Calculator and print pid/window_id candidates
EOF
}

ensure_dir() { mkdir -p "$(dirname "$1")"; }
json_escape() { python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }

case "${1:-}" in
  ""|-h|--help|help) usage ;;
  driver) shift; exec "$CUA_DRIVER_BIN" "$@" ;;
  call) shift; exec "$CUA_DRIVER_BIN" call "$@" ;;
  serve) open -n -g -a CuaDriver --args serve ;;
  status)
    echo "== version =="; "$CUA_DRIVER_BIN" --version || true
    echo "== cursor =="; "$CUA_DRIVER_BIN" call get_agent_cursor_state '{}' || true
    echo "== config =="; "$CUA_DRIVER_BIN" call get_config '{}' || true
    echo "== permissions =="; "$CUA_DRIVER_BIN" call check_permissions '{}' || true
    echo "== processes =="
    ps -axo pid,ppid,stat,lstart,command | awk '/CuaDriver|cua-driver/ && !/awk/ {print}'
    ;;
  fast)
    "$CUA_DRIVER_BIN" call set_agent_cursor_enabled '{"enabled":false}'
    "$CUA_DRIVER_BIN" call set_agent_cursor_motion '{"glide_duration_ms":50,"dwell_after_click_ms":0,"idle_hide_ms":1000}'
    "$CUA_DRIVER_BIN" config set capture_mode ax
    ;;
  demo)
    "$CUA_DRIVER_BIN" call set_agent_cursor_enabled '{"enabled":true}'
    "$CUA_DRIVER_BIN" call set_agent_cursor_motion '{"glide_duration_ms":50,"dwell_after_click_ms":0,"idle_hide_ms":1000}'
    ;;
  ax|vision|som) "$CUA_DRIVER_BIN" config set capture_mode "$1" ;;
  apps) "$CUA_DRIVER_BIN" call list_apps '{}' ;;
  windows)
    pid="${2:?usage: cua-driver-agent windows <pid>}"
    "$CUA_DRIVER_BIN" call list_windows "{\"pid\":${pid}}"
    ;;
  state)
    pid="${2:?usage: cua-driver-agent state <pid> <window_id> [query]}"
    wid="${3:?usage: cua-driver-agent state <pid> <window_id> [query]}"
    if [[ $# -ge 4 ]]; then
      q=$(json_escape "$4")
      "$CUA_DRIVER_BIN" call get_window_state "{\"pid\":${pid},\"window_id\":${wid},\"query\":${q}}"
    else
      "$CUA_DRIVER_BIN" call get_window_state "{\"pid\":${pid},\"window_id\":${wid}}"
    fi
    ;;
  state-shot)
    pid="${2:?usage: cua-driver-agent state-shot <pid> <window_id> [out.png]}"
    wid="${3:?usage: cua-driver-agent state-shot <pid> <window_id> [out.png]}"
    out="${4:-$SHOT_DIR/cua-state-${pid}-${wid}-$(date +%s).png}"
    ensure_dir "$out"
    "$CUA_DRIVER_BIN" config set capture_mode som >/dev/null
    cleanup() { "$CUA_DRIVER_BIN" config set capture_mode ax >/dev/null 2>&1 || true; }
    trap cleanup EXIT
    "$CUA_DRIVER_BIN" call get_window_state "{\"pid\":${pid},\"window_id\":${wid},\"screenshot_out_file\":\"${out}\"}"
    test -s "$out"
    printf '\nSCREENSHOT:%s\n' "$out"
    ;;
  screenshot)
    wid="${2:?usage: cua-driver-agent screenshot <window_id> [out.png]}"
    out="${3:-$SHOT_DIR/cua-window-${wid}-$(date +%s).png}"
    ensure_dir "$out"
    if timeout 20 "$CUA_DRIVER_BIN" call screenshot "{\"window_id\":${wid},\"format\":\"png\"}" --screenshot-out-file "$out" >/tmp/cua-driver-agent-screenshot.log 2>&1 && test -s "$out"; then
      printf '%s\n' "$out"
    else
      /usr/sbin/screencapture -x -l "$wid" "$out"
      test -s "$out"
      printf '%s\n' "$out"
    fi
    ;;
  screen)
    out="${2:-$SHOT_DIR/cua-screen-$(date +%s).png}"
    ensure_dir "$out"
    /usr/sbin/screencapture -x "$out"
    test -s "$out"
    printf '%s\n' "$out"
    ;;
  calc) "$CUA_DRIVER_BIN" call launch_app '{"bundle_id":"com.apple.calculator"}' ;;
  *) echo "Unknown command: $1" >&2; usage >&2; exit 64 ;;
esac
