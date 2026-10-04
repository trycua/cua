---
title: Optional structured abstain handoff above Cua Driver
authors:
  - JkRheezy
created: 2026-09-21
last_updated: 2026-09-30
status: review
discussion: https://github.com/trycua/cua/issues/4013
rfc_pr: https://github.com/trycua/cua/pull/4014
implementation: []
supersedes:
superseded_by:
---

# RFC: Optional structured abstain handoff above Cua Driver

## Summary

Add an optional structured handoff payload when the external `jev-use` recipe
returns `abstain`, so the caller's reasoning/visual planner can resume from the
chooser's reason, bounded history, and current observation provenance. Jev remains
optional; the existing chooser and `reobserve` behavior are unchanged.
The host continues to own model selection, credentials,
permission policy, and the planner loop. Driver continues to own observation,
action validity, execution, and deterministic verification. This document asks
for a design decision; it contains no runtime implementation or performance claim.

## Motivation

A confident choice among available actions does not establish that those actions
can solve the next subgoal. Interpreting a document, resolving contradictory
requirements, or judging a canvas may require evidence or capabilities absent
from a text-based chooser.

The missing piece is structured context on the existing `abstain` return:
why a bounded chooser stopped, what limited context the host may use, and what
must happen before a planner's answer can influence a new action. That context
should not be inferred from confidence alone or confused with permission to execute.

## Goals

- Add an optional structured payload to the recipe's existing `abstain` return.
- Preserve the existing recipe behavior when the payload is disabled.
- Return the chooser's reason, bounded history, current `capture_id` and observation reference.
- Permit a host to use its existing planner without adding a provider to Driver.

## Non-goals

No model/provider router inside Driver; no new Driver CLI, SDK, or MCP tool;
no specific GPT/Claude client; no automatic authorization; no generated-code
execution; no batching, observation skipping, historical snapshots, or persistent
semantic-state framework. General model routing, classifier/provider configuration,
and planner callbacks are out of scope. Hosts retain model selection.

## Terminology

- **Handoff:** return of control and a limited reason/state summary to the host;
  it is neither permission nor a Driver action.
- **Planner:** a host-owned reasoning or visual component. It can be the calling
  agent itself; it need not be another hosted model API.

## Current state

The [landed recipe](../libs/cua-driver/examples/jev-use/README.md) builds complete
bounded actions outside Driver. Its chooser offers `reobserve` and `abstain`;
the runner returns on abstention and verifies completion independently.
`cua.decision_choice_v1` already carries `abstain` and `error` outcomes with reasons.
[RFC #4268](https://github.com/trycua/cua/issues/4268) added per-candidate `source`
and task `progress`. The missing piece is the recipe's structured return context,
not another chooser or route head.

Related work has distinct ownership:

- [#3931](https://github.com/trycua/cua/issues/3931) and
  [its RFC](3931-cua-perception-and-jev-use.md) establish the external model-policy
  boundary. This proposal preserves it.
- [#3963](https://github.com/trycua/cua/issues/3963), by `kvnloo`, has a
  [recorded maintainer decision](https://github.com/trycua/cua/issues/3963#issuecomment-5896357755):
  mechanisms remain recipe-local and opt-in; promotion requires a forced path,
  path attribution, an independent oracle, and visible failure/fallback.
  This RFC addresses only the abstain handoff payload.

## Proposal

### Ownership and flow

```text
host goal and capability policy
  -> fresh Driver observation
  -> caller-built bounded candidates + limited evidence
  -> existing chooser and code-owned gates
       -> execute: existing validation, one action, fresh verification
       -> reobserve: bounded fresh read, then decide again
       -> abstain: return to the host, optionally with handoff payload; no action
  -> host-reviewed subgoal, fresh observation and new candidates before resuming
```

### Proposed recipe-local result

Add an optional `handoff` object to the recipe's host-facing `abstain` result,
assembled by the runner from the validated chooser result and its own context:

- `reason`: the chooser's existing reason for declining, or `null` if absent;
  do not invent a capability judgment.
- `history`: `{entries, truncated}`, with recent `{step, selected_id, outcome}`
  entries from this run, capped by caller-owned entry and byte limits before
  return. Outcomes retain refusals and uncertainty; truncation is explicit.
- `capture_id`: the current observation's capture ID, or `null` when that
  observation has no capture. Never substitute an earlier capture.
- `observation_ref`: a caller-approved reference to the current observation
  associated with that chooser decision, or `null` when unavailable. It is
  context, not reusable action authority.

This is additive recipe-local return data, not a field in the chooser's wire
request or response. It does not change `reobserve`, convert `error` to
`abstain`, or introduce a new reason vocabulary.

It must not automatically include screenshots, complete application trees,
secrets, Driver objects, executable code, or previously resolved action arguments.
The host chooses what additional evidence may be gathered or sent to its planner.

Returning the payload invokes no planner and executes no action. A larger model's
answer must go through a new observation, new candidates, existing authorization,
and independent verification.

### State and lifecycle

Budgets belong to the whole host task, not only one recipe invocation. Otherwise
repeated handoff and restart could reset the limits indefinitely. The host
retains its existing task-budget accounting.
Permission refusals and uncertain mutation outcomes are not reasoning failures:
retain the existing recovery/confirmation path and do not silently retry them
with a different model. Cancellation stops pending recipe work; the host owns
planner cancellation.
Neither path may reissue a mutation whose result is unknown.

The behavior is platform-independent above Driver. There are no changes to native
input, coordinate spaces, permissions, or capture semantics. Recipe evidence must
still identify the tested Driver version and exact platform limitations.

## Alternatives considered

1. **Outer agent only.** Keep the current design; this remains the default.
2. **General routing layer or a new route head.** Rejected for this RFC: hosts
   already own model selection. Extend the existing `abstain` return only.

## Compatibility and migration

Start as an explicitly opt-in addition to the existing `jev-use` recipe.
Maintain the default mock/live path, `cua.jev_choice_request_v1`,
`cua.jev_choice_request_v2`, and existing
chooser responses, including `cua.decision_choice_v1`. The handoff stays in the
recipe's host-facing return; no Driver or chooser wire schema revision is needed.
Disabling the payload is the complete rollback.

## Security, privacy, and telemetry

The host allowlists its planner and the data it may receive. A handoff cannot
expand permissions or override an action refusal.
Application text is untrusted input, including instructions to bypass the local
chooser or send information to a different provider. Raw model confidence does
not authorize anything.

Log abstain reason, whether the payload path ran, history truncation, fallback,
verification outcome, model/version, and code SHA. Keep private task content, screenshots,
credentials, observation references and planner output out of public evidence.

## Implementation plan

Implementation begins only after the recorded maintainer acceptance.

1. Add the optional runner-assembled `abstain` payload with unchanged default
   behavior and matching Python/TypeScript fixtures.
2. Cover bounded history, current observation provenance, task budgets,
   cancellation, dry-run and independent verification.
3. Qualify the payload path under the promotion rule below; no routing experiment
   or planner integration is part of this implementation.

## Test and acceptance plan

Follow the promotion rule adopted in
[#3963](https://github.com/trycua/cua/issues/3963#issuecomment-5896357755):

- **Forced path:** force the chooser to return `abstain` with a known reason
  after recorded attempts; enable the payload and exercise history truncation
  and present/missing current capture and observation references. Also exercise
  disabled mode, `reobserve`, and `error` without changing their behavior.
- **Path attribution:** record and assert that the optional abstain payload path
  actually ran, with its chooser outcome, reason and truncation state. A green
  test that never reaches that branch is not qualifying evidence.
- **Independent oracle:** compare payload fields with fixture-owned observation
  identities and an independently recorded action journal. Assert no additional
  mutation or planner call on handoff; a chooser or runner success flag alone
  does not prove those assertions.
- **Visible fallback:** retain abstention, errors, permission refusal, uncertain
  action outcomes, exhausted budgets and unavailable context as visible outcomes.
  Disabled payload mode keeps the existing abstain return. Missing references,
  cancellation and dry-run must not cause an action or a silent retry. Retain
  the existing recovery path; returning the payload adds no planner invocation.

Python/TypeScript contract parity is required. This RFC-only PR does not claim
runtime execution evidence or a speedup, and does not require unrelated desktop
E2E. Any later speed claim must independently meet the same promotion rule.

## Unresolved questions

- Which existing recipe return type should carry the optional `handoff` object?
- Which default entry and byte limits should bound the returned history?

## Decision record

[Maintainer review on #4014](https://github.com/trycua/cua/pull/4014#issuecomment-5896358721)
requested this narrower scope: an optional structured payload on `abstain`,
with no general routing layer or classifier configuration. Acceptance of this
revised RFC remains pending; no runtime implementation is represented as accepted.
