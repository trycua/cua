#!/bin/zsh
# One command for the real run. Start it from a Terminal.app shell so every claude process is a child of that shell:
#   open -a Terminal <this file>
# Settings come from $CDB_BENCH_WORK/launch.env (RUN_ID, CUTOFF_UTC, EXTRA_ARGS), see launch.env.example.
# CDB_BENCH_WORK defaults to ~/.cache/cua-bench-h2h. It holds the private Cua Driver install, the run folders, the
# spend ledger and the HOLD file.
#
# Steps:
#   1. Write the arm B MCP config from OpenAI's shipped plugin file (tools/make_codex_cu_mcp.py).
#   2. gate-check (no model call): refuses while $CDB_BENCH_WORK/HOLD exists or the latest known seven-day
#      utilisation is at or above 0.95.
#   3. run_bench.py run: preflight (pins, daemons, MCP servers, recording, quota probe, init checks) and, only if
#      every check passes, the schedule.
# Logs: runs/<RUN_ID>/launcher.log. launcher.exit holds the exit code (3 = gated, 4 = held, 2 = preflight failed).
# Watch it from any shell: python3 launch/watch_bench.py <RUN_ID> -f
BENCH="${0:A:h:h}"
export CDB_BENCH_WORK="${CDB_BENCH_WORK:-$HOME/.cache/cua-bench-h2h}"
WORK="$CDB_BENCH_WORK"
mkdir -p "$WORK"
[ -f "$WORK/launch.env" ] && source "$WORK/launch.env"
RUN_ID="${RUN_ID:-main}"
RUN_DIR="$WORK/runs/$RUN_ID"
mkdir -p "$RUN_DIR"
rm -f "$RUN_DIR/launcher.exit"
cd "$BENCH" || exit 1
# Keep this Terminal window out of the way of the task windows (minimise it; the run continues).
/usr/bin/osascript -e 'tell application "Terminal" to set miniaturized of (every window whose name contains "launch_bench") to true' >/dev/null 2>&1 &
PY=/opt/homebrew/bin/python3; [ -x "$PY" ] || PY="$(command -v python3)"
echo "$(date -u +%FT%TZ) launcher pid $$ run $RUN_ID" >> "$RUN_DIR/launcher.log"
$PY tools/make_codex_cu_mcp.py "$WORK/codex-access/mcp.json" 2>&1 | tee -a "$RUN_DIR/launcher.log"
$PY run_bench.py gate-check ${=EXTRA_ARGS} 2>&1 | tee -a "$RUN_DIR/launcher.log"
GATE=${pipestatus[1]}
if [ "$GATE" != "0" ]; then
  echo "$GATE" > "$RUN_DIR/launcher.exit"
  echo "$(date -u +%FT%TZ) launcher refused to start (gate-check rc=$GATE)" | tee -a "$RUN_DIR/launcher.log"
  exit $GATE
fi
# Keep the Mac and its display awake while the runner lives; the runner is a child of this shell.
/usr/bin/caffeinate -dimsu -w $$ &
CUTOFF_ARGS=(); [ -n "$CUTOFF_UTC" ] && CUTOFF_ARGS=(--cutoff-utc "$CUTOFF_UTC")  # zsh: one array, not one word
$PY run_bench.py run \
  --run-id "$RUN_ID" \
  --build-dir "$WORK/build" \
  --phase1-runs 3 --phase2-runs 2 \
  "${CUTOFF_ARGS[@]}" \
  ${=EXTRA_ARGS} \
  >> "$RUN_DIR/launcher.log" 2>&1
echo $? > "$RUN_DIR/launcher.exit"
echo "$(date -u +%FT%TZ) runner exited rc=$(cat "$RUN_DIR/launcher.exit")" >> "$RUN_DIR/launcher.log"
