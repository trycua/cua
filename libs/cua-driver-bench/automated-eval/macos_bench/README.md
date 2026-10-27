# Bench v2 harness (macOS and Linux/X11)

`cx-browser-use-cli` is staged and unavailable for scored execution: preflight always fails.
It requires `CDB_CODEX_AUTH=chatgpt` and browser-only tasks. Codex driving the CLI is not
like-for-like with Browser Use's own Agent loop. Its planned GUI-only exception covers
only recognized Browser Use CLI invocations, not general Bash or arbitrary Python.

The runner, test apps, BenchLab tasks and analysis of Cua Driver Bench v2. Start
with [`../../README.md`](../../README.md) for what the benchmark measures and
how to reproduce it, and [`../../METHOD.md`](../../METHOD.md) for the method.

| Path | Purpose |
| --- | --- |
| [`TASKS.md`](TASKS.md) | The ten Bench v2 tasks: prompts, setup, checkers, limits |
| [`RUNNER.md`](RUNNER.md) | The `claude -p` invocation, isolation, schedule, gates, output |
| `run_bench.py`, `bench_v2.py`, `bench_core.py`, `claude_*.py`, `recorder.py` | The runner |
| `cdb_adapter.py` | Runs a pack task of [`../../taskpack`](../../taskpack) from its launch descriptor |
| `probes/MB-09` to `MB-11`, `probes/IR-01` to `IR-04` | BenchLab tasks: brief template, checker, limits, provenance |
| `probes/CDB-G02` to `CDB-G04` | Selectors for the GUI-only pack tasks (`probes/CDB-S02` to `CDB-S04` are their Bench v1 variants) |
| `swift/` | BenchLab (the test app) and BenchSentinel (the witness and simulated user) |
| `tools/` | Driver installer, Codex MCP config writer, VM build scripts, fixture and validation tools, `analyze_v2.py` |
| `launch/` | One-command launcher with the quota gate, and a live status viewer |
| `pins.json` | Versions and digests the preflight enforces |
| `run_pilot.py`, `arms.py`, `codex_events.py`, `mcp_preflight.py` | The earlier Codex CLI pilot runner, which the main runner reuses for process and sentinel handling |

For the helper arm, grant `app-cu-helper` Accessibility separately from Apple's
Screen Recording grant: System Settings → Privacy & Security → Screen Recording
→ **+** → Go to `/usr/sbin/screencapture`. Terminal, adapter and launcher stay ungranted.

Preflight, including `--offline`, captures through every selected arm's own MCP
and runs the recorder's three-second test unless recording is disabled. It fails
on errors, missing images or uniform RGB frames; details contain size and path,
not image content. Offline skips model calls, not capture checks.
Tool-reported setup errors become infrastructure failures with `infra_pattern`;
model prose alone never triggers this classification.

Keep results, streams and recordings out of git.

## Linux desktop setup

Install `xvfb dbus-x11 at-spi2-core openbox picom python3-gi gir1.2-gtk-3.0
 gir1.2-atspi-2.0 xdotool x11-utils libxss1 ffmpeg lsof`, plus the applications
required by each pack task's `platform/launch.linux.json` prerequisites. Use system
Python for GTK/AT-SPI; no Swift build is required. Older non-v2 BenchLab modes are
explicitly unsupported on Linux. Codex Sky and Claude Desktop arms are skipped
with an `unsupported-platform` reason before any MCP or daemon setup.

From this directory, using an explicitly selected Linux driver and matching skills.
The launcher brings up Xvfb (1920x1080x24), a private D-Bus session, openbox and picom,
runs any command inside it, and writes its evidence (`launch.log`, component logs,
`at-spi-address.txt`, `pids.jsonl`, `exit`) to a directory outside the checkout, exported
as `CDB_SESSION_DIR`. It stops only the processes it started, fails if any recorded
process outlives the session, and passes the command's exit status through.

```sh
export CDB_BENCH_WORK="$PWD/local-work"
bash launch/linux_session.sh --evidence "$HOME/cua-session" \
  --check-run "$HOME/cua-session/runs/RUN_ID" -- bash -c '
  python3 desktop_runtime.py doctor --cua-bin "$CUA_DRIVER_BIN" --out "$CDB_SESSION_DIR" --json &&
  python3 tools/make_linux_pins.py --cua-bin "$CUA_DRIVER_BIN" --cua-skills "$CUA_SKILLS" \
    --doctor-json "$CDB_SESSION_DIR/doctor.json" --out "$CDB_SESSION_DIR/pins.json" &&
  python3 run_bench.py preflight --bench-v2 --arms cc-cua-driver \
    --cua-bin "$CUA_DRIVER_BIN" --cua-skills "$CUA_SKILLS" --pins "$CDB_SESSION_DIR/pins.json" &&
  python3 run_bench.py run --bench-v2 --run-id RUN_ID --runs-dir "$CDB_SESSION_DIR/runs" \
    --cua-bin "$CUA_DRIVER_BIN" --cua-skills "$CUA_SKILLS" --pins "$CDB_SESSION_DIR/pins.json"'
```

`doctor` needs no pins, agent or ledger. Without `--socket` it starts a private driver
daemon and always stops it; with `--socket` it only probes that socket. The pins writer
refuses to write inside a git checkout or over an existing file. `--check-run` runs
`tools/check_run_evidence.py` on a run directory after teardown. That check covers
evidence completeness only; it is separate from task success and from run completion
(`DONE`). `--disk-floor-gb` sets the free-space floor for preflight and every trial
admission (the default is unchanged and recorded in the manifest).

Task descriptors name a browser; on hosts without that name, set up a shim (a
symlink or wrapper script on `PATH`) in the environment, not in the pack.

When the disposable-environment flag is set on a shared host, run inside a private PID
and mount namespace with a private `/tmp`: the runner's fixed sockets live in `/tmp`,
and it kills apps by name.

The pin file must declare `platform: "linux"`, `cua_driver_version`,
`cua_driver_binary_sha256`, and `cua_skills_tree_sha256`, matching the selected
executable and skill tree. Main/script arms additionally require the existing
`cua_main` version, binary, git and skills identities. Never reuse the macOS pin
file. Existing model, quota, disk-space and pack-integrity gates still apply.
Keep `local-work/` outside git; the disposable-environment flag is still required
for pack cleanup.

`desktop_runtime.py` owns environment allowlisting, process groups, app command
resolution, driver launch/readiness, health parsing, doctor and screenshots.
Doctor connects to X11 and the AT-SPI registry and checks driver capabilities and
ffmpeg's x11grab support. Linux uses the existing `CuaRecorder` on its dedicated
control daemon, whose backend already uses x11grab; that daemon also handles
window placement. No separate Linux recording implementation is used.

The GTK helpers preserve BenchLab's state/event evaluator contracts. The sentinel
observes EWMH focus/stacking/workspaces, AT-SPI focus, root pointer coordinates,
clipboard ownership and transient windows. It never refocuses itself after arming.
XScreenSaver idle values include synthetic input: Linux HID counts and human-input
inference are **unmeasured**, not zero. Screenshots and app/sentinel/driver logs
are retained in trial evidence directories. Raw evidence and results are internal.
HID counts stay unmeasured on Linux.

Headless contract tests use independent `xdotool` XTest actions. The Linux CI job
runs doctor, GTK interactions, sentinel and screenshot tests without a driver;
only driver health and recording smoke requires `CUA_DRIVER_BIN`. No model is
called by these tests.
