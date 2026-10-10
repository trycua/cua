# How to run and verify a task trial

This guide shows you how to execute one registered task and verify the runtime
evidence before using its result.

## When to use this guide

Use this guide for local lifecycle checks and diagnostic task runs. Protected
production execution requires separately authorized infrastructure and the
additional controls described in the
[protected debug-mode reference](../reference/protected-debug-mode.md).

## Before you start

- Install the runtime from the repository root.
- Select a task manifest and an agent executable.
- Choose an output directory that does not contain the requested trial ID.

## Run the trial

```console
cdb run \
  --task path/to/task.cuabench.json \
  --agent path/to/agent \
  --out /absolute/path/to/trials \
  --trial-id trial-id \
  --seed 1 \
  --env local
```

`cdb run` returns zero for a completed lifecycle even when the evaluator grades
the task as failed. Harness faults, timeouts, validation errors, interruptions,
and cleanup failures use separate exit codes.

## Verify the trial

Verify the evidence before reading or exporting the result:

```console
cdb explain --verify-only /absolute/path/to/trials/trial-id
```

Then inspect the decisions:

```console
cdb explain /absolute/path/to/trials/trial-id
```

Treat the following fields independently:

- `evaluation` records the task outcome and score;
- `participation` records required driver interactions;
- `apparatus_certification` records the protected execution boundary;
- `comparison_eligible` records whether observed system facts support the
  requested comparison.

## Choose a local environment

| Environment | Purpose | Certifying |
| --- | --- | --- |
| `local` | Headless lifecycle execution | No |
| `local-cua-smoke` | Synthetic local Cua Driver diagnostics | No |
| `task-local-smoke` | Task reset and external desktop-agent diagnostics | No |
| `task-local-cua-smoke` | Task reset plus local Cua Driver recording | No |
| `lume-macos` | Lume control-plane diagnostics | No |
| `lume-macos-certifying` | Protected macOS execution | Conditional |

## Troubleshooting

**The trial directory already exists.** Choose a new `--trial-id` or a fresh
output root. Runtime publication is one-shot.

**The task failed but `cdb run` returned zero.** This is expected for a graded
task failure. Read `evaluation` in the verified result.

**`cdb explain --verify-only` fails.** Do not export or report the trial. Retain
the directory and diagnose the named digest, event-chain, receipt, or cleanup
failure.

## Related guides

- [Export and report trial results](export-and-report-results.md)
- [CLI reference](../reference/cli.md)
- [Trial artifact reference](../reference/trial-artifacts.md)
