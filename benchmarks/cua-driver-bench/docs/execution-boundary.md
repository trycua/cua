# Execution boundary

The comparison runner runs on machines you control. It starts the selected
Cua Driver release, the agent, and the task applications on the local desktop,
and it writes every result to a local directory. No hosted execution backend is
involved.

## Task shards

The runner builds one task shard for each selected task. A shard runs the
baseline release before the candidate release.

Different shards can run at the same time. The final report always uses the
canonical task order, not completion order. A shard can return task-level
output with a failed score, timeout, or incomplete participation. This output
remains useful benchmark evidence.

An infrastructure failure stays separate from task-level output. When
preservation is safe, a completed shard keeps its usable output.

## Local execution

A normal run uses one desktop. Its default is `--max-parallel-tasks 1`.

A parallel run requires Linux/X11. Pass one unique `--local-display` option for
each active task shard. The scheduler starts one spawned process for each
active display, and each process gets private home, cache, configuration, data,
state, and temporary directories.

The operator starts the X11 sessions before the run. The scheduler does not
create displays, virtual machines, or network namespaces.

macOS, Windows, and a single shared desktop run serially.

## Private tasks and driver releases

Authorized task content stays outside the public monorepo. Pass an authorized
`tasks/` directory with `--tasks-root`.

The manual GitHub Actions workflow retrieves only the selected tasks from an
access-controlled source and stores them in temporary runner storage. It
removes the temporary task source after the benchmark step and does not add
task content to the GitHub artifact.

The workflow downloads the selected Cua Driver release from GitHub and checks
it against the release checksums. Locally, put releases under `cua-drivers/` or
pass `--drivers-root`.

## Result aggregation

The runner aggregates shard output into these files:

```text
comparison.json
comparison.md
report/index.html
trials/
```

Aggregation keeps baseline and candidate identity explicit in every comparison.
The report contract is the same for serial and parallel runs.

## Report publishing

Publishing the static report to S3 is a separate, opt-in step. It runs only
when you pass `--publish` or run the `publish` command with your own bucket and
credentials. See [Publish a static report](../automated-eval/README.md#publish-a-static-report).
