# Provenance: CDB-S02 (Bench v2 task CDB-G02)

CDB-S02 is an original Cua task, written on 2026-08-16. Bench v2 runs it as
CDB-G02, its GUI-only variant: the same task files, with no coding tools
offered to the agent.

A Cua-produced customer sample was consulted only for abstract workload
structure: chained subtasks, cross-application handoffs, persistent final
artifacts, progressive difficulty, lifecycle hooks, weighted checks and
execution budgets. No task story, wording, domain, application identity,
fixture data, identifier, exact workflow, code, evaluator, oracle, UI copy or
recording was copied or adapted. The task is therefore classified as
**independently implemented with structural inspiration**, not as a strict
clean-room task.

The Lighthouse scenario, Copperfield Mail, Signal Quay Chat, Daymark Calendar,
all `LHP-*`, `RQ-*`, `CM-*` and `CAL-*` identifiers, the fixture data,
scheduling constraints, blocker mechanism, workbook, evaluator, oracle and
tests are original. LibreOffice is a runtime prerequisite, not a distributed
dependency.

## Related prior work

OSWorld 2.0 ([arXiv 2606.29537](https://arxiv.org/abs/2606.29537)) describes
two tasks with a similar mechanic: Task 001 reconciles a schedule from email
attachments with existing calendar conflicts, and Task 035 has a chat message
arrive mid-task that overrides earlier rules, with a protected row that must
not be overwritten. CDB-S02 shares the "late authoritative override" and
"protected record" ideas with them, and we credit OSWorld 2.0 as prior art for
that mechanic. The domain, applications, entities, workbook, wording and
checker of CDB-S02 are its own. OSWorld 2.0's task files are gated and were
not opened; the comparison is with the task descriptions in its paper.

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

Copyright (c) 2026 Cua AI, Inc. Released under the Functional Source License, Version 1.1, MIT
Future License (FSL-1.1-MIT) as part of
the public Cua Driver Bench task set (see [the task pack README](../../../README.md)).
This task was first written in Cua's private benchmark repository under a
proprietary licence; Cua relicensed it for this release (under MIT in
#4973 and #4978, under FSL-1.1-MIT since; see the [bench LICENSE](../../../../LICENSE)). Electron and its
lockfile-pinned dependencies are external and not vendored (see
[THIRD_PARTY_NOTICES.md](../../../THIRD_PARTY_NOTICES.md)).
