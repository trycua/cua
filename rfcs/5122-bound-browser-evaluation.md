---
title: Bound JavaScript evaluation for driver-owned browsers
authors:
  - Iflaqbhat
created: 2026-10-14
last_updated: 2026-10-14
status: review
discussion: https://github.com/trycua/cua/issues/5122
rfc_pr:
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
the initial contract. Use the existing per-tab mutation gate to serialize it
with navigation and other browser mutations. Recheck generation, ownership,
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
limit, alongside existing CDP response limits. Do not return remote object IDs
or leave retained object handles for callers to manage.

An exception or timeout is not success. Return a structured refusal using the
browser refusal vocabulary, with documented details. A timeout bounds how
long the driver waits; it does not guarantee the page script stopped or undo
its effects. Do not automatically retry or terminate the renderer. If the
document changes, invalidate its old snapshot refs through the same mechanism
used by other browser mutations.

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
3. Mutation-check the ownership and origin guards: removing either must fail
   its owner regression. Verify refused calls issue no evaluation request.
4. Add a canonical browser fixture with two tabs, distinguishable state, and
   navigation. Observe the selected tab independently of the tool receipt and
   verify no fallback, foreground activation, or mutation of the other tab.
5. Validate the accepted contract on supported macOS, Windows, and Linux
   browser lanes, regenerate public surfaces, and record the candidate SHA.

## Decisions requested

- Is arbitrary JavaScript required, or should the first tool expose fixed QA
  queries instead?
- Approve or adjust the proposed scope: owned isolated browsers and the
  top-level tab only, with existing-profile and embedded targets refused.
- Approve the timeout, result size, result encoding, and refusal semantics.

## Decision record

Pending maintainer review in #5122. No implementation has been approved.
