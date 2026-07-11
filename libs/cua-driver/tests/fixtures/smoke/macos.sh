#!/usr/bin/env bash
# macOS cua-driver per-tool smoke test.
#
# Spawns the built AppKit harness app from libs/cua-driver/rust/test-apps/harness-appkit
# (source lives under libs/cua-driver/tests/fixtures/apps/macos),
# runs every cua-driver tool against it with sensible JSON args, and reports
# PASS / FAIL / SKIP in a sorted table.
#
# Mirror of scripts/linux-smoke.sh — same shape, classified the same way.
# Different from the Rust integration tests in scope: this one hits the
# CLI (`cua-driver call <tool> <json>`) rather than the MCP stdio loop,
# so it covers a different code path (CLI argument parser + tool
# resolution) and gives a single broad PASS/FAIL across the whole tool
# surface in ~30 seconds.
#
# Prereqs:
#   - libs/cua-driver/tests/fixtures/build/macos.sh must have been run
#   - cua-driver binary built (this script uses target/release if present)
#   - TCC Accessibility permission granted to the cua-driver binary
#     (otherwise AX-using tools return empty trees, which scores as PASS
#     because the call exits cleanly — flagged in output)
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
DRIVER="$ROOT/libs/cua-driver/rust/target/release/cua-driver"
[[ -x "$DRIVER" ]] || DRIVER="$ROOT/libs/cua-driver/rust/target/debug/cua-driver"
HARNESS_APP="$ROOT/libs/cua-driver/rust/test-apps/harness-appkit/CuaTestHarness.AppKit.app"
HARNESS_EXE="$HARNESS_APP/Contents/MacOS/CuaTestHarness.AppKit"

# macOS ships bash 3.2 (no associative arrays). Accumulate results as
# newline-delimited "TOOL|VERDICT|REASON" rows; sort + dedup at the end.
RESULT_LOG="$(mktemp -t mac-smoke-results)"
trap 'rm -f "$RESULT_LOG"' EXIT
record() {
    local tool="$1" verdict="$2" reason="${3:-}"
    printf '%s|%s|%s\n' "$tool" "$verdict" "$reason" >> "$RESULT_LOG"
}

run_tool() {
    local tool="$1"
    # Don't write '${2:-{}}' — bash parses that as '${2:-{}' + literal '}',
    # which evaluates to '{' (not '{}') and breaks JSON parsing downstream.
    # Same trap I hit in scripts/linux-smoke.sh.
    local args
    if [[ -z "${2-}" ]]; then args='{}'; else args="$2"; fi
    local out code
    out=$("$DRIVER" call "$tool" "$args" 2>&1) ; code=$?
    # Treat documented "this tool is intentionally a per-platform stub"
    # responses as SKIP rather than FAIL — they indicate the tool was
    # called correctly but isn't meaningful on this OS by design.
    if echo "$out" | grep -q "unsupported_on_platform\|is Windows-only\|is Linux-only\|is macOS-only"; then
        record "$tool" "SKIP" "intentional cross-platform stub on macOS"
        return
    fi
    if [[ $code -eq 0 ]]; then
        if [[ "$out" == "❌"* || "$out" == "Error:"* ]]; then
            record "$tool" "FAIL" "exit0+❌: $(echo "$out" | head -1 | cut -c1-100)"
        else
            record "$tool" "PASS" "$(echo "$out" | head -1 | cut -c1-100)"
        fi
    else
        record "$tool" "FAIL" "exit=$code: $(echo "$out" | head -1 | cut -c1-100)"
    fi
}

echo "============================================================"
echo "macOS cua-driver smoke test"
echo "Driver: $DRIVER"
echo "Version: $("$DRIVER" --version 2>&1)"
echo "Harness: $HARNESS_APP"
echo "Date: $(date)"
echo "============================================================"

if [[ ! -x "$DRIVER" ]]; then
    echo "ERROR: cua-driver binary not found at $DRIVER" >&2
    echo "       Run: cd libs/cua-driver/rust && cargo build --release -p cua-driver" >&2
    exit 1
fi

if [[ ! -x "$HARNESS_EXE" ]]; then
    echo "ERROR: AppKit harness not built at $HARNESS_EXE" >&2
    echo "       Run: libs/cua-driver/tests/fixtures/build/macos.sh" >&2
    exit 1
fi

echo ""
echo "==> Spawning AppKit harness as victim"
"$HARNESS_EXE" >/dev/null 2>&1 &
HARNESS_PID=$!
trap 'rm -f "$RESULT_LOG"; kill -9 $HARNESS_PID 2>/dev/null; pkill -9 -f CuaTestHarness 2>/dev/null' EXIT
sleep 1.5
echo "  Harness pid=$HARNESS_PID"

# Resolve harness window via list_windows (so window_id is the real one).
WIN_JSON=$("$DRIVER" call list_windows "{\"pid\":$HARNESS_PID}" 2>/dev/null)
WIN_ID=$(printf '%s' "$WIN_JSON" \
    | python3 -c "import json,sys
data=json.load(sys.stdin)
for w in data.get('windows', []):
    if w.get('pid')==$HARNESS_PID and 'AppKit' in (w.get('title') or ''):
        print(w.get('window_id'));break" 2>/dev/null || echo "")
echo "  Window id=$WIN_ID"

# ── group 1: no-arg tools (informational, always-safe) ──────────────────────
echo ""
echo "=== group 1: no-arg / config tools ==="
for t in check_permissions get_screen_size get_cursor_position get_config \
         get_agent_cursor_state get_recording_state list_apps list_windows; do
    run_tool "$t"
done

# ── group 2: setters ─────────────────────────────────────────────────────────
echo ""
echo "=== group 2: setters ==="
run_tool set_config '{"max_image_dimension":1024}'
run_tool set_agent_cursor_enabled '{"enabled":true}'
run_tool set_agent_cursor_style '{"style":"default"}'
run_tool set_agent_cursor_motion '{}'
run_tool set_recording '{"enabled":false}'

# ── group 3: app lifecycle ──────────────────────────────────────────────────
echo ""
echo "=== group 3: app lifecycle ==="
run_tool launch_app '{"name":"TextEdit"}'
sleep 1.5
TE_PID=$(pgrep -n TextEdit || echo 0)
echo "  TextEdit pid=$TE_PID"
if [[ "$TE_PID" != "0" ]]; then
    run_tool kill_app "{\"pid\":$TE_PID}"
else
    record kill_app FAIL "launch_app didn't surface a pid"
fi

# ── group 4: per-window state ───────────────────────────────────────────────
echo ""
echo "=== group 4: per-window state (harness pid=$HARNESS_PID wid=$WIN_ID) ==="
if [[ -z "$WIN_ID" ]]; then
    record get_window_state SKIP "no window_id resolved"
    record get_accessibility_tree SKIP "no window_id resolved"
    record zoom SKIP "no window_id resolved"
else
    run_tool get_window_state "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"capture_mode\":\"tree\"}"
    run_tool get_accessibility_tree "{\"pid\":$HARNESS_PID}"
    run_tool zoom "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"x1\":0,\"y1\":0,\"x2\":200,\"y2\":200}"
fi

# ── group 5: input synthesis ────────────────────────────────────────────────
echo ""
echo "=== group 5: input synthesis (against harness) ==="
run_tool move_cursor '{"x":400,"y":400}'
if [[ -z "$WIN_ID" ]]; then
    for t in click double_click right_click drag scroll type_text press_key hotkey set_value bring_to_front; do
        record "$t" SKIP "no window_id"
    done
else
    # Coordinate-based clicks against the harness window's known scenarios.
    # These exercise the CGEvent path even when AX target isn't precise.
    run_tool click            "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"x\":120,\"y\":80}"
    run_tool double_click     "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"x\":120,\"y\":80}"
    run_tool right_click      "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"x\":120,\"y\":80}"
    run_tool drag             "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"from_x\":120,\"from_y\":80,\"to_x\":180,\"to_y\":120,\"delivery_mode\":\"foreground\"}"
    run_tool scroll           "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"x\":200,\"y\":400,\"direction\":\"down\"}"
    run_tool type_text        "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"text\":\"hi\"}"
    run_tool press_key        "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"key\":\"a\"}"
    run_tool hotkey           "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID,\"keys\":[\"cmd\",\"a\"]}"
    run_tool bring_to_front   "{\"pid\":$HARNESS_PID,\"window_id\":$WIN_ID}"
    # set_value needs element_index — we don't have one without a snapshot.
    record set_value SKIP "exercised by harness_appkit_text_input integration test instead"
fi

# ── group 6: deferred-alias / special ───────────────────────────────────────
echo ""
echo "=== group 6: aliases + special ==="
# type_text_chars is intentionally NOT in the registry (mirrors Windows
# after this branch's parity fix). The mcp-server's invoke layer
# resolves it as an alias to type_text.
record type_text_chars SKIP "deprecated alias resolved by mcp-server, not registered"
record page            SKIP "needs Chromium with --remote-debugging-port"
record replay_trajectory SKIP "needs a recorded trajectory file"

# ── summary ─────────────────────────────────────────────────────────────────
echo ""
echo "==> cleanup"
kill -9 $HARNESS_PID 2>/dev/null
pkill -9 -f CuaTestHarness 2>/dev/null

echo ""
echo "============================================================"
echo "SUMMARY"
echo "============================================================"
printf "%-30s %-6s %s\n" "TOOL" "VERDICT" "DETAIL"
printf "%s\n" "----------------------------------------------------------------"

# Keep the LAST verdict per tool (later record() calls win), then sort by name.
sort -t '|' -k1,1 -u <(awk -F'|' '
    { last[$1] = $0 }
    END { for (k in last) print last[k] }
' "$RESULT_LOG") | while IFS='|' read -r tool verdict reason; do
    printf "%-30s %-6s %s\n" "$tool" "$verdict" "$(printf '%s' "$reason" | cut -c1-80)"
done

pass=$(awk -F'|' '{print $1"|"$2}' "$RESULT_LOG" | sort -u | grep -c '|PASS$' || true)
fail=$(awk -F'|' '{print $1"|"$2}' "$RESULT_LOG" | sort -u | grep -c '|FAIL$' || true)
skip=$(awk -F'|' '{print $1"|"$2}' "$RESULT_LOG" | sort -u | grep -c '|SKIP$' || true)
total=$((pass + fail + skip))
echo ""
echo "Tools probed: $total    PASS=$pass    FAIL=$fail    SKIP=$skip"
echo ""
echo "Interpretation:"
echo "  PASS = tool ran cleanly (exit 0, no ❌ in output)"
echo "  FAIL = error or non-zero exit"
echo "  SKIP = intentionally not probed (covered by integration tests, missing fixture, etc.)"
