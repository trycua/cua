# Automated Driver Evaluation

This directory contains diagnostic tools for released Cua Driver versions.

The runner uses Codex as the agent. It uses the selected Cua Driver binary as the MCP server.

The tools can test one release or compare two releases. They support the four shared tasks, CDB-S01 through CDB-S04.

## Scope

These runs are non-certifying. They can run on one local desktop or on isolated Cua Fleet workers.

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

- Fleet runs use `2` by default.
- Local runs use `1` by default.
- Parallel local runs require one independent Linux/X11 display for each active shard.

Concurrent shards share the configured model provider. Higher concurrency can increase latency or cause provider capacity errors. Use `1` for latency-sensitive comparisons.

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

## Run on Cua Fleet

Fleet runs the same comparison logic on isolated Linux/X11 workers. Each active task shard uses one worker.

The worker runs the baseline and candidate in sequence for its task. The controller can run multiple task shards at the same time.

The controller uploads only the public benchmark source, runtime, selected drivers, and selected authorized tasks. Task content remains separate from the public source archive.

From the monorepo root, install the Fleet extra:

```bash
python -m pip install -e 'libs/cua-bench-runtime[fleet]'
```

Put Fleet and model credentials in `benchmarks/cua-driver-bench/automated-eval/.env`. If `OPENAI_BASE_URL` is reachable only through a tailnet, set `OPENAI_FLEET_BASE_URL` to a provider URL that the worker can reach.

Run the controller from the benchmark directory:

```bash
cd benchmarks/cua-driver-bench
python automated-eval/cli.py fleet \
  --model small \
  --reasoning-effort high \
  --tasks-root /absolute/path/to/authorized/tasks \
  --drivers-root cua-drivers \
  --baseline 0.28.0 \
  --candidate 0.26.1 \
  --task CDB-S01 \
  --task CDB-S04 \
  --max-parallel-tasks 2
```

Fleet reads the selected task descriptors and provisions the required external applications. Each worker returns a self-contained task result bundle.

The controller aggregates all bundles into the standard report files. It releases each worker after result retrieval and on error paths.

Fleet results are written to `automated-eval/fleet-results/<UTC timestamp>/`.

## Manual GitHub Actions Run

The `Cua Driver Benchmark` workflow is manual. Start it with `workflow_dispatch` and provide these inputs:

- baseline Git ref
- candidate Git ref
- comma-separated task IDs
- maximum parallel task shards

The workflow builds isolated Linux drivers from both refs. It retrieves only the selected authorized tasks from an access-controlled source.

The task pack stays in runner temporary storage. A cleanup step removes it, and the result artifact does not include it.

The workflow starts the Fleet comparison and publishes an available report bundle to S3. It also uploads the scrubbed result bundle as a GitHub artifact. The report and summary identify incomplete infrastructure execution.

The artifact name is `cua-driver-bench-<run-id>`. The workflow summary includes refs, resolved SHAs, tasks, benchmark status, S3 status, and artifact status.

## Publish a Static Report

Publishing uses the AWS CLI and the standard AWS credential chain. Configure the
`cua-artifacts` profile locally, then set these non-secret values in
`automated-eval/.env`:

```env
AWS_PROFILE=cua-artifacts
AWS_REGION=us-west-2
AWS_S3_BUCKET=cua-agent-artifacts
AWS_S3_REPORT_PREFIX=cua-driver-bench
CDB_REPORT_BASE_URL=https://bench.trycua.com
```

Publish an existing completed run without rerunning the benchmark:

```bash
python automated-eval/cli.py publish \
  --run-dir automated-eval/fleet-results/<run>
```

Or publish automatically after a local comparison or Fleet run by passing
`--publish` to the `compare` or `fleet` command.

The publisher uploads only `report/`, `comparison.md`, and `comparison.json`.
It keeps the local run unchanged and prints two machine-readable lines:

```text
S3_URI=s3://cua-agent-artifacts/cua-driver-bench/<run-id>/
REPORT_URL=https://bench.trycua.com/<run-id>/index.html
```

After uploading, the publisher uses authenticated AWS access to verify that
`index.html` exists in S3. It does not make an anonymous request to the raw S3
object URL. `REPORT_URL` points to the Cloudflare Access-protected report site.
The publisher does not change bucket policy, Block Public Access, ACLs,
Cloudflare, or other infrastructure.

## Main Options

| Option | Purpose |
| --- | --- |
| `--baseline` | Select the baseline release, or select one release without `--candidate`. |
| `--candidate` | Select the candidate release, or select one release without `--baseline`. |
| `--task` | Select one shared task. Repeat the option to select more tasks. |
| `--model` | Select the Codex model or configured provider tier. |
| `--reasoning-effort` | Set Codex reasoning effort to `low`, `medium`, or `high`. |
| `--timeout` | Set the maximum seconds for each trial. The default is 1800 seconds. |
| `--max-parallel-tasks` | Set the task-shard concurrency limit. Fleet defaults to `2`. Local runs default to `1`. |
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

Local runs use `artifacts/automated-eval/<UTC timestamp>/` by default.

Fleet runs use `automated-eval/fleet-results/<UTC timestamp>/` by default.

Both execution paths produce the same final report contract:

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

Exit code `2` means that infrastructure execution was incomplete. A completed run can still contain failed tasks. Read the report for each task result.

## Limits

- The runner performs one attempt for each selected task and release.
- Each concurrent task shard requires an isolated executor and desktop.
- The reports are diagnostic and cannot support certification claims.
- Foreground disturbance metrics are available only on Linux/X11.
- Model and token values come from Codex telemetry and remain diagnostic.
