---
title: Safe application-composite AX scope for unattributed macOS surfaces
authors:
  - Kickflip73
created: 2026-08-03
last_updated: 2026-08-03
status: review
discussion: https://github.com/trycua/cua/issues/2807
rfc_pr: https://github.com/trycua/cua/pull/2808
implementation: []
supersedes:
superseded_by:
---

# RFC: Safe application-composite AX scope for unattributed macOS surfaces

## Summary

Add an explicit, capability-gated application-composite accessibility scope for
semantic elements that belong to a target process but cannot be safely
attributed to one native window identifier. Preserve the existing exact
`(pid, window_id)` contract and its fail-closed behavior. Application-composite
observation must never be selected implicitly, and any future action path must
use short-lived scope-bound references that are revalidated before dispatch.

## Motivation

The exact-window contract introduced by
[#2237](https://github.com/trycua/cua/issues/2237) and
[#2645](https://github.com/trycua/cua/pull/2645) prevents an agent from silently
acting on the wrong surface. That remains the correct default.

Some macOS applications, however, expose semantic controls through the
application AX hierarchy or compositor-owned top-level elements without a
stable `CGWindowID`. In a sanitized live reproduction with cua-driver 0.16.0,
one process exposed three overlapping WindowServer surfaces. One exact window
resolved to an AXWindow but omitted the writable editor. Two same-process
compositor surfaces returned `ax_window_unresolved`. A separate read-only walk
from the application AX root could observe the editor.

Per-window callers therefore cannot acquire a semantic target. Pixel fallback
can still observe and interact visually, but cannot establish semantic element
identity or a semantic postcondition. This is a platform contract gap rather
than an application-specific selector problem.

## Goals

- Keep exact-window isolation and `ax_window_unresolved` behavior unchanged.
- Explicitly observe same-process AX elements that have no safe native-window
  attribution.
- Return typed scope and surface identity rather than implying an exact window.
- Bound application-composite observations by process, query, depth, element
  count, and runtime policy.
- If actions are later admitted, make stale, foreign, or ambiguous references
  fail closed before input dispatch.
- Define common protocol semantics and explicit platform availability.

## Non-goals

- Restore automatic largest-window or focused-window selection.
- Make `window_id` optional for existing state or action tools.
- Create a desktop-wide or cross-process semantic element index.
- Synthesize a screenshot from unrelated WindowServer surfaces.
- Add application-specific scripts, selectors, coordinates, or exceptions.
- Weaken Accessibility permission, focus, background-delivery, or input-leakage
  guarantees.

## Terminology

- **Exact-window scope:** AX state rooted at a platform accessibility window
  that is safely attributed to the requested native window identifier.
- **Application-composite scope:** AX state rooted at a requested process where
  one or more semantic subtrees cannot be represented as exact-window scope.
- **Scope identity:** A typed observation identity issued by the driver. It is
  not a raw PID, AX pointer, element index, or caller-selected window guess.
- **Element reference:** An opaque, runtime-issued reference to an observed
  element. It is not necessarily an action capability.

## Current state

`get_window_state` requires `pid` and `window_id`. On macOS, an unresolved AX
window returns an empty AX tree with `degraded_reason = ax_window_unresolved`;
pixel capture can remain available for that exact WindowServer surface.

The macOS tree walker already distinguishes window-rooted and application-rooted
walks internally. The public contract exposes only the former as rich state.
Application topology from `get_accessibility_tree` is intentionally lightweight
and does not return the bounded semantic application tree needed here.

Related proposals cover different boundaries:

- [#2238](https://github.com/trycua/cua/issues/2238) addresses rediscovery and
  rebinding after surface changes.
- [#2200](https://github.com/trycua/cua/issues/2200) rejects ambiguous PID-only
  action targets.
- [#1968](https://github.com/trycua/cua/issues/1968) keeps desktop scope
  vision-only and excludes desktop-wide element indices.

## Proposal

### Public contract

Add an explicit application-composite observation scope to the common driver
contract and advertise its availability through capability discovery. The
final schema may be a dedicated tool or a typed `scope` variant on a broader
state API; review should choose that shape before implementation.

The caller must explicitly request the target PID and application-composite
scope. The driver must not infer the scope from window area, focus, z-order, or
failure of an exact-window request. Existing exact-window requests do not
automatically fall back to application scope.

The response distinguishes at least:

1. exact-window-backed AX subtrees with a native window identifier;
2. application-composite AX subtrees without safe native-window attribution;
3. degraded or unavailable subtrees with a typed reason.

The observation is AX-only unless a later decision defines independent image
semantics. Exact-window and desktop screenshot contracts remain unchanged.

### Platform ownership

The common protocol crate owns scope enums, capability discovery, bounds,
degraded reasons, and serialized response shapes. Each platform adapter owns
native discovery and identity mapping.

On macOS, the adapter walks the requested application's AX root using the
existing bounded walker and partitions top-level descendants by attribution.
It must not manufacture a `CGWindowID` when one is unavailable. Windows UIA and
Linux AT-SPI either implement equivalent process-scoped semantics or return an
explicit capability-unavailable result; they must not silently reinterpret the
scope.

### Identity and lifecycle

Each observation receives a runtime-local scope identity. Element references,
if returned, are opaque and bound to:

- driver runtime instance;
- target PID and process identity;
- scope kind and scope identity;
- observation snapshot or generation;
- an expiry policy.

References are observation identities in the first increment, not ambient
authority to act. They must not expose raw AX pointers or be valid after daemon
restart.

### Optional action increment

Application-composite actions are a separate reviewable increment. If accepted,
an action consumes an opaque element reference. Immediately before dispatch the
platform adapter re-resolves the reference and verifies the process, scope,
generation, role/action compatibility, and uniqueness. Missing, stale,
foreign-process, multiply-resolved, or permission-denied references fail with a
typed outcome and dispatch no input.

Raw PID plus element index is never an action target. Query text, role, focus,
z-order, or area are not sufficient to repair a stale reference.

## Alternatives considered

### Pixel fallback only

This remains a safe degraded observation path, but it cannot bind semantic
identity or prove a semantic postcondition.

### Restore automatic application-state selection

Selecting the largest, focused, or first window can choose hidden utility
panels or overlapping surfaces and recreate wrong-surface actions. It conflicts
with the safety motivation for exact-window scope.

### Optional `window_id` or PID-only actions

This makes ambiguity part of the public contract and conflicts with #2200.

### Merge all exact-window responses in the caller

Unresolved compositor surfaces intentionally contain no AX tree, so the
application-level elements never appear in the merge.

### Desktop semantic scope

This crosses process and privacy boundaries and is deliberately out of scope in
#1968.

### Application-specific automation

AppleScript, JXA, private selectors, or fixed coordinates are not a general
driver contract and do not preserve semantic identity across UI changes.

## Compatibility and migration

The change is additive. Existing `get_window_state` inputs and outputs,
exact-window tokens, action schemas, and degraded responses do not change.
Older drivers do not advertise the capability, so clients retain their current
pixel/degraded fallback.

Release sequencing:

1. common types, capability discovery, and negative protocol tests;
2. read-only macOS observation plus canonical fixture evidence;
3. platform parity or explicit typed unavailability;
4. optional token-bound actions after a separate safety gate.

Rollback stops advertising the capability. No persisted user data or migration
is required.

## Security, privacy, and telemetry

Application-composite scope can reveal content from multiple windows of one
process and is more privacy-sensitive than exact-window scope. It requires the
same OS Accessibility permission, an explicit PID, existing permission-mode
policy checks, and strict process containment. Existing depth, element-count,
query, and response-size limits apply.

No implementation may auto-select a scope by area, focus, z-order, or first
match. Any action reference is short-lived, runtime-bound, process-bound,
scope-bound, and revalidated. Failure dispatches no input.

Telemetry may include capability availability, scope kind, element counts,
latency, and degraded reason. It must never contain AX text or values,
screenshots, application content, account identifiers, or element references.

## Implementation plan

1. Add common scope/capability/degraded types and schema tests without enabling
   application-composite observation.
2. Add a source-built macOS fixture that separates WindowServer and AX
   attribution, plus independent state oracles.
3. Implement bounded read-only macOS observation behind capability discovery.
4. Add Windows and Linux typed-unavailable or parity implementations.
5. Review evidence and decide whether to admit a separately gated action path.

Each increment is independently reversible and keeps exact-window behavior as
the parity gate.

## Test and acceptance plan

- A repository-owned macOS fixture exposes one normal exact window and one
  same-process semantic editor without stable `CGWindowID` attribution.
- Existing exact-window state remains isolated; unresolved window state still
  fails closed.
- Application-composite observation sees only same-process elements and returns
  typed scope identities under all configured bounds.
- Stale identity, foreign PID, ambiguity, vanished element, permission denial,
  and no-GUI session return declared failures with zero leaked input.
- Windows and Linux tests prove equivalent capability semantics or explicit
  typed unavailability.
- If actions are implemented, an independent fixture oracle proves that only
  the intended control changed, while foreground window, z-order, cursor, and
  input-leakage evidence remain clean.
- Protocol/schema tests, focused Rust tests, and the canonical macOS harness pass.
- The sanitized installed-application reproduction is supporting evidence only;
  no private content or screenshot is required for acceptance.

## Unresolved questions

- Dedicated `get_application_state` tool or a typed scope parameter?
- Read-only first release or observation plus token-bound actions?
- Which native facts form a stable macOS scope identity without `CGWindowID`?
- Group composite children by top-level AX element, WindowServer metadata, or
  only opaque driver identity?
- Which semantics require parity and which may be capability-gated by platform?
- Must application-composite scope remain AX-only permanently?

## Decision record

Pending maintainer review in
[#2807](https://github.com/trycua/cua/issues/2807).
