---
title: Coordinate-free macOS menu-extra automation
authors:
  - mwildehahn
created: 2026-08-20
last_updated: 2026-08-20
status: review
discussion: https://github.com/trycua/cua/issues/3301
rfc_pr:
implementation:
supersedes:
superseded_by:
---

# RFC: Coordinate-free macOS menu-extra automation

## Summary

Cua Driver should expose an explicit, capability-gated macOS menu-extra scope
for bounded read-only discovery and exact-path invocation of
`AXExtrasMenuBar` items. Existing exact-window application-menu behavior stays
unchanged. The new action has its own authorization boundary, never falls back
to pixels, and does not activate or raise an application window.

## Motivation

`invoke_menu` resolves an application's `AXMenuBar` and requires an exact
`(pid, window_id)` target. That is the correct identity contract for
application commands because the active and key application window determines
menu availability.

macOS menu extras are a different surface. Accessory processes expose them
through `AXExtrasMenuBar`, may own no layer-zero application window, and must
not be driven by activating an unrelated application window. The current
driver therefore cannot semantically address system menu extras. Callers fall
back to screenshots and coordinates, which can select the wrong control after
a display, locale, menu-bar, or popover change.

Cua already has most of the desired action behavior in `invoke_menu`: it
re-resolves the live hierarchy at every hop, matches exact case-sensitive
labels, refuses ambiguity and disabled items, and never silently falls back to
pixels. The missing decision is how to expose the different root, identity,
permission, and focus contract without weakening exact-window tools.

## Goals

- Discover one exact running application's `AXExtrasMenuBar` hierarchy through
  a bounded, read-only tool.
- Invoke a menu extra or descendant through an exact immediate-child path.
- Preserve existing `invoke_menu(pid, window_id, path)` behavior and schema.
- Preserve the prior frontmost application and exact focused window.
- Return typed failures for missing, duplicate, disabled, stale,
  permission-denied, timed-out, and unsupported targets.
- Authorize menu-extra mutation independently from application-menu commands.
- Advertise explicit platform availability.

## Non-goals

- A desktop-wide accessibility tree or generic application-root action API.
- Application-specific selectors, account logic, password entry, or personal
  data handling.
- Pixel, OCR, fuzzy-label, or first-match fallback.
- Replacing or loosening exact-window `invoke_menu`.
- Persisting accessibility objects or menu content across calls.
- Claiming Windows notification-area or Linux status-notifier parity before
  those native contracts have been researched and tested.

## Terminology

- **Application menu:** the `AXMenuBar` whose commands depend on the active
  application and key window.
- **Menu extra:** an item under an application's `AXExtrasMenuBar`, normally
  displayed in the system menu-bar area and often owned by an accessory
  process.
- **Application target:** either an exact positive process identifier or an
  exact bundle identifier that resolves to exactly one running process.
- **Exact path:** one to sixteen trimmed, case-sensitive labels, each matching
  exactly one immediate semantic child.
- **Semantic child:** a direct accessibility child after treating an untitled
  `AXMenu` container as transparent.

## Current state

The contract type `InvokeMenuInput` requires `pid`, `window_id`, and `path`.
The macOS adapter verifies exact WindowServer ownership, activates and focuses
that window, reads `AXMenuBar`, and restores the prior foreground state after
the native action. Windows UIA and Linux AT-SPI implement the same
exact-window public tool with platform-native adapters.

Application enumeration intentionally filters macOS processes to regular
activation policy, while window enumeration intentionally filters to
layer-zero surfaces. Those defaults prevent menu extras, popovers, tooltips,
and other transient system UI from swamping ordinary app/window discovery, but
they also mean an accessory process may have no public discovery path.

The application-composite proposal in #2807 and implementation PR #2894
address same-process `AXWindow` roots without a `CGWindowID`. They explicitly
exclude menu bars and actions. This RFC keeps menu extras separate rather than
reintroducing a broad application-root fallback.

Apple exposes the application extras menu bar through the public
`AXExtrasMenuBar` accessibility attribute. The same Accessibility permission
and stable macOS application identity used by other Cua AX features govern
access to it.

## Proposal

### Public tools

Add two tools to the shared contract:

```text
get_menu_extra_state(
  target: { pid } | { bundle_id },
  max_depth?,
  max_elements?
)

invoke_menu_extra(
  target: { pid } | { bundle_id },
  path: [label, ...]
)
```

The target union has exactly one field. `pid` must be positive. A bundle
identifier must be non-empty and resolve to exactly one running process.
Missing and multiply resolved targets return typed failures. Resolution does
not launch an application.

`get_menu_extra_state` is read-only. It reads only the resolved application's
`AXExtrasMenuBar` and returns a bounded semantic hierarchy containing:

- role;
- title and accessibility description;
- enabled state;
- supported native actions; and
- children within the requested limits.

It returns no screenshot, creates no action-cache entries, and retains no
native accessibility objects after the response. One native-query budget,
one serialized-element budget, and `max_depth` bound the whole call. Native AX
query errors and timeouts remain distinguishable from a genuine empty menu.

`invoke_menu_extra` is mutating. It normalizes and validates a one-to-sixteen
segment path, then performs each hop as follows:

1. Resolve the exact application target again.
2. Create its AX application object and read a fresh `AXExtrasMenuBar`.
3. Resolve the exact path prefix from that live root.
4. Require exactly one match for every segment.
5. Require that the selected element is not disabled.
6. Select a declared native action in an explicit priority order.
7. Dispatch the native accessibility action.

Opening a menu extra can replace its accessibility subtree. Re-reading the
root and re-resolving every prefix avoids retained stale objects or snapshot
indices. An untitled `AXMenu` container is transparent, matching the current
application-menu path model. Missing, duplicate, disabled, timed-out, or
unactionable segments fail closed with a typed refusal. There is no pixel,
keyboard, OCR, fuzzy-match, area, focus, z-order, or first-match fallback.

The action does not activate the target application, make a window key, raise
a window, or intentionally change keyboard focus. It snapshots the prior
frontmost PID and focused window for the desktop-side-effect oracle. If native
menu behavior unexpectedly changes that state, the call reports the observed
delivery/effect rather than silently claiming background preservation.

### Capability and authorization

The tools are separate from `invoke_menu` so existing wire contracts stay
strict and bounded permission manifests can distinguish system menu mutation
from application menu commands.

`get_menu_extra_state` is a read-only capability. `invoke_menu_extra` is an
effectful system-UI capability and receives its own authorization/risk entry.
Authorization runs before target resolution, AX traversal, or action dispatch.
Allowing `invoke_menu` does not imply permission to invoke menu extras.

macOS advertises the implemented tools after the normal driver capability
checks. Windows and Linux register the shared tool schemas but return a typed
`unsupported_platform` result until equivalent notification-area or
status-notifier adapters have an accepted contract and native evidence. They
must not substitute an application menu or coordinate action.

### Owner transitions

Some menu-extra actions open a popover whose accessibility descendants may be
owned by another process. Version one follows only elements reachable from the
resolved application's live `AXExtrasMenuBar` root and verifies each resolved
element's owner where the platform supplies it. An ownership transition not
explicitly represented in the accepted contract is refused.

If live research shows that useful public menu extras universally require a
cross-process transition, the RFC must define a typed owner-chain binding
before implementation. It must not infer the next owner from window position,
focus, z-order, title, or process order.

### Results and verification

Discovery returns typed availability and degradation state rather than
collapsing native API failure into an empty hierarchy. Invocation returns an
action record only after the native API accepts every requested hop. The final
semantic effect remains `unverifiable`; callers must observe a fresh external
postcondition rather than treating AX API success as proof that the requested
system change completed.

## Alternatives considered

### Generalize `invoke_menu` in place

Adding `menu_bar: application | extras` and making `window_id` optional would
reduce tool count. It would also weaken an established cross-platform input
schema and make permission manifests less able to distinguish application
commands from system-menu mutation. Separate tools preserve compatibility and
authorization clarity.

### Expose accessory processes through `list_apps`

Broader process discovery could help callers find a PID but does not provide
bounded semantic menu state or a safe action identity. It also changes an
independent cross-platform enumeration contract. Exact bundle targeting solves
the initial discovery problem without flooding normal app lists.

### Reuse application-composite scope

Walking an application root would mix exact windows, unattributed windows,
application menus, extras, and unrelated controls. That ambiguity is precisely
what current exact-window APIs avoid. A dedicated root and capability are
narrower.

### Keep screenshot and coordinate automation

Coordinate scripts are useful diagnostic probes but cannot establish semantic
identity and can hit the wrong target after layout, locale, display, or
popover changes. They are not a production secret- or session-control path.

### Define a common status-item abstraction immediately

Windows notification-area and Linux status-notifier semantics may eventually
fit a shared model. Generalizing before native research would either encode
macOS assumptions as universal or make the first capability too broad. Typed
platform availability keeps this increment honest and reversible.

## Compatibility and migration

The proposal is additive. Existing tool names, schemas, action tokens,
authorization, and `invoke_menu` behavior do not change. Older drivers do not
advertise the new tools, and clients retain their existing behavior.

Rollback removes capability advertisement and the two tool registrations. No
state, native element, credential, or migration artifact is persisted. A later
accepted RFC may add native Windows/Linux implementations or supersede the
macOS-specific names with a proven common abstraction.

## Security, privacy, and telemetry

Both tools require the existing macOS Accessibility grant held by the stable
Cua Driver app identity. `invoke_menu_extra` is separately authorized because
menu extras can trigger system-level effects such as session, network, audio,
or accessibility changes.

Discovery is restricted to one exact running application and bounded across
native queries, depth, and serialized elements. The driver retains no native
object after a call and writes no menu-extra state to the action cache.
Permission denial, ambiguous target, duplicate path, changed hierarchy,
foreign owner, unavailable action, messaging timeout, and locked/no-GUI state
all fail closed.

No application content, labels, descriptions, account identifiers, bundle
identifiers, element paths, screenshots, native objects, or action arguments
may enter telemetry or Computer History. Allowed telemetry is limited to
capability availability, platform, counts, latency, and typed result or refusal
codes.

The tools do not accept or type credentials. They must not deliver keyboard
input, alter the clipboard, or expose an anonymous desktop-wide element index.

## Implementation plan

1. Resolve the open target and cross-process ownership questions with a live,
   sanitized AX probe and record only structural results.
2. Add contract types, schemas, capability discovery, separate authorization,
   typed unsupported adapters, and protocol tests.
3. Add the bounded macOS `AXExtrasMenuBar` walker and native unit tests.
4. Add exact-path macOS invocation without activation or pixel fallback.
5. Extend a repository-owned macOS fixture with a deterministic menu extra and
   independent state oracle.
6. Add canonical Lume rows for discovery, invocation, focus/z-order/cursor
   preservation, zero keyboard leakage, and negative outcomes.
7. Generate public reference documentation and update the driver skill only
   after the behavior is proven.

Each increment retains an explicit rollback at capability advertisement. No
implementation pull request becomes ready before this RFC is accepted.

## Test and acceptance plan

- Contract/schema tests prove both tools are additive, strictly typed,
  bounded, separately authorized, and advertised or unavailable by platform.
- macOS unit tests prove exact matching, transparent untitled menus,
  duplicate/missing/disabled refusal, action ordering, native query bounds,
  and reference cleanup.
- A repository-owned fixture exposes an `AXExtrasMenuBar` item whose action
  changes independently observable fixture state.
- Canonical macOS Lume E2E proves bounded discovery and exact-path invocation
  at the candidate SHA while preserving prior frontmost PID/window, z-order,
  cursor position, and the keyboard sentinel.
- Negative E2E rows cover dead and ambiguous targets, duplicate labels,
  permission denial, locked/no-GUI state, native timeout, hierarchy replacement
  between hops, and an unapproved cross-process owner transition.
- Windows and Linux protocol tests produce the declared typed unsupported
  result with zero substituted window-menu or pixel input.
- A supporting built-in macOS smoke may validate real-world compatibility, but
  private labels, screenshots, and user-specific content are not acceptance
  evidence.

## Unresolved questions

- Should the accepted names remain macOS-specific (`menu_extra`) or adopt a
  future-facing `status_item` term before Windows/Linux adapters exist?
- Should the first target accept exact bundle identifier plus PID, or require
  PID and add accessory-process discovery separately?
- Do useful menu-extra popovers cross process ownership, and if so, what public
  owner-chain identity can bind that transition without heuristics?
- Should discovery return semantic state only, or opaque short-lived tokens
  that invocation must consume?
- What native query and wall-clock limits prevent an unresponsive accessory
  process from exhausting a call while preserving useful discovery?
- Which native actions are valid for intermediate and final segments of a menu
  extra path?

## Decision record

Pending maintainer review in #3301.
