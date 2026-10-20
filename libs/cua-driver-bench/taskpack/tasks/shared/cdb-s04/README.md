# CDB-S04: Linked support-case diagnosis

This shared macOS, Windows, and Linux task measures whether an agent can
combine ticket reports, a separate wiki, live browser reproduction, and
correlated terminal logs into a correct support-case graph.

The fixture contains two true duplicates and two plausible near-misses. The
evaluator grades final state independently of the action trajectory: confirmed
duplicates must link directly to the root with case-specific evidence;
unsupported reports must remain open; all source material and unrelated case
content must remain unchanged.

Shell, code, filesystem, browser, and computer-control tools are permitted.
No application interaction path receives credit by itself.

## Development checks

The implementation must provide deterministic reset, generated platform
descriptors, pinned evaluator inputs, adversarial evaluator tests, Electron app
tests and task/schema validation.

Run the task's tests with `python3 -m unittest discover -s tests` from this
directory, and see [the task pack README](../../../README.md) for the
checks to run after changing task-owned inputs.
