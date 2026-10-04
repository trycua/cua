# Held-out task pack

Cua Driver Bench tasks, fixtures, evaluators, expected state, and evidence are
not distributed in this public repository. They remain a separately controlled
task pack.

Public runtime and conformance tests use synthetic fixtures only. Local
diagnostic runs must receive an authorized task directory explicitly rather
than assuming that tasks exist in the repository checkout.

Task-specific protected desktop applications, mediators, launch parameters,
participation facts, and end states also remain with the authorized task pack
or prepared seed. The public runtime provides only their generic interface and
validation apparatus.

The expected layout is:

```text
<tasks-root>/
  shared/
    <task-id>/
      task.cuabench.json
      platform/
        launch.<platform>.json
```

See the [task-pack interface](../docs/reference/task-pack-interface.md) for the
path contract and validation boundary.
