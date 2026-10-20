# Platform launch descriptors

The three descriptors share an identical `semantics` block: task identity,
brief, workspace layout, lifecycle commands, and required application roles.
Only executable paths, prerequisites, and window placement vary by operating
system.

`${workspace}`, `${bundle}`, `${artifacts}`, `${result}`,
`${agent_exit_code}`, and `${python}` are resolved by the environment harness.
`${workspace_uri}` is the absolute workspace encoded as a platform-correct
`file://` URI (including `file:///C:/...` on Windows).
The harness starts three separate Lighthouse Desk Electron processes with
distinct product names and durable stores, plus LibreOffice Calc on the same
ODS workbook.

A certifying harness exposes the brief, fixture inputs, launch assets, and
application package to the agent. `evaluator/`, its oracle, and evaluator-only
inputs exist only in the evaluator view. `task-local-smoke` cannot enforce that
split and always records `certifying: false`.
