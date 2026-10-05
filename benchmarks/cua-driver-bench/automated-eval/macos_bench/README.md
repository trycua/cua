# macOS head-to-head: Claude Code with Cua Driver vs Claude Code with Codex computer use

A local, rerunnable comparison of two computer-use tool layers with the same model, the same agent harness, the same
prompts and the same limits. Both arms run `claude -p --model claude-sonnet-5-5`; only the MCP tools differ (arm A also
gets the Cua Driver skill). It was started to test a public claim whose eight tasks were never published, so the tasks,
the test app, the checkers, the harness and the pre-registration are all here.

Status: the harness is built and its unit tests pass. No analysed trial has run yet; the run waits on the Claude
subscription quota (see `PREREGISTRATION.md`, section 0).

## Read in this order

| File | What it is |
| --- | --- |
| [PREREGISTRATION.md](PREREGISTRATION.md) | The protocol, fixed before any analysed trial: arms, versions, tasks, schedule, metrics, stopping rules, what the results cannot show |
| [TASKS.md](TASKS.md) | The twelve tasks (MB-01 to MB-12): exact prompts, setup, checkers, limits, validation evidence |
| [RUNNER.md](RUNNER.md) | How the runner works: the `claude -p` command, isolation, schedule, quota gate, recording, set-up from a clean checkout |

## Layout

| Path | Purpose |
| --- | --- |
| `run_bench.py`, `bench_core.py`, `claude_arms.py`, `claude_driver.py`, `claude_events.py`, `recorder.py` | The Claude Code runner |
| `analyze_bench.py`, `analyze.py` | Analysis on complete task blocks (Wilson intervals, paired bootstrap, cost and token tables) |
| `probes/MB-*` | The twelve tasks: brief template, seeds, checker, limits |
| `swift/` | `BenchLab` (the test app) and `BenchSentinel` (focus, keystroke, click and pointer witness) |
| `tools/` | Fixture lifecycle, oracle validation, pinned driver installer, arm B MCP config writer |
| `launch/` | One-command launcher and a live status viewer |
| `pins.json` | The versions the preflight enforces |
| `run_pilot.py`, `arms.py`, `codex_events.py`, `probes/PROBE-*` | The earlier Codex CLI pilot harness. Kept for the optional Codex-side supplement; its CDB task support needs the private task pack |

## Run

```bash
export CDB_BENCH_WORK=~/.cache/cua-bench-h2h
swift/build.sh "$CDB_BENCH_WORK/build"
python3 tools/install_cua_driver.py
python3 run_bench.py preflight --build-dir "$CDB_BENCH_WORK/build"
open -a Terminal launch/launch_bench.command
```

The launcher refuses to run while `$CDB_BENCH_WORK/HOLD` exists or the latest known 7-day quota is at or above 0.95.
Arm B needs the ChatGPT app at the pinned version and uses OpenAI's own plugin launcher; read the terms note in
PREREGISTRATION.md section 10 before publishing anything from it.

Keep results, traces and recordings out of git.
