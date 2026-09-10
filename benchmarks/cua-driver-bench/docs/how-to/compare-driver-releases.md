# Compare local Cua Driver releases

Use the automated-evaluation runner for a local, non-certifying comparison of
explicit Cua Driver binaries.

## Prepare the runtime

From the monorepo root:

```console
uv sync --project libs/cua-bench-runtime
```

Prepare an authorized task pack and local driver-release directory using the
layouts in the [task-pack interface](../reference/task-pack-interface.md) and
the [runner reference](../../automated-eval/README.md).

## Inspect the plan

```console
uv run --project libs/cua-bench-runtime \
  python benchmarks/cua-driver-bench/automated-eval/compare_drivers \
  --tasks-root /absolute/path/to/authorized/tasks \
  --drivers-root /absolute/path/to/cua-drivers \
  --model small \
  --reasoning-effort high \
  --baseline 0.23.2 \
  --task CDB-S01 \
  --dry-run
```

Remove `--dry-run` only after verifying the selected task, release, platform,
Codex configuration, and output directory. A generated comparison is diagnostic
even when every task passes; it does not use the protected certification path.
