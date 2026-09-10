# Task-pack interface

The held-out Cua Driver Bench task pack is not distributed in this repository.
Commands that need it accept an explicit root rather than searching private or
machine-specific locations.

## Required layout

```text
<tasks-root>/
  shared/
    <lowercase-task-id>/
      task.cuabench.json
      platform/
        launch.linux.json
        launch.macos.json
        launch.windows.json
```

Each task manifest owns its referenced fixtures, reset commands, evaluator, and
platform descriptors. Paths must stay within the task directory and declared
artifacts must match their recorded SHA-256 digests.

## Protected desktop contract

A protected desktop task declares its launch contract in the first variant's
`parameters.protected_desktop` object:

```json
{
  "protected_desktop": {
    "app_id": "example-desk",
    "store_relative": "task-store",
    "launch_arguments": ["--record=ITEM-1042"]
  }
}
```

The contract has three required fields:

- `app_id` names a task-owned application under `task/apps/`.
- `store_relative` names one workspace directory that the protected apparatus
  snapshots for the console application.
- `launch_arguments` provides up to 16 bounded `--name=value` arguments. It
  cannot override the apparatus-owned `--store` argument.

These values belong to the authorized task pack. Do not duplicate a real task's
application, target identifier, required facts, or end state in the public
runtime or its tests.

## Protected mediator contract

The public macOS helper manages a root-owned mediator process, binds it to the
declared task ID and application process, and validates its sealed evidence. It
does not include a task-specific mediator executable.

The authorized task pack or prepared seed must install its mediator at
`/usr/local/libexec/cdb-driver-mediator` with the ownership, mode, and SHA-256
digest declared by the Lume configuration. The mediator must emit the generic
event and seal contract that the runtime validates. Use
`libs/cua-bench-runtime/guest/macos/install-cdb-helper.sh` to install the public
helper together with the authorized mediator source.

## Runtime boundary

Use `cdb validate --kind task <manifest>` before execution. Use the comparison
runner's required `--tasks-root` option for driver comparisons. Neither command
copies the task pack into the monorepo.

Task results, traces, screenshots, releases, and evidence remain outside Git.
Only synthetic fixtures under `libs/cua-bench-runtime/conformance/` are public.
Held-out integration tests also remain in the authorized task pack and must use
an explicit task root.
