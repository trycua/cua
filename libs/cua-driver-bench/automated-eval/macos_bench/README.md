# Bench v2 harness (macOS)

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

Keep results, streams and recordings out of git.
