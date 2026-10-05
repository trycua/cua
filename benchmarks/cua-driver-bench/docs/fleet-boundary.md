# Fleet execution boundary

The benchmark separates comparison logic from the current cloud execution backend. Cua Fleet is the current cloud backend.

No generic executor interface exists yet. The boundary is a small task-shard contract in the existing comparison and Fleet code.

## Backend-neutral behavior

The following behavior does not depend on Fleet:

- task selection and canonical task order
- baseline and candidate release selection
- one task shard for each selected task
- sequential baseline and candidate trials inside one shard
- the `--max-parallel-tasks` concurrency limit
- deterministic aggregation of completed shard output
- trial metrics, participation data, recording data, and Papercut data
- JSON, Markdown, and HTML report generation
- S3 report publishing

The scheduler can run different task shards at the same time. It returns shard results in canonical task order, not completion order.

A shard can return task-level output with a failed score, timeout, or incomplete participation. This output remains useful benchmark evidence.

An infrastructure failure stays separate from task-level output. When preservation is safe, completed shard output remains available.

## Fleet-specific behavior

The following operations remain in the Fleet execution code:

- Fleet authentication and pool selection
- worker claim and keepalive
- Fleet image selection
- remote file upload and download
- remote shell commands
- Linux/X11 worker checks
- external application provisioning
- model endpoint checks on the worker
- worker release

Each concurrent Fleet shard claims one isolated worker. The worker runs the baseline before the candidate for that task.

The controller retrieves the shard result bundle before it releases the worker. Error paths also attempt secret cleanup and worker release.

## Local execution

A normal local run uses one desktop. Its default is `--max-parallel-tasks 1`.

A parallel local run requires Linux/X11. Pass one unique `--local-display` option for each active task shard.

The local scheduler starts one spawned process for each active display. Each process gets private user and temporary directories.

The operator must start the X11 sessions before the run. The scheduler does not create displays, virtual machines, or network namespaces.

Windows, macOS, and a single shared desktop remain serial. Local and Fleet runs produce the same final report files.

## Private tasks and driver builds

Authorized task content stays outside the public monorepo. Local users pass an authorized `tasks/` directory with `--tasks-root`.

The manual GitHub Actions workflow retrieves only the selected tasks from an access-controlled source. It stores them in temporary runner storage.

The workflow removes the temporary task source after the benchmark step. It does not add task source content to the GitHub artifact.

The workflow resolves baseline and candidate Git refs to commits. It builds one isolated Linux driver release for each resolved commit.

The Fleet controller stages the public benchmark source, selected drivers, runtime files, and selected tasks on each required worker.

## Result aggregation

Each task shard produces a self-contained result bundle. The bundle contains its trial output and task result data.

The controller aggregates shard bundles into these files:

```text
comparison.json
comparison.md
report/index.html
trials/
```

Aggregation uses canonical task order. It keeps baseline and candidate identity explicit in every comparison.

The final report contract is the same for serial and parallel runs. It is also the same for local and Fleet runs.

## Replacing Fleet

A future backend must provide one isolated executor for each active task shard. Local Linux can use prestarted X11 sessions for this boundary.

The backend must perform these operations:

1. Acquire an isolated executor.
2. Stage the benchmark inputs.
3. Run one task shard.
4. Retrieve the shard result bundle.
5. Release the executor.

A replacement does not need to change comparison logic, aggregation, report generation, or S3 publishing.
