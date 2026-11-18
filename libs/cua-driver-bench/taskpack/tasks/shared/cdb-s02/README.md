# CDB-S02: Conflicting-change replan

This shared macOS/Windows/Linux task measures whether an agent notices that a
later authoritative update invalidates a tentative plan, then reconciles
durable state across four applications without erasing the conflict.

The agent reads Copperfield Mail and Signal Quay Chat, edits Daymark Calendar
and `lighthouse-replan.ods` in LibreOffice Calc, and records a structured
blocker for the one unschedulable session. Final-state grading is independent
of the agent's tools or claimed trajectory.

## Required outcome

- `LHP-ASTER` is cancelled and absent from the calendar.
- `LHP-BOREAL` is confirmed at 11:00–12:00 in Juniper.
- `LHP-CINDER` is confirmed at 13:00–13:30 in Juniper, after Boreal and lunch.
- `LHP-DUNE` remains off the calendar and is marked `blocked:LHP-C17` in Calc.
- Signal Quay contains one audited `LHP-C17` blocker against protected
  `CAL-HOLD-31` with escalation `program-lead`.
- Mail, chat messages, protected events, workbook controls, and unrelated
  records remain unchanged.

The evaluator checks calendar state, hard constraints, workbook/calendar
agreement, blocker content, blocker audit consistency, mail preservation, and
decoy/protected state. Every check is required; weights provide diagnostic
partial credit only.

## Development

Run the task's tests with `python3 -m unittest discover -s tests` from this
directory, and see [the task pack README](../../../README.md) for the
checks to run after changing task-owned inputs.

The ODS evaluator compares logical cells rather than ZIP bytes because Calc
may rewrite package metadata. Reset verification does compare the pristine
input byte for byte. A local desktop smoke is non-certifying unless the
environment enforces evaluator secrecy and the declared network policy.
