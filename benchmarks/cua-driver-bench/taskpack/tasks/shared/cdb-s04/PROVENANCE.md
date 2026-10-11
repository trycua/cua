# Provenance: CDB-S04 (Bench v2 task CDB-G04)

CDB-S04 is an original Cua task, written in August 2026. Bench v2 runs it as
CDB-G04, its GUI-only variant: the same task files, with no coding tools
offered to the agent.

A Cua-produced customer sample was consulted only for abstract workload
structure: chained subtasks, cross-application handoffs, persistent final
artifacts, progressive difficulty, lifecycle hooks, weighted checks and
execution budgets. No task story, wording, domain, application identity,
fixture data, identifier, exact workflow, code, evaluator, oracle, UI copy or
recording was copied or adapted. The task is therefore classified as
**independently implemented with structural inspiration**, not as a strict
clean-room task.

Morrowglass Archive, Rillwatch Cases, Margin Lantern Manual, all case and
article identifiers, the fixture data, reproduction behaviour, log
signatures, evaluator, oracle and tests are original.

## Related prior work

Triage against a manual with evidence is a common task genre (for example
Terminal-Bench's legacy utility triage). The domain, apps, data and checker
of CDB-S04 are its own.

## Prohibited-source similarity review

A Cua review on 2026-10-10 compared this task's text, fixture structure,
application flow and evaluator logic with public desktop, web and terminal
benchmarks: OSWorld, OSWorld 2.0 (from its paper), WindowsAgentArena,
macOSWorld, Mac Agent Bench, Hunch Bench, Terminal-Bench, SkillsBench,
Agents' Last Exam, WebArena, VisualWebArena, Mind2Web, AndroidWorld,
WorkArena, tau-bench and GDPval. It also compared every line of 40 or more
characters with the customer sample named above. Result: no near-copy or
structural copy, and no shared text, data or code. Similarity is at most
thematic (the same app or the same kind of work).

## Licence

Copyright (c) 2026 Cua AI, Inc. Released under the MIT License as part of
the public Cua Driver Bench task set (see [the task pack README](../../../README.md)).
This task was first written in Cua's private benchmark repository under a
proprietary licence; Cua relicensed it for this release. Electron and its
lockfile-pinned dependencies are external and not vendored (see
[THIRD_PARTY_NOTICES.md](../../../THIRD_PARTY_NOTICES.md)).
