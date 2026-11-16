# Provenance: CDB-S03 (Bench v2 task CDB-G03)

CDB-S03 is an original Cua task, written in August 2026. Bench v2 runs it as
CDB-G03, its GUI-only variant: the same task files, with no coding tools
offered to the agent.

A Cua-produced customer sample was consulted only for abstract workload
structure: chained subtasks, cross-application handoffs, persistent final
artifacts, progressive difficulty, lifecycle hooks, weighted checks and
execution budgets. No task story, wording, domain, application identity,
fixture data, identifier, exact workflow, code, evaluator, oracle, UI copy or
recording was copied or adapted. The task is therefore classified as
**independently implemented with structural inspiration**, not as a strict
clean-room task.

Kestrel Wharf Workshop, Ledger Lantern Mail, all invoice and ledger
identifiers, the fixture data, reconciliation cases, workbook, GnuCash book,
evaluator, oracle and tests are original.

The GnuCash XML fixture and parser were written independently against the
public [GnuCash 5 storage documentation](https://www.gnucash.org/docs/v5/C/gnucash-guide/basics-files1.html)
and [backup and lock behaviour](https://www.gnucash.org/docs/v5/C/gnucash-guide/basics-backup1.html).
No GnuCash source code or sample book is vendored. GnuCash and LibreOffice are
runtime prerequisites, not distributed dependencies.

## Related prior work

Matching invoices against other records is a common task genre (for example
OSWorld's multi-app invoice and bank-statement cross-check). CDB-S03 shares
no text, data, accounts or checker code with any public task.

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
