# Automated Driver Evaluation

This directory contains a local diagnostic runner for released Cua Driver versions.

The runner uses Codex as the agent. It uses the selected Cua Driver binary as the MCP server.

The runner can test one release or compare two releases. It supports the four shared tasks, CDB-S01 through CDB-S04.

## Scope

These runs are local and non-certifying. They do not use the protected production report path.

The runner does not change task evaluators. Each task evaluator grades the final trial state independently.

Each selected release uses its explicit binary path. The runner does not use a globally installed Cua Driver.

## Requirements

- Use Python 3.11 or newer with this project installed.
- Install and configure Codex CLI.
- Obtain an authorized task pack and pass its `tasks/` directory with
  `--tasks-root`.
- Put released drivers under `cua-drivers/`, or use `--drivers-root`.
- Install the prerequisites declared by each selected task.
- On Linux, start X11 or XWayland and set `DISPLAY`.

Use this release layout:

```text
cua-drivers/
  0.23.2/
    release-manifest.json
    binary/
      cua-driver
    cua-driver-rs-v0.23.2-skills.tar.gz
```

Use `cua-driver.exe` in the `binary/` directory on Windows.

## Check the Plan

Use `--dry-run` to check release and task selection. This command does not start applications, drivers, Codex, or evaluators.

```bash
python automated-eval/compare_drivers \
  --tasks-root /absolute/path/to/authorized/tasks \
  --model small \
  --reasoning-effort high \
  --baseline 0.23.2 \
  --task CDB-S01 \
  --dry-run
```

## Run One Release

Pass only `--baseline` or only `--candidate` to run one release.

```bash
python automated-eval/compare_drivers \
  --tasks-root /absolute/path/to/authorized/tasks \
  --model small \
  --reasoning-effort high \
  --baseline 0.23.2 \
  --task CDB-S01
```

The report keeps the comparison format. The comparison table has no rows.

Repeat `--task` to select more tasks. If you omit `--task`, the runner uses all four shared tasks.

## Compare Two Releases

Pass distinct baseline and candidate versions to compare two releases.

```bash
python automated-eval/compare_drivers \
  --tasks-root /absolute/path/to/authorized/tasks \
  --model large \
  --reasoning-effort high \
  --baseline 0.22.2 \
  --candidate 0.23.2
```

If you omit both version flags, the runner compares 0.22.2 with 0.23.2.

## Linux X11

Set `DISPLAY` before an actual Linux run.

```bash
export DISPLAY=:99
```

The selected platform must match the host for an actual run. For example, a Windows host cannot run a Linux trial.

The Linux observer records three diagnostic foreground metrics:

- unexpected keyboard focus drops
- interrupted foreground mouse drags
- physical cursor deviations from the initial static position

These metrics do not change scores, pass results, or comparison signals.

## Main Options

| Option | Purpose |
| --- | --- |
| `--baseline` | Select the baseline release, or select one release without `--candidate`. |
| `--candidate` | Select the candidate release, or select one release without `--baseline`. |
| `--task` | Select one shared task. Repeat the option to select more tasks. |
| `--model` | Select the Codex model or configured provider tier. |
| `--reasoning-effort` | Set Codex reasoning effort to `low`, `medium`, or `high`. |
| `--timeout` | Set the maximum seconds for each trial. The default is 1800 seconds. |
| `--output` | Select a new output directory. The directory must not exist. |
| `--tasks-root` | Select the authorized task pack's `tasks/` directory. Required. |
| `--drivers-root` | Select the directory that contains released driver folders. |
| `--codex` | Select the Codex executable name or path. |
| `--codex-home` | Select the source Codex configuration and authentication directory. |
| `--platform` | Select `auto`, `linux`, `windows`, or `macos`. |
| `--dry-run` | Print the trial plan without starting the benchmark. |

Run this command for the complete option list:

```bash
python automated-eval/compare_drivers --help
```

## Output

By default, the runner writes to `artifacts/automated-eval/<UTC timestamp>/`.

```text
<output>/
  comparison.json
  comparison.md
  launchers/
  runtime/
  trials/
```

The reports contain these main values:

- evaluator pass result and score
- total elapsed time
- all Cua MCP calls
- successful input actions
- Codex input, cached, and output tokens
- termination status
- Linux foreground disturbance metrics and availability
- baseline-to-candidate deltas for a two-release run

The runner preserves each raw trial directory. Keep those artifacts outside Git
and use them locally to investigate failures and reported metrics.

The command exit code reports runner errors. Read the generated report for individual task pass results.

## Limits

- The runner performs one attempt for each selected task and release.
- The reports are diagnostic and cannot support certification claims.
- Foreground disturbance metrics are available only on Linux/X11.
- Model and token values come from Codex telemetry and remain diagnostic.
