---
title: Bound JavaScript evaluation for driver-owned browsers
authors:
  - Iflaqbhat
created: 2026-10-14
last_updated: 2026-10-14
status: review
discussion: https://github.com/trycua/cua/issues/5122
rfc_pr: https://github.com/trycua/cua/pull/5150
implementation:
supersedes:
superseded_by:
---

# RFC: Bound JavaScript evaluation for driver-owned browsers

## Summary

Add `browser_evaluate` for a tab bound to a driver-owned isolated browser.
Use the existing browser binding, session ownership, origin authorization,
and audit path. Treat arbitrary JavaScript as a mutation even when the
expression appears to read data. Initially refuse existing-profile and
embedded-browser targets; broader access requires a separate decision.

This is a proposal for #5122. It is not an accepted contract or a shipped tool.

## Motivation

QA agents need page-specific assertions beyond the semantic snapshot. Today
they either enable an intentionally gated legacy tool or connect directly to
DevTools. A supported route should preserve the driver's target and permission
checks without requiring an agent to choose a raw debugging port.

### QA checks and evidence needed

The body of #5122 does not list specific failing assertions, and its discussion
has no workload examples yet. The following are candidate checks for review,
not requirements attributed to the reporter:

| Candidate assertion                                                                       | Current semantic snapshot                                                                                                         | Narrower alternative                                                                                  |
| ----------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| A control has the expected accessible name, value, or checked/disabled/expanded state.    | Already represented by semantic nodes and their states; missing or omitted nodes need snapshot scope/continuation checks first.   | Use the existing snapshot; this does not justify arbitrary evaluation.                                |
| An element's computed `color` or `font-size` equals an expected CSS value.                | The collector uses a finite computed-style list for visibility/action decisions; it does not expose these arbitrary style values. | A bound element query accepting an allowlist of CSS properties.                                       |
| A test-owned `localStorage` key contains the value the application was expected to save.  | Storage values are not part of the semantic snapshot.                                                                             | An explicitly authorized query for named storage keys; this still exposes potentially sensitive data. |
| A test application's own JavaScript predicate or promise reports that its model is ready. | Application-specific JavaScript state is not a semantic node field.                                                               | A predefined test hook if the required assertions form a stable finite set.                           |

The coverage comparison comes from
[the semantic collector](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/semantic.rs),
including `SemanticNode`, `SEMANTIC_COMPUTED_STYLES`, and `ax_states`.
These source observations establish possible coverage gaps, not that the QA
run in #5122 needs each one. No real application assertions have been supplied
or reproduced for this proposal.

Before choosing an API, ask the reporter for the actual assertions, the
expected result for each, and why a scoped semantic snapshot cannot answer it.
If all confirmed cases fit fixed queries, prefer that smaller API and revise
the contract before implementation. The design decision remains open; this
RFC does not establish that arbitrary JavaScript is necessary.

## Current state

- [The legacy page tool](../libs/cua-driver/rust/crates/cua-driver-core/src/page.rs)
  gates `execute_javascript` with `CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS`.
- [The registry](../libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs)
  additionally requires trusted unrestricted launch authorization for legacy
  mutations. Enabling the environment variable alone is insufficient.
- [BrowserEngine](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/engine.rs)
  resolves session-bound tabs, checks connection generations and native window
  identity, and rechecks the live origin before browser mutations.
- [Isolated browser preparation](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/prepare.rs)
  records the owned PID and preparing sessions. This ownership proof is distinct
  from an approved connection to a person's existing profile.

## Goals

- Evaluate against one explicitly selected tab in an owned isolated browser.
- Apply runtime permission mode, tool policy, ownership, and origin checks.
- Return bounded structured data without exposing a raw DevTools connection.
- Provide observable errors for exceptions, lost bindings, and timeout.

## Non-goals

- A JavaScript sandbox inside an otherwise authorized page.
- A claim that arbitrary expressions are read-only.
- Existing-profile or embedded-host evaluation in the first implementation.
- Browser lifetime renewal, which is discussed separately in #5123.
- New network access restrictions beyond the runtime's existing browser policy.

## Proposed contract

The proposed input requires `target_id`, `tab_id`, and `expression`, with the
existing optional public `session` selector. It accepts a bounded `timeout_ms`;
proposed limits are a 5-second default and a 10-second maximum. The server owns
these limits; a caller cannot request an unlimited wait.

Use the same explicit tab binding as browser mutations. Do not accept a PID,
port, WebSocket URL, or URL substring as an alternative target. The current
session must own the isolated browser, and the target must remain an exact,
live binding to that process and browser window.

Evaluation runs in the selected tab's top-level page context. Frame selection,
OOPIF evaluation, workers, browser chrome, and extension contexts are outside
the initial contract. Use the existing per-tab mutation gate to serialize
driver requests while an evaluation is being awaited, with the timeout behavior
specified below. Recheck generation, ownership,
and the live allowed origin immediately before dispatch.

Do not add an implicit grant. Standard mode uses the existing authorization
path for a mutation of a driver-owned resource; bounded mode additionally
requires the new tool in the launch-approved manifest and the live origin in
scope. Unrestricted mode retains its existing trusted launch requirements.
An existing-profile attachment grant does not authorize this tool.

The MCP annotations classify evaluation as potentially destructive and neither
read-only nor idempotent. Policy and action classification must agree with
those annotations across the registry, SDK, and MCP adapters.

Await returned promises and return only JSON-compatible values by value.
Represent JavaScript `undefined` separately from JSON `null`; refuse values
that cannot be represented under the contract. Propose a 1 MiB encoded result
limit. Do not return remote object IDs
or leave retained object handles for callers to manage.

### Result limits

Measure the 1 MiB limit as the UTF-8 byte length of the serialized JSON value,
before wrapping it in the tool result. Exactly 1 MiB is allowed; any larger
value receives a structured refusal with reason `result_too_large`, the applied
limit, and the observed size when known. Return no partial value, truncated
string, or success receipt. Do not include the rejected data in the refusal.

The result limit and the CDP/WebSocket response limits are independent guards.
Both apply; neither overrides or raises the other. Transport limits count the
whole message or frame, including the CDP envelope, whereas the proposed limit
counts the decoded result value. A stricter transport limit can therefore
refuse delivery even for a value within 1 MiB. If transport delivery fails first,
return the existing structured browser-route error without claiming a decoded
result size. If transport succeeds but the value exceeds 1 MiB, return
`result_too_large`. In neither case truncate or retry with relaxed limits.

[The current CDP connection](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/cdp_ws.rs)
uses `connect_async` without an explicit WebSocket configuration; it inherits
the dependency's transport bounds. The new tool must not silently enlarge
those bounds to satisfy its own result limit. Exact public refusal-code naming
remains part of the contract decision; `result_too_large` is the proposed reason.

### Timeout and subsequent actions

An exception or timeout is not success. Return a structured refusal using the
browser refusal vocabulary, with documented details. A timeout bounds how
long the driver waits; it does not guarantee the page script stopped or undo
its effects. For a dispatched evaluation, the proposed timeout refusal records
reason `evaluation_timeout` and `execution_may_continue: true`.

Release the per-tab gate when the evaluation returns its timeout refusal, or
the caller is cancelled, rather than retaining it until an arbitrary promise
settles. A following navigation or `browser_evaluate` on that tab can acquire
the gate and proceed after its own fresh binding, generation, ownership, and
origin checks. It is not a continuation or retry of the timed-out call, and
must not assume that the previous script has completed. Earlier side effects
and asynchronous work can overlap the next request; even a successful
evaluation can schedule work beyond its returned promise. Serialization of
driver requests is not isolation of all page execution.

Do not automatically retry the expression, terminate the renderer, or treat
navigation or reconnect as proof that every prior effect was cancelled. A late
reply must not become a second receipt or be attributed to a later request.
If the document changes, invalidate its old snapshot refs through the existing
browser mutation mechanism.

This choice keeps an unresolved promise from blocking driver requests forever,
but its uncertain-effect behavior needs maintainer approval. An alternative
that blocks later mutations until execution is proven settled would require
separate tracked-operation state and a recovery contract; retaining the mutex
indefinitely alone would not provide bounded operation.

The current `CdpConnection::call` has a 20-second response wait, removes pending
request state on its own timeout, and discards later replies for that ID.
Implementing the shorter tool timeout also requires cancellation-safe removal
of that pending state; dropping an outer timeout future must not retain a
sender forever for an unresolved page promise. No cancellation or script
termination capability is claimed by this proposal.

## Alternatives considered

**Enable legacy evaluation:** retains the unbound route the typed tools are
intended to replace and requires broad trusted startup authorization.

**Label evaluation read-only:** property getters and function calls can mutate
state or issue requests. Source inspection does not establish a dependable
permission boundary.

**A fixed assertion/query API:** offers a narrower contract and may be preferable
if maintainers only need a finite set of QA observations. Confirm whether
arbitrary expressions are required before approving this proposal.

## Compatibility and platform behavior

This is an additive tool, implemented in the shared browser core. Existing
tools, legacy gating, and existing-profile grants keep their current meaning.
Publish it only where the registered browser engine supports the contract;
unsupported engines retain an explicit browser-route refusal.

The implementation must update the shared contract and generated SDK/MCP
surfaces together, including policy classification and tool inventory. Revert
the new tool and its generated entries together if rollout is withdrawn.

## Security, privacy, and telemetry

Page evaluation can read sensitive data and trigger external effects. Session
ownership and origin scope remain necessary, but they do not make JavaScript
safe or constrain every request a page can send. Avoid claiming otherwise.

Audit the tool, target, permission decision, timing, and outcome under existing
policy. Do not add default logging of expressions, returned page data, cookies,
or exception contents. Keep raw endpoints and runtime-private session IDs
internal.

## Implementation and acceptance plan

Implementation starts after the discussion records the accepted contract.

1. Add the typed contract, policy classification, and shared engine route.
2. Add hermetic contract and mock-CDP tests for exact tab routing, ownership,
   stale generations, origin refusal, exceptions, promise completion, timeout,
   result limits, `undefined` versus `null`, and unsupported target types.
   Cover values at and above 1 MiB, a tighter transport bound, no truncation,
   pending-request cleanup on cancellation, and late replies after timeout.
   Verify the gate releases on timeout, a later same-tab request reruns its
   authorization checks, and another tab is not blocked by the pending script.
3. Mutation-check the ownership and origin guards: removing either must fail
   its owner regression. Verify refused calls issue no evaluation request.
4. Add a canonical browser fixture with two tabs, distinguishable state, and
   navigation. Observe the selected tab independently of the tool receipt and
   verify no fallback, foreground activation, or mutation of the other tab.
5. Validate the accepted contract on supported macOS, Windows, and Linux
   browser lanes, regenerate public surfaces, and record the candidate SHA.

## Decisions requested

- Which concrete assertions from the QA run in #5122 cannot be answered by
  semantic snapshots? Confirm or replace the candidate examples above, then
  choose fixed queries or arbitrary JavaScript.
- Approve or adjust the proposed scope: owned isolated browsers and the
  top-level tab only, with existing-profile and embedded targets refused.
- Approve the timeout, result size, result encoding, and refusal semantics,
  including release of the per-tab gate with potentially continuing effects.

## Decision record

Pending maintainer review in #5122. No implementation has been approved.
