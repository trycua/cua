# About the benchmark design

Cua Driver Bench evaluates complete computer-use agents on work where driver
behavior can change the result.

## Background

A restricted GUI benchmark can isolate an input API, but it does not represent
an agent that also has coding, shell, filesystem, browser, accessibility,
memory, and orchestration tools. Cua Driver Bench keeps those native
capabilities enabled in its primary system track.

This choice changes the question. The benchmark asks whether a driver helps a
capable agent complete the intended work safely and repeatably. It does not
assume that every desktop effect came from the named driver.

## Final state and trajectory evidence

Task evaluators grade final state through an independent channel. They check
the intended result, protected data, decoys, forbidden mutations, and reset
state. Driver traces do not define task success.

The runtime records driver participation separately. This proves whether a
declared interaction sequence occurred through the protected computer-use
path. A shell-only solution can therefore receive the correct task score while
failing the GUI participation gate.

## Full systems and controlled slices

The system track compares frozen agent stacks with their declared tools,
skills, model routing, permissions, memory, and subagent behavior. Separate
controlled views hold those factors fixed where the evidence permits a narrow
harness, model, driver-profile, or tool-surface comparison.

Removing native tools to equalize systems would create a different system.
The benchmark instead records support, usage, routing, and participation so a
report can separate task success from the claimed cause.

## Independent enforcement

An agent or harness cannot certify its own isolation, model route, cleanup, or
desktop effects. Protected adapters obtain these facts from benchmark-owned
enforcers and observers. Missing evidence preserves the outcome and narrows
the claim.

## Task design

Tasks contain attractive near misses and protected state. Their evaluators use
fresh probes and hidden checks to distinguish a durable fix from a plausible
shortcut. Diagnostic weights explain partial progress, while a pass requires
every named acceptance check.

## Further reading

- [Your first verified trial](../tutorials/your-first-verified-trial.md)
- [Held-out task-pack interface](../reference/task-pack-interface.md)
- [Component map](../reference/component-map.md)
- [Benchmark definition](../../definition.md)
