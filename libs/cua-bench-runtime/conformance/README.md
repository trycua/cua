# Runtime conformance reference

The conformance suite exercises the runtime lifecycle with external reference
agent and evaluator processes.

## Covered lifecycle cases

- successful task completion;
- graded task failure;
- timeout;
- cleanup-failure precedence;
- trial explanation and integrity checks;
- `not_required` participation for a headless task.

The runtime unit suite contains driver-gate cases for shell-only success,
unrelated application events, ordered action and readback, receipt signing, and
protected-observer trust.

## Boundary

Headless conformance proves no claim about display access, input injection,
accessibility, window management, desktop isolation, or a computer-use driver.
Platform adapters must add native driver and environment evidence before they
claim desktop certification.

## Files

| Path | Purpose |
| --- | --- |
| `run_conformance.py` | Artifact-level lifecycle suite |
| `agents/` | Reference success, failure, timeout, malformed, and adversarial agents |
| `tasks/` | Synthetic lifecycle task and evaluator |
Run it from the monorepo root with:

```console
uv run --project libs/cua-bench-runtime \
  python libs/cua-bench-runtime/conformance/run_conformance.py
```
