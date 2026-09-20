---
title: Optional lightweight-model routing and planner handoff above Cua Driver
authors:
  - JkRheezy
created: 2026-09-21
last_updated: 2026-09-21
status: review
discussion: https://github.com/trycua/cua/issues/4013
rfc_pr: https://github.com/trycua/cua/pull/4014
implementation: []
supersedes:
superseded_by:
---

# RFC: Optional lightweight-model routing and planner handoff above Cua Driver

## Summary

Permit an optional experiment in the external `jev-use` recipe where a configurable bounded
chooser (Jev or a compatible local model) can request fresh observation or return a subgoal to the caller's
reasoning/visual planner. The host continues to own model selection, credentials,
permission policy, and the planner loop. Driver continues to own observation,
action validity, execution, and deterministic verification. This document asks
for a design decision; it contains no runtime implementation or performance claim.

## Motivation

A confident choice among available actions does not establish that those actions
can solve the next subgoal. Interpreting a document, resolving contradictory
requirements, or judging a canvas may require evidence or capabilities absent
from a text-based chooser. Conversely, invoking a large planner for every obvious
button choice may add avoidable cost and latency.

The missing example is an explicit return boundary: why a bounded chooser stopped,
what limited context the host may use, and what must happen before a planner's
answer can influence a new action. That boundary should not be inferred from
confidence alone or confused with permission to execute.

## Goals

- Preserve the existing single-chooser recipe with routing disabled by default.
- Allow caller-configured local or hosted routing models without mandatory Jev credentials.
- Distinguish fresh observation, reasoning assistance, and visual assistance.
- Demonstrate independent capability, evidence, progress, and attempt limits.
- Permit a host to use its existing planner without adding a provider to Driver.
- Make both incorrect self-routing and unnecessary handoff measurable.

## Non-goals

No model/provider router inside Driver; no new Driver CLI, SDK, or MCP tool;
no specific GPT/Claude client; no automatic authorization; no generated-code
execution; no batching, observation skipping, historical snapshots, or persistent
semantic-state framework. This proposal does not select work from another PR or
claim that model-assisted routing will improve every task.

## Terminology

- **Route judgment:** a bounded choice about which capability the next subgoal
  needs, separate from selecting an executable action.
- **Handoff:** return of control and a limited reason/state summary to the host;
  it is neither permission nor a Driver action.
- **Planner:** a host-owned reasoning or visual component. It can be the calling
  agent itself; it need not be another hosted model API.
- **Hard gate:** a caller/code check that the model cannot override, such as
  unsupported observations, exhausted budgets, or ambiguous action outcomes.

## Current state

The [landed recipe](../libs/cua-driver/examples/jev-use/README.md) builds complete
bounded actions outside Driver. Its chooser offers `reobserve` and `abstain`;
the runner returns on abstention and verifies completion independently.

Related work has distinct ownership:

- [#3931](https://github.com/trycua/cua/issues/3931) and
  [its RFC](3931-cua-perception-and-jev-use.md) establish the external model-policy
  boundary. This proposal preserves it.
- [#3961](https://github.com/trycua/cua/pull/3961), by `kvnloo`, adds provider-neutral
  backends and factorized decision gates. Reuse its eventual provider/decision
  boundary instead of introducing competing backend adapters.
- [#3963](https://github.com/trycua/cua/issues/3963), also by `kvnloo`, addresses
  measured decision count and observation overhead and identifies existing owners
  for batching, freshness, and timing. This RFC addresses only the planner
  handoff boundary and should not become a second optimization framework.

The Python agent framework's
[composed loop](../libs/python/agent/cua_agent/loops/composed_grounded.py) calls
its configured thinking model and then grounds requested descriptions. That is
fixed role composition, not a lightweight chooser deciding whether to invoke a
planner at all.

## Proposal

### Ownership and flow

```text
host goal and capability policy
  -> fresh Driver observation
  -> caller-built bounded candidates + limited evidence
  -> optional route judgment and action judgment
  -> code-owned gates
       -> execute: existing validation, one action, fresh verification
       -> reobserve: bounded fresh read, then decide again
       -> handoff: return to the host; no action from this decision
  -> host-reviewed subgoal, fresh observation and new candidates before resuming
```

The route and action questions may share one provider request when independent.
Each question must state its hypothetical premise; it cannot read another
question's answer. Ignore the action answer unless the validated route permits
local execution. Extra questions still consume tokens, so a batched request is
not evidence of a net speedup.

Known capability gaps should bypass the model. A caller that knows screenshot
interpretation is necessary must not depend on a text-only chooser to discover
that absence. An invalid route, low confidence in the route, repeated lack of
observable progress, or an exhausted reobservation budget returns control rather
than forcing another action. A changing snapshot is not itself proof of progress;
task-specific postconditions remain authoritative.

### Provider-neutral configuration

Routing is an opt-in policy, not a required dependency. The host chooses the
adapter for a locally served classifier, Jev, or another compatible model and
normalizes its typed result. Reuse the provider boundary from #3961 where it
applies; do not add a parallel registry of endpoints or API keys. A route-only
model can be paired with a different action model, while a capable backend can
answer both questions in one request. Measure the separate-call overhead rather
than assuming a small model is always faster. Backend-specific confidence
semantics and unavailable scores must be explicit and evaluated separately.

Disabling routing restores the existing action chooser without invoking an extra
model. The host's deterministic capability and authorization checks still apply.

### Proposed recipe-local result

The exact serialized shape is an unresolved compatibility decision. A minimal
host-facing handoff needs only:

- finite reason: `reasoning_required`, `visual_evidence_required`,
  `insufficient_observation`, `uncertain_route`, `no_observed_progress`, or
  `budget_exhausted`;
- caller task/subgoal identity and remaining caller-owned budget;
- a bounded, caller-approved observation summary and compact action/outcome history;
- observation/capture provenance sufficient to identify stale context, without
  supplying reusable action authority.

It must not automatically include screenshots, complete application trees,
secrets, Driver objects, executable code, or previously resolved action arguments.
The host chooses what additional evidence may be gathered or sent to its planner.

Prefer returning a typed result first. If maintainers want a callback example,
keep it optional and host-supplied, with cancellation and a deadline. It runs once
for a handoff and never during dry-run. Errors, timeout, and late completion do
not resume actions. A larger model's answer must go through a new observation,
new candidates, existing authorization, and independent verification.

### State and lifecycle

Budgets belong to the whole host task, not only one recipe invocation. Otherwise
repeated handoff and restart could reset the limits indefinitely. The host
accounts for chooser calls, planner calls, reobservations, and actions separately.
Permission refusals and uncertain mutation outcomes are not reasoning failures:
retain the existing recovery/confirmation path and do not silently retry them
with a different model. Cancellation stops the pending route/plan work and may
not reissue a mutation whose result is unknown.

The behavior is platform-independent above Driver. There are no changes to native
input, coordinate spaces, permissions, or capture semantics. Recipe evidence must
still identify the tested Driver version and exact platform limitations.

## Alternatives considered

1. **Outer agent only.** Keep the current design; this remains the default and may
   be best for short or mostly complex workflows.
2. **Confidence-only escalation.** Simple but does not detect confidently wrong
   choices, missing evidence, or an unsuitable action space.
3. **Always use the thinking/grounding composition.** Useful when reasoning is
   required every step, but it does not test whether that call can be avoided.
4. **Generic model routing inside Driver.** Rejected: violates the existing
   provider-neutral execution boundary.
5. **Extend `abstain` only.** May be sufficient if maintainers prefer a small
   reason vocabulary instead of another route head. Evaluate this simpler arm too.

## Compatibility and migration

Start as an opt-in companion recipe or an explicitly approved opt-in example mode
after the relevant parts of #3961. Maintain the default mock/live path and
`cua.jev_choice_request_v1` behavior. Do not add fields to that wire envelope
without deciding versioning, unknown-field behavior, and Python/TypeScript parity.
A first implementation may keep the handoff solely in the host's in-process
control flow. Reverting to the existing recipe is the complete rollback.

## Security, privacy, and telemetry

The host allowlists its planner and the data it may receive. Model routes cannot
expand permissions, select arbitrary endpoints, or override an action refusal.
Application text is untrusted input, including instructions to bypass the local
chooser or send information to a different provider. Raw model confidence does
not authorize anything.

Log route reason, phase duration, call counts, budget consumption, verification
outcome, model/version, and code SHA. Keep private task content, screenshots,
credentials and planner output out of public evidence. The proposal concerns a
new policy boundary, not a report of an exploitable shipped defect.

## Implementation plan

Implementation begins only after the recorded maintainer decision.

1. Agree on ownership and whether a typed result is sufficient or a callback
   example is useful; coordinate with #3961 rather than replacing its authorship.
2. Add a credential-free companion with identical Python/TypeScript fixtures and
   unchanged default behavior. Keep the first handoff boundary in process if
   that avoids an unnecessary public schema revision.
3. Add host-budget, cancellation, dry-run, stale-return, no-progress and
   independent-verification coverage.
4. Run a paired live evaluation. Promote only if the configured routing policy
   meets the agreed outcome and latency bounds; otherwise keep it experimental
   or remove it.

## Test and acceptance plan

The offline matrix must include an obvious button action, a missing/stale
observation, a task requiring unavailable visual evidence, conflicting task
requirements, repeated no-progress with high model confidence, invalid route
output, permission refusal, ambiguous action result, planner failure/deadline,
late or stale planner return, and an exhausted task budget. Assert actual action
counts and verified outcomes, not just output shapes. Dry-run calls neither the
planner nor any mutation. No planner return is directly executed.

Live comparison arms: current recipe, outer planner only, and optional routing.
Interleave simple and complex task fixtures and record wrong self-routes,
unnecessary handoffs, abstentions, completion rates, chooser/planner call counts,
token/cost estimates and total task latency with cold/warm setup separated.
Include routing mistakes rather than discarding them as failed setup. No
universal confidence threshold or latency improvement is assumed.

Python/TypeScript contract parity is required. If implementation later changes
Driver behavior, use the owning RFC and canonical desktop matrix; this RFC-only
PR does not claim desktop execution evidence or require unrelated runtime E2E.

## Unresolved questions

- Companion recipe or opt-in mode after #3961?
- Extend `abstain` reasons, add a route head, or compare both experimentally?
- Typed return only, or also a host-supplied planner callback?
- Keep the first boundary in process, or version a public chooser envelope?
- Which fixture battery and non-regression bounds determine promotion?

## Decision record

Pending maintainer review in [#4013](https://github.com/trycua/cua/issues/4013).
No architecture decision or runtime implementation is represented as accepted.
