# Cua Driver Bench definition

Cua Driver Bench evaluates complete computer-use agents on tasks where driver
behavior can change the result. It measures whether an agent completes the
task on the intended target, avoids prohibited effects, satisfies required
driver participation, and leaves the environment in the declared state.

## Evaluation unit

One trial binds a task, complete agent system, execution policy, environment,
attempt index, seed, and evidence set. Task outcome remains separate from
participation, apparatus certification, and comparison eligibility.

## Experiment conditions

The benchmark has five driver profiles:

- `native-bundle`;
- `bare-driver`;
- `harness-native-reference`;
- `normalized-facade`;
- `driver-only-diagnostic`.

The first four form the required driver comparison. The fifth is a restricted
diagnostic. The separate tool-surface study compares consolidated, factored,
and fine-grained interfaces while holding the backend and capability coverage
fixed.

Version 0.3 also defines a system track and controlled harness and model views.
See the [comparison view reference](docs/reference/comparison-views.md).

## Result contract

Results publish typed fields for support, environment eligibility, completion,
forbidden effects, wrong-target mutations, leaked input, termination, focus,
cursor movement, surface transitions, wall time, tokens, cost, tool use, and
skill delta where applicable.

A task may promote an observable to a graded outcome. The manifest identifies
the rule and the result retains the raw observation.

## Validity rules

- A separate observer or evaluator verifies each claimed outcome.
- Tasks declare allowed and forbidden side effects.
- Distinct profiles remain separate in results.
- Native-bundle results include the paired bare-driver condition.
- Normalized-facade results use frozen benchmark-owned instructions.
- Tool-surface arms keep capability coverage fixed.
- Delivered driver guidance is content-addressed and fixed before task
  selection.
- Tasks reset deterministically and carry provenance records.

## Related documentation

- [About the benchmark design](docs/explanation/benchmark-design.md)
- [Profiles, systems, and comparisons](docs/explanation/profiles-systems-and-comparisons.md)
- [Held-out task-pack interface](docs/reference/task-pack-interface.md)
- [Manifest reference](docs/reference/manifests.md)
- [Component map](docs/reference/component-map.md)
