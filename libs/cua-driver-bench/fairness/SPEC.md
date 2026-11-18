# Fairness and health checks

A benchmark comparison is only fair if every setup gets a working environment. When the harness fails (the tool
server never started, the app's data could not be read back, the environment went away), the agent did not fail,
and scoring that trial as a fail would penalise whichever setup happened to hit the fault. This document defines
the checks Cua Driver Bench runs around every trial, what a failure means, and how results are counted. It is
runner-agnostic: [`cdb_fairness.py`](cdb_fairness.py) implements every check with the Python standard library only
(3.9 or later), as functions and as a command line, so any runner can call it. The bench's own runner
(`automated-eval/macos_bench`) uses it; so can an AgentEnv provider or a hosted evaluation service.

## Outcomes

Every trial attempt has exactly one outcome:

| Outcome | Meaning | Counted |
| --- | --- | --- |
| `pass` | The task's checker passed every required check, and no GUI-only rule was broken | Yes |
| `fail` | The checker ran on a healthy environment and did not pass | Yes |
| `harness_error` | A check below failed, or the export step or the evaluator produced no result | **Never** |

A `harness_error` attempt is **re-run with the same seed** (same task, run index and condition, same setup). The
latest `pass` or `fail` attempt of a trial is the one that counts. A trial whose attempts are all harness errors
after the runner's retry limit (default 3 attempts) is reported as **excluded**, with its last reason, and is never
counted as a fail. Reports give the number of harness-error attempts and excluded trials per setup.

`cdb_fairness.classify(row)`, `scored_rows(rows)`, `rerun_plan(rows)` and `summarize(rows)` implement these rules on
trial rows with `arm` (or `setup`), `task`, `run_index`, `condition`, `passed`, and optionally `harness_error` (a
reason), `status` (`harness_error` or `infra_error`) and `health` (a list of check results). The command line
`cdb_fairness.py summarize rows.jsonl` prints the scored count, passes, and the trials to re-run or exclude.

## Seeds

```text
seed = int(sha256(f"{task_id}:{run_index}").hexdigest()[:8], 16) % 1_000_000
```

`run_index` counts from 0 for each task: trial k of a task (counting from 1) has run index k-1. Every setup gets the
same seed for the same task and run index, so setups are compared on the same task variants, and a re-run of a
harness-error attempt keeps its seed. `cdb_fairness.trial_seed(task, run_index)`; `cdb_fairness.py seed TASK N`.

## The checks

Each check returns `ok`, a short `reason` and `detail`. Any `ok: false` makes the attempt a `harness_error`. The
command line prints one JSON object and exits 0 when every check passed, **97** on a harness error, 2 on bad usage.

| When | Check | Function / command | Fails when |
| --- | --- | --- | --- |
| Before the agent starts | Tool-server preflight | `mcp_preflight_from_config(mcp.json)` / `cdb_fairness.py mcp MCP_JSON [--server NAME] [--probe-tool T]` | The setup's MCP server does not start, does not answer `initialize`, lists no tools within the timeout (default 60 s), or a given probe tool (for example a permissions check) returns an error |
| First event of the agent session | Session tool servers | `session_servers_connected(init_event)` / `cdb_fairness.py session-init STREAM` | The agent session's own `init` event shows a tool server that is not `connected` |
| After every reset | Apple events (macOS) | `appleevents_health(apps)` / `cdb_fairness.py appleevents APP ...` | An app the task's reset or export automates refuses a harmless Apple event: Automation consent missing or pending (-1743, -1744), sender not entitled (-10004), time-out (-1712), app gone (-600, -609) |
| After every reset | Network | `network_health(urls, require_lease)` / `cdb_fairness.py network [--url URL ...] [--require-lease]` | Loopback (127.0.0.1) cannot accept a connection; a task app's ready URL does not answer 200; with `--require-lease`, the environment has no usable IPv4 address (none, or self-assigned 169.254/16, a failed DHCP lease) |
| After every reset | Checker toolchain | `toolchain_health(programs, path)` with `programs_of_descriptor(descriptor)` / `cdb_fairness.py toolchain [PROGRAM ...] --descriptor D --path PATH` | A program the reset, export or checker needs is not on the PATH they get, or does not start (`python3`, `node` for a checker that starts the task's own web server, `osascript`, `xattr`) |
| During the agent phase | Environment watchdog | `EnvWatchdog(probe, interval, misses)` | The runner's liveness probe (the VM answers, the desktop session and the tool server process exist) fails `misses` times in a row (default 3, every 10 s) |
| After the agent | Export and evaluator | runner | The task's `semantics.export` step exits non-zero, or the evaluator writes no result |

What each task needs is in the task-set manifest
([`../taskpack/v2-taskset.json`](../taskpack/v2-taskset.json)), in each task's `health` block:

* `automation_apps`: the apps to pass to the Apple-events check;
* `ready_urls`: the local web app's ready URL (the runner waits on it when it starts the app);
* `evaluator_tools`: the programs for the toolchain check, in addition to those of the launch descriptor.

## Runner duties

A conforming runner:

1. derives seeds as above and records `run_index` and `seed` in every row;
2. runs the tool-server preflight before the agent, and the session check on the agent's first event;
3. after each reset, runs the Apple-events check for the task's `automation_apps` (macOS), the network check, and
   the toolchain check with the PATH and interpreters the evaluator will get;
4. runs an environment watchdog during the agent phase when the agent and the environment are on different
   machines;
5. records every check in the row (`health`), marks a failed attempt `harness_error` with its reason, re-runs it
   with the same seed, and counts only `pass` and `fail`;
6. reports harness-error attempts and excluded trials per setup next to the scores.

Health checks run as the same user, with the same environment, as the reset, export and evaluator they protect.
None of them touches task state, so running them never changes a task's outcome.

## How the bench's own runner maps onto this

`automated-eval/macos_bench/run_bench.py` records a harness error as `status: infra_error` with an
`infra_failure` kind (`mcp_start`, `tool_setup`, `harness_exception`, `harness_evaluator`, ...), re-runs it once and
then excludes it (`excluded: true`), and never counts it as a pass or a fail. `cdb_adapter.CdbTask.health_check`
runs the toolchain, loopback and Apple-events checks from this module after every pack-task reset, once the task's apps are up; a failed check raises, so
the attempt becomes a `harness_exception`. An export step that fails or an evaluator that writes no result becomes
`harness_evaluator`. The runner runs the agent in the same macOS VM as the apps, so it needs no separate watchdog;
a runner that drives a remote VM should add one.
