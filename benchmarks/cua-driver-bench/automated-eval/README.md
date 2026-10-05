# Automated Driver Evaluation

This directory contains diagnostic tools for released Cua Driver versions.

The runner uses Codex as the agent. It uses the selected Cua Driver binary as the MCP server.

The tools can test one release or compare two releases. They support the four shared tasks, CDB-S01 through CDB-S04.

## Scope

These runs are non-certifying. They run on a local desktop, or on a runner you control through the manual GitHub Actions workflow. No hosted execution backend is involved.

The runner does not change task evaluators. Each task evaluator grades the final trial state independently.

Each selected release uses its explicit binary path. The runner does not use a globally installed Cua Driver.

## Requirements

- Use Python 3.11 or newer with this project installed. From the monorepo root, run
  `uv sync --project libs/cua-bench-runtime`.
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

To fetch a released macOS build into this layout, download
`cua-driver-rs-<version>-darwin-universal.tar.gz` and
`cua-driver-rs-v<version>-skills.tar.gz` from the `cua-driver-rs-v<version>`
GitHub release, verify them against `checksums.txt`, copy `cua-driver` into
`binary/`, and write a `release-manifest.json` with `version`, `binaryVersion`,
and `product`. The manual workflow does exactly this.

On macOS, grant Accessibility and Screen Recording to the app that starts the
runner, such as Terminal.

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

## Task Shards and Concurrency

The comparison builds one task shard for each selected task. Each shard runs the baseline before the candidate.

Different task shards can run at the same time. The final report always uses this canonical task order:

```text
CDB-S01
CDB-S02
CDB-S03
CDB-S04
```

Completion order does not change report order. If another shard has an infrastructure failure, a completed shard keeps its usable output.

Use `--max-parallel-tasks` to set the maximum number of active task shards.

- The default is `1`, which runs every shard in sequence.
- Parallel runs require Linux/X11 and one independent display for each active shard.
- macOS, Windows, and a single shared desktop run serially.

Use `--max-parallel-tasks 1` to run all task shards in sequence.

For a parallel local run, start the X11 sessions before you run the command. Then assign each session with a separate `--local-display` option.

```bash
python automated-eval/cli.py compare \
  --model large \
  --tasks-root /absolute/path/to/authorized/tasks \
  --task CDB-S01 \
  --task CDB-S03 \
  --max-parallel-tasks 2 \
  --local-display :91 \
  --local-display :92
```

The command starts one spawned process for each active display. Each process gets private home, cache, configuration, data, state, and temporary directories.

The command does not start X11 sessions or network namespaces. Use displays and application endpoints that do not conflict.

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

Reports also show required driver participation and recording coverage. A
required participation failure does not rewrite the evaluator result, but it
makes a baseline-versus-candidate signal incomplete. Recording coverage is the
number of successful input actions captured by Cua Driver recording compared
with the successful input actions counted from the agent transcript.

The participant prompt forbids direct backing-store, internal API, IPC, or
shell-based substitutes for state changes that the task requires through a GUI.
If the required GUI interaction fails, the agent must leave it incomplete and
report the blocker as a papercut.

## Manual GitHub Actions Run

The `Cua Driver Benchmark` workflow is manual. Start it with `workflow_dispatch` and provide these inputs:

- suite: `driver-comparison` or `macos-head-to-head`
- runner label, such as a self-hosted macOS runner
- Cua Driver release, and an optional candidate release
- comma-separated task IDs
- arms and runs per task (head-to-head only)
- model or provider tier
- whether to publish the report to S3

GUI runs need a desktop session with Accessibility and Screen Recording granted. GitHub-hosted runners cannot grant them, so use a self-hosted macOS runner for real runs.

The workflow downloads the selected release from GitHub and verifies its checksum. It retrieves only the selected authorized tasks from an access-controlled source. The task pack stays in runner temporary storage, a cleanup step removes it, and the result artifact does not include it.

Configure these repository settings before you run the `driver-comparison` suite:

| Setting | Kind | Purpose |
| --- | --- | --- |
| `CDB_TASK_PACK_GH_TOKEN` | secret | Read access to the task-pack repository |
| `CDB_TASK_PACK_REPOSITORY` | variable | Task-pack repository, as `owner/name` |
| `CDB_TASK_PACK_REF` | variable | Task-pack ref to check out |
| `OPENAI_API_KEY` | secret | Model gateway key |
| `OPENAI_BASE_URL` | variable | Model gateway URL |

The workflow uploads the complete result bundle as the GitHub artifact `cua-driver-bench-<run-id>` and writes refs, tasks, benchmark status, and artifact status to the run summary.

## Publish a Static Report

Publishing is off by default. It runs only when you pass `--publish`, or when you run the `publish` command. Neither path runs without an `AWS_S3_BUCKET` value.

Publishing uses the AWS CLI and the standard AWS credential chain. No credentials or bucket names live in this repository. Set these values in your environment or in `automated-eval/.env`, which Git ignores:

```env
AWS_PROFILE=<your profile>
AWS_REGION=<your region>
AWS_S3_BUCKET=<your bucket>
AWS_S3_REPORT_PREFIX=cua-driver-bench
```

Publish an existing completed run without rerunning the benchmark:

```bash
python automated-eval/cli.py publish \
  --run-dir artifacts/automated-eval/<run>
```

Or publish automatically after a local comparison by passing `--publish` to the `compare` command.

The publisher uploads only `report/`, `comparison.md`, and `comparison.json`.
It keeps the local run unchanged and prints one machine-readable line:

```text
REPORT_URL=https://...
```

The S3 report prefix must already be publicly readable. The publisher checks
the generated HTTP URL. If S3 returns `403` or `404`, the public URL check fails.
The publisher does not change bucket policy, Block Public Access, ACLs, or
CloudFront configuration.

The manual workflow publishes only when `publish_report` is set, the `CDB_REPORT_S3_BUCKET` variable exists, and the `CDB_REPORT_AWS_ACCESS_KEY_ID` and `CDB_REPORT_AWS_SECRET_ACCESS_KEY` secrets exist. Otherwise it skips the step. The GitHub artifact remains available either way.

## Main Options

| Option | Purpose |
| --- | --- |
| `--baseline` | Select the baseline release, or select one release without `--candidate`. |
| `--candidate` | Select the candidate release, or select one release without `--baseline`. |
| `--task` | Select one shared task. Repeat the option to select more tasks. |
| `--model` | Select the Codex model or configured provider tier. |
| `--reasoning-effort` | Set Codex reasoning effort to `low`, `medium`, or `high`. |
| `--timeout` | Set the maximum seconds for each trial. The default is 1800 seconds. |
| `--max-parallel-tasks` | Set the task-shard concurrency limit. The default is `1`. |
| `--local-display` | Assign one independent Linux/X11 display to a local task shard. Repeat this option for parallel local runs. |
| `--output` | Select a new output directory. The directory must not exist. |
| `--tasks-root` | Select the `tasks/` directory in the authorized task pack. Required. |
| `--drivers-root` | Select the directory that contains released driver folders. |
| `--codex` | Select the Codex executable name or path. |
| `--codex-home` | Select the source Codex configuration and authentication directory. |
| `--platform` | Select `auto`, `linux`, `windows`, or `macos`. |
| `--dry-run` | Print the trial plan without starting the benchmark. |
| `--publish` | Publish the generated static report bundle after a successful run. |

Run this command for the complete option list:

```bash
python automated-eval/compare_drivers --help
```

## Result Locations

Runs write to `artifacts/automated-eval/<UTC timestamp>/` by default. Use `--output` to choose another new directory.

Serial and parallel runs produce the same final report contract:

```text
<output>/
  comparison.json
  comparison.md
  report/
    index.html
  launchers/
  runtime/
  trials/
```

Task shards can finish in any order. Aggregation sorts tasks by canonical task ID and keeps baseline and candidate identity explicit.

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
- Each concurrent task shard requires its own isolated Linux/X11 display.
- The reports are diagnostic and cannot support certification claims.
- Foreground disturbance metrics are available only on Linux/X11.
- Model and token values come from Codex telemetry and remain diagnostic.
