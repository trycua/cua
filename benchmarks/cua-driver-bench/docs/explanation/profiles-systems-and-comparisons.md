# About profiles, systems, and comparisons

Cua Driver Bench separates driver presentation from complete agent-system
identity so each report answers a defined question.

## Driver profiles

A profile changes how one driver candidate appears inside a fixed harness. The
native-bundle and bare-driver pair estimates the effect of shipped guidance.
The harness-native reference shows the harness without the evaluated bundle.
The normalized facade presents each candidate through the same frozen
interface and neutral guidance. The restricted driver-only diagnostic remains
outside headline results.

These conditions preserve task content and pairing coordinates.

## Complete systems

A system is one content-addressed agent configuration. It includes the harness
build, model routes, driver profile, tools, MCP servers, skills, permissions,
memory, retries, compaction, and subagent policy.

Benchmark-enforced limits remain in a separate execution-policy manifest. This
keeps the system identity stable when an experiment changes time, cost,
network, reset, or autonomy rules.

## Declared and observed execution

A system manifest records intended routing. Runtime evidence records the
served route, subagents, fallbacks, interventions, usage, and cost when those
facts are observable. A mismatch limits comparison eligibility but does not
erase the task outcome.

## System track

The system track compares frozen default or tuned stacks with their normal
capabilities. It answers which complete system performed best under one fixed
benchmark policy.

## Controlled comparisons

A harness comparison varies the harness while holding the observed model
route, driver, profile, and tool contract fixed. A model comparison varies the
declared and observed model while holding the harness and tool contract fixed.
Driver-profile and tool-surface views isolate their own factors.

These comparisons exist only when every paired arm and fixed-factor binding is
present. A gateway, compatibility layer, substituted model, or missing arm is
a different condition.

## Descriptive reports

A report remains useful when it cannot support its registered comparison. It
can state task outcomes, participation, certification, wall time, and
missingness while marking the controlled claim unavailable.

## Further reading

- [Profile reference](../../profiles/README.md)
- [Comparison view reference](../reference/comparison-views.md)
- [Public monorepo boundary decision](../decisions/0001-public-monorepo-boundary.md)
