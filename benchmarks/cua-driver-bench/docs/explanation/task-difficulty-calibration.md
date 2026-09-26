# Task difficulty calibration

This specification calibrates the ten original Cua Driver Bench portfolio tasks
without importing task text, fixtures, evaluators, workflows, or implementations
from another benchmark. It is a pre-release gate, not a leaderboard scoring rule.

The method borrows evaluation practices—not task content—from
[SkillsBench](https://www.skillsbench.ai/skillsbench.pdf),
[OSWorld 2.0](https://arxiv.org/abs/2606.29537),
[Terminal-Bench/Harbor](https://github.com/harbor-framework/terminal-bench/blob/main/CONTRIBUTING.md#task-difficulty),
and [Agents' Last Exam](https://github.com/rdi-berkeley/agents-last-exam):
multi-system pilots, valid-run accounting, executable outcome checks, failure
analysis, hidden references, and contamination controls.

## Scope and distribution

The candidate registry contains exactly ten portfolio tasks and must maintain this
distribution:

| Band | Count | Frontier-system target | Mid-level-system target | Purpose |
| --- | ---: | --- | --- | --- |
| `anchor` | 2 | 70--90% valid-run completion | 25--60% | Establish a usable floor and detect regressions. |
| `discriminating` | 5 | 30--70% | 5--35% | Main comparative signal. |
| `frontier` | 3 | 5--30% | 0--10% | Test authentic long-horizon and platform-specific limits. |

The initial band assignments are hypotheses for piloting, not claims about
measured difficulty. A task becomes `accepted` only after its valid trajectories
and failure analysis support the assigned band.

The two systems are predeclared archetypes, not post-hoc selected models: a
frontier system and a materially less capable mid-level system. For a pilot,
freeze the exact model route, harness, tool surface,
permission and network policy, limits, and environment image for each archetype.
Do not make a task harder merely by adding app hops, hidden conventions, or
resource restrictions. Every required application transition must carry state or
evidence needed by a later outcome.

## Pilot protocol

Each candidate receives three independent attempts for each of the two system
archetypes: 3 attempts x 2 systems = 6 planned attempts per task, or 60 planned
attempts for the registry. Reset verification precedes every attempt. Randomize
task order within each system. A fourth attempt per system is permitted only when
the three-attempt point estimate does not resolve the assigned band and one more
valid outcome could move the estimate into or out of that band, or when the three
attempts show materially inconsistent valid outcomes; record the reason. Report
Wilson 95% intervals as uncertainty; with only three or four attempts they are
not required to nest inside the target band.

A **valid run** has a successful reset and terminates as `completed`,
`agent_stop`, `timeout`, or `cost_limit`. `infrastructure_error` and
`reset_failure` are excluded from completion-rate denominators and reported
separately with a cause code. A valid run can still be a failure.

Before every pilot batch, execute the oracle, no-op fixture, each near-miss
fixture, preservation checks, and reset verification. Grade final state through
the independent evaluator, never from an agent claim or driver return.

## Calibration decisions

**Keep** a candidate when the oracle and negative fixtures are stable, the
completion point estimate for each archetype is inside its assigned target band,
registry-wide invalid runs are below 5% of all launched pilot attempts, and
failed trajectories concentrate in declared capability cruxes rather than
ambiguity or apparatus faults. Always publish the Wilson interval beside the
point estimate; it expresses pilot uncertainty rather than acting as a
three-to-four-attempt acceptance gate.

**Revise** when either archetype's completion point estimate remains outside the
candidate's assigned target band after the permitted fourth attempt; when
frontier completion is below that band's minimum without meaningful checkpoint
progress; when registry-wide invalid runs exceed 10% of all launched pilot
attempts; or when failures show an accidental timing dependency, ambiguous
instruction, hidden evaluator rule, or nonessential transition. The invalid-run
rates are aggregated across the registry pilot batch, never computed per task or
against only valid runs. Revise the smallest independent scenario, fixture, or
evaluator element; bump `candidate_version` and re-pilot the changed candidate.

**Drop** (or replace under a new candidate ID) after one revision cycle if oracle
or negative fixtures remain unstable, infrastructure cannot be separated from
capability, a prohibited-source similarity review fails, or no authentic
discriminating crux remains. Never silently retune an accepted or released task:
retain its prior registry record, mark it retired, and link its replacement.

## Leakage and clean-room controls

Candidates are independently written original scenarios. Any prohibited
reference corpus may inform only abstract workload properties such as
application count, handoff density, artifact flow, ambiguity, and difficulty.
It must not supply task text, story, domain, exact app sequence, data, fixture,
code, evaluator, oracle format, identifier, or recording. Each candidate records
author exposure, clean-room review, prohibited-source similarity review, and a
unique canary. The hidden oracle/reference stays outside the agent-visible
environment and is staged only for grading. Agent-visible skills and guidance may
not contain candidate-specific identifiers, filenames, paths, constants, command
sequences, test references, or expected outputs.

## Required evidence

For each candidate, retain the frozen archetype configuration digests, valid and
excluded-run counts, Wilson intervals, median and p90 steps/time/cost, checkpoint
progress, failure taxonomy, oracle/no-op/near-miss/reset evidence, and decision
history. The candidate registry records targets and lifecycle controls; pilot
result records will be added in a later schema before any candidate can become
`accepted`. Task manifests remain the source of truth for execution and evaluator
contracts.
