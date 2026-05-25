#!/usr/bin/env bash
# Linux 28-tool smoke test (Combo A — Ubuntu 22.04 + Xfce + X11).
# Runs each cua-driver tool against an Xvfb display with xeyes as the
# victim. Reports PASS / FAIL / SKIP per tool with brief reason.
set -u
export PATH="$HOME/.local/bin:$PATH"
DRIVER="cua-driver"
DISPLAY_NUM=":99"
OUT="$HOME/linux-smoke-output.log"

log() { printf '%s\n' "$@"; }
sep() { log "------------------------------------------------------------"; }

declare -A RESULT
record() {
    # record TOOL VERDICT [REASON]
    local tool="$1" verdict="$2" reason="${3:-}"
    RESULT["$tool"]="$verdict|$reason"
}

# Run a tool with JSON args, classify
run_tool() {
    local tool="$1"
    # Default to '{}' for no-arg tools. Don't use ${2:-{}} — bash parses
    # that as ${2:-{} + literal } which appends a spurious '}' to every arg.
    local args
    if [[ -z "${2-}" ]]; then args='{}'; else args="$2"; fi
    local out err code
    out=$("$DRIVER" call "$tool" "$args" 2>&1) ; code=$?
    if [[ $code -eq 0 ]]; then
        # cua-driver convention: ❌ prefix means tool failure. Don't match
        # the word "error" generically — it appears as a field name in
        # several successful responses (e.g. get_recording_state JSON).
        if [[ "$out" == "❌"* || "$out" == "Error:"* ]]; then
            local first; first=$(echo "$out" | head -1 | tr -d '\n' | cut -c1-160)
            record "$tool" "FAIL" "exit0 but err in output: $first"
        else
            local first; first=$(echo "$out" | head -1 | tr -d '\n' | cut -c1-100)
            record "$tool" "PASS" "$first"
        fi
    else
        local first; first=$(echo "$out" | head -1 | tr -d '\n' | cut -c1-160)
        record "$tool" "FAIL" "exit=$code: $first"
    fi
}

log "============================================================"
log "Linux cua-driver smoke test"
log "Driver: $(which "$DRIVER")"
log "Version: $($DRIVER --version)"
log "$(uname -a)"
log "============================================================"
log ""

# === 0. Set up Xvfb + a victim ===
log "==> Starting Xvfb on $DISPLAY_NUM"
pkill -f "Xvfb $DISPLAY_NUM" 2>/dev/null; sleep 1
Xvfb "$DISPLAY_NUM" -screen 0 1280x800x24 &
XVFB_PID=$!
sleep 2
export DISPLAY="$DISPLAY_NUM"
log "  Xvfb pid=$XVFB_PID DISPLAY=$DISPLAY"

# Start at-spi-bus + register so the at-spi-heavy tools have a fighting chance.
log "==> Starting at-spi"
/usr/libexec/at-spi-bus-launcher --launch-immediately 2>/dev/null &
ATSPI_PID=$!
sleep 1
export NO_AT_BRIDGE=0
export GTK_MODULES=gail:atk-bridge

log "==> Spawning xeyes victim"
xeyes -geometry 200x200+100+100 &
XEYES_PID=$!
sleep 2
log "  xeyes pid=$XEYES_PID"

# Determine xeyes window id. `xdotool search --pid` doesn't work for
# xeyes (no _NET_WM_PID), so search by window class.
WIN_ID=$(xdotool search --class XEyes 2>/dev/null | tail -1)
if [[ -z "$WIN_ID" ]]; then
    WIN_ID=$(xdotool search --name "xeyes" 2>/dev/null | tail -1)
fi
if [[ -z "$WIN_ID" ]]; then WIN_ID=0; fi
log "  xeyes window_id=$WIN_ID"
log ""

# === 1. No-args tools (7) ===
log "=== Group 1: no-arg tools ==="
for tool in check_permissions get_screen_size get_cursor_position get_config get_agent_cursor_state get_recording_state list_apps list_windows; do
    run_tool "$tool"
done

# === 2. Setters (5) ===
log ""
log "=== Group 2: setters ==="
run_tool set_config '{"max_image_dimension":1024}'
run_tool set_agent_cursor_enabled '{"enabled":true}'
run_tool set_agent_cursor_style '{"style":"default"}'
run_tool set_agent_cursor_motion '{}'
run_tool set_recording '{"enabled":false}'

# === 3. App lifecycle (2) ===
log ""
log "=== Group 3: app lifecycle ==="
run_tool launch_app '{"name":"xclock"}'
sleep 2
XCLOCK_PID=$(pgrep -n xclock || echo "0")
log "  xclock pid=$XCLOCK_PID"
if [[ "$XCLOCK_PID" != "0" ]]; then
    run_tool kill_app "{\"pid\":$XCLOCK_PID}"
else
    record kill_app FAIL "xclock didn't launch — can't test kill"
fi

# === 4. Per-window state (4) ===
log ""
log "=== Group 4: per-window state (xeyes pid=$XEYES_PID win=$WIN_ID) ==="
run_tool screenshot "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID}"
run_tool zoom "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"x1\":0,\"y1\":0,\"x2\":100,\"y2\":100}"
run_tool get_window_state "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID}"
run_tool get_accessibility_tree "{\"pid\":$XEYES_PID}"

# === 5. Input synthesis (11) ===
log ""
log "=== Group 5: input synthesis (against xeyes) ==="
run_tool move_cursor '{"x":300,"y":300}'
run_tool click "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"x\":50,\"y\":50}"
run_tool double_click "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"x\":50,\"y\":50}"
run_tool right_click "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"x\":50,\"y\":50}"
run_tool drag "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"from_x\":40,\"from_y\":40,\"to_x\":80,\"to_y\":80}"
run_tool scroll "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"x\":50,\"y\":50,\"direction\":\"down\"}"
# Keyboard tools need window_id (the keyboard backend looks up the focused
# X window through the target pid's windows, and xeyes only exposes one).
run_tool type_text "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"text\":\"hi\"}"
run_tool press_key "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"key\":\"a\"}"
run_tool hotkey "{\"pid\":$XEYES_PID,\"window_id\":$WIN_ID,\"keys\":[\"ctrl\",\"a\"]}"
record type_text_chars SKIP "deprecated tool — driver returns deprecation message pointing to type_text"
record set_value SKIP "needs an AT-SPI element with editable text — xeyes has none"

# === 6. Special (2) ===
log ""
log "=== Group 6: special ==="
record page SKIP "needs Chrome/Chromium with --remote-debugging-port — not installed"
record replay_trajectory SKIP "needs a recorded trajectory file"

# === Cleanup ===
log ""
log "==> Cleanup"
kill "$XEYES_PID" 2>/dev/null
kill "$ATSPI_PID" 2>/dev/null
kill "$XVFB_PID" 2>/dev/null
sleep 1

# === Summary ===
log ""
log "============================================================"
log "SUMMARY"
log "============================================================"
pass=0; fail=0; skip=0
# Sort tool names for stable output
tools=$(printf '%s\n' "${!RESULT[@]}" | sort)
printf "%-28s %-6s %s\n" "TOOL" "VERDICT" "DETAIL"
printf "%s\n" "----------------------------------------------------------------"
for tool in $tools; do
    IFS='|' read -r verdict reason <<< "${RESULT[$tool]}"
    case "$verdict" in
        PASS) ((pass++)) ;;
        FAIL) ((fail++)) ;;
        SKIP) ((skip++)) ;;
    esac
    printf "%-28s %-6s %s\n" "$tool" "$verdict" "${reason:0:80}"
done

log ""
log "Tools tested: ${#RESULT[@]}    PASS=$pass    FAIL=$fail    SKIP=$skip"
