# Exact macOS Background Input v1

**Status:** Proposed implementation plan

**Base:** `origin/main` at `d21e3447f9b08c761c090946648d5aca5e6c9cf1`

**Date:** 2026-08-04

## Goal

Let an agent operate ordinary macOS applications in the background without
moving the real pointer, changing the user's active application, or silently
acting on another window owned by the same process.

V1 is deliberately conservative. It expands the routes for which Cua Driver can
prove an exact `(pid, CGWindowID)` destination and returns a structured refusal
for every other route. A refused action is preferable to a process-scoped key
event that appears successful after mutating a sibling window.

## Success criteria

V1 is complete when all of the following are true:

1. Every window-scoped macOS mutation carries the requested `(pid,
   CGWindowID)` from target resolution through route selection, dispatch, and
   post-action verification.
2. A result can be `confirmed` only from evidence belonging to that exact
   window or to an exact browser target bound to it. State from another
   same-process window is never accepted as evidence.
3. Background routing is ordered as semantic Accessibility, exact browser/CDP,
   exact window-local pointer, and finally PID keyboard with an independent
   exact-delivery proof. If none is safe, the driver refuses before sending
   input.
4. Background mode never silently crosses to global HID, activates another
   application, moves the real pointer, changes Space, or restores a minimized
   window.
5. Minimized and hidden windows retain safe semantic AX actions and current
   one-shot capture. Raw key commits are refused; callers use `AXConfirm`, an
   exact `AXPress`, or an exact browser action when one exists.
6. An off-Space window that is absent from the process's `AXWindows` is
   observation-only. Cua Driver does not use another window's AX tree or send
   process-scoped input while the target is unresolved.
7. Ordinary Electron applications get per-process AX enablement, exact
   `BrowserWindow` mapping, bounded fresh-tree acquisition, delayed effect
   verification, and explicit two-window safety coverage.
8. Existing action request schemas and the closed `ActionResult` wire shape
   remain compatible.
9. Focused unit/contract tests run in ordinary CI. Exact-SHA native evidence is
   collected separately on a signed macOS Namespace runner and in the
   canonical Lume acceptance lane.

## Non-goals

V1 does not attempt to provide:

- live minimized-window PiP or a window-scoped ScreenCaptureKit stream;
- continuously changing Chromium pixels while the application is occluded,
  hidden, or minimized;
- full Accessibility parity across Spaces;
- automatic Space switching, temporary Space membership, sticky windows, or
  switch-and-restore input;
- deminiaturize/order-behind/re-minimize transactions;
- minimize-button interception or application-specific window swizzling;
- a background raw-key guarantee for multi-window applications;
- a global-HID fallback in background mode;
- background control of games, Metal surfaces, protected video, secure input,
  or other surfaces that do not expose an exact semantic or routed target;
- generic WKWebView mutation, Safari JavaScript fallback expansion, or a new
  browser attachment mechanism;
- a redesign of the public action-result contract; or
- changes to application-owned Chromium settings such as
  `backgroundThrottling`.

The existing caller-selected `delivery_mode: "foreground"` remains a separate
last-resort behavior. This plan does not remove it, but the background router
must never enter it implicitly.

## Current source-grounded state

### Exact AX scoping exists

[`ax/window_scope.rs`](../rust/crates/platform-macos/src/ax/window_scope.rs)
already defines `Matched`, `NotFound`, `OwnerPidMismatch`, and `AxUnresolved`.
Its structural invariant is the right foundation: every non-`Matched` result
walks no elements.

[`ax/tree.rs`](../rust/crates/platform-macos/src/ax/tree.rs) unions
`AXChildren` and `AXWindows`, maps top-level AX windows to CG window IDs through
`_AXUIElementGetWindow`, and bounds the default walk at 2,000 nodes and depth
25. [`get_window_state.rs`](../rust/crates/platform-macos/src/tools/get_window_state.rs)
preflights WindowServer ownership, empties the element cache when the requested
window does not resolve, and still returns an exact-window screenshot for the
`AxUnresolved` degradation.

These checks prevent an unresolved request from returning a sibling window's
tree. Mutation needs to carry the same invariant farther than it does today.

### Explicit `window_id` does not make PID keyboard delivery window-scoped

[`window_target.rs`](../rust/crates/cua-driver-core/src/window_target.rs)
correctly promotes a unique PID-only target and refuses a PID with multiple
eligible top-level windows. Explicit `window_id` and element-token requests,
however, pass through that cardinality guard unchanged.

On macOS, [`keyboard.rs`](../rust/crates/platform-macos/src/input/keyboard.rs)
posts through `SLEventPostToPid` with a public `CGEvent::post_to_pid` fallback.
Both transports are process-scoped. [`press_key.rs`](../rust/crates/platform-macos/src/tools/press_key.rs)
best-effort focuses an element and then posts the key to the PID. Its pixel form
first invokes the click path to focus `(x, y)` and then posts the same
process-scoped key, so that combined pointer-plus-key branch needs one exact
route decision rather than bypassing keyboard safety after the focus click.

[`type_text.rs`](../rust/crates/platform-macos/src/tools/type_text.rs) has a
second exactness gap: when no explicit element pointer is supplied,
`read_axvalue(pid, None)` reads `focused_element_of_pid(pid)`. Its before/after
verification can therefore observe whichever same-process field is focused,
not necessarily a descendant of the requested `window_id`. Passing an explicit
window ID does not change that readback.

The required same-PID regression test follows directly from these source
semantics: unresolved target window A, focused sibling window B, process-routed
key, and PID-global focused-element readback. V1 must refuse before the key is
posted and prove from fixture journals that neither window changed.

### Exact semantic and window-local primitives already exist

[`ax_actions.rs`](../rust/crates/platform-macos/src/input/ax_actions.rs)
supports `AXPress`, `AXConfirm`, selection, focus, and enabled-state checks.
[`set_value.rs`](../rust/crates/platform-macos/src/tools/set_value.rs) already
does native AX value readback and correctly distrusts AXValue echo from an
`AXWebArea`.

[`mouse.rs`](../rust/crates/platform-macos/src/input/mouse.rs) has a routed
pointer primitive that stamps window-local location plus the target PID and
CGWindowID fields before SkyLight/PID posting. It does not move the real
pointer. The same file also contains private no-raise activation and
Chromium-specific event recipes. These are implementation candidates, not by
themselves proof that an action reached the requested window. V1 must gate them
on exact target facts and exact post-action evidence.

[`click.rs`](../rust/crates/platform-macos/src/tools/click.rs) already prefers
AX, can perform window-local pointer fallback for specific selection controls,
and distinguishes verified AX selection from an unverified dispatch. The new
router should consolidate those decisions rather than add a second hidden
fallback ladder.

### Electron AX enablement exists but needs lifecycle and mutation discipline

[`ax/bindings.rs`](../rust/crates/platform-macos/src/ax/bindings.rs) writes
`AXManualAccessibility` and falls back to `AXEnhancedUserInterface` only when
the modern attribute is unsupported. The tree walker enables it once per PID,
waits 500 ms when accepted, and then walks the bounded tree.

The current cache is a set of PIDs. V1 should associate enablement with the
observed process lifetime so PID reuse cannot inherit a stale "enabled"
decision. Electron mutation must reacquire an exact bounded tree rather than
using title order, the first AX window, or stale process-global focus.

### Exact browser mutation is already unique-or-refuse

[`browser/binding.rs`](../rust/crates/cua-driver-core/src/browser/binding.rs)
contains the pure native-window-to-CDP correlation. [`browser/engine.rs`](../rust/crates/cua-driver-core/src/browser/engine.rs)
checks native ownership, process fingerprint, endpoint ownership, CDP window
geometry/cardinality, tab identity, and generation again before every
mutation. [`browser/tools.rs`](../rust/crates/cua-driver-core/src/browser/tools.rs)
uses those exact capabilities for typed browser actions.

V1 should reuse this rung rather than creating a macOS-specific browser
shortcut. The bounded Electron fallback remains exact only when the endpoint
exposes one page and the owner has exactly one native window. Two Electron
`BrowserWindow`s must not use that fallback; they need a proven CDP-window
mapping or a refusal. WKWebView mutation remains unsupported.

### Capture and Spaces metadata are limited

[`capture.rs`](../rust/crates/platform-macos/src/capture.rs) currently obtains
a one-shot window image with `screencapture -l <CGWindowID> -x -o`. The
ScreenCaptureKit implementation is display-scoped recording, not a
window-scoped live stream. V1 keeps the current one-shot path and does not
claim frame freshness from API success.

[`list_windows.rs`](../rust/crates/platform-macos/src/tools/list_windows.rs)
describes minimized, hidden, and off-Space enumeration, but the current macOS
records do not reliably populate `current_space_id`, `on_current_space`, or
`space_ids`. V1 must therefore use exact AX-window presence as its input safety
gate instead of inventing a complete Spaces model.

### The result contract is intentionally closed

[`outputs.rs`](../rust/crates/cua-driver-contract/src/outputs.rs) defines the
closed `ActionResult`: effect, route, delivery, evidence, and escalation.
[`action_record.rs`](../rust/crates/cua-driver-core/src/action_record.rs) is the
functional core that conservatively projects native attempts to that public
shape. [`action-result-contract.md`](action-result-contract.md) explicitly
excludes echoed selectors, coordinates, and request targets.

V1 keeps that public result shape. Exact target facts belong to the request,
the internal execution record, a read-only capability report, and structured
refusals. Any future proposal to add optional result fields must update the
contract and every generated strict SDK together; it is not part of this work.

## Locked design decisions

### 1. Exact-window invariant

The canonical target key is the requested `(pid, CGWindowID)`. It is carried
unchanged through planning, execution, and verification.

The following rules are non-negotiable:

- WindowServer must attribute the CGWindowID to the requested PID immediately
  before mutation.
- An AX element route must prove that the live element ascends to an AX window
  whose `_AXUIElementGetWindow` is the requested CGWindowID.
- A browser route must revalidate its existing exact native-window capability
  immediately before mutation.
- A pointer route must use the requested window's validated pixel frame and
  stamped CGWindowID, then verify an effect on that same target.
- A keyboard readback must read the addressed element or a fresh element
  proven to descend from the requested AX window. It must never call a
  PID-global focused-element reader as target evidence.
- A sibling window's state may be retained as a negative canary, but it can
  never confirm the requested action.
- If the target disappears, changes owner, becomes AX-unresolved where the
  chosen route needs AX, or becomes ambiguous between planning and dispatch,
  no fallback actuator runs.

An explicit `window_id` is an address supplied by the caller, not proof that a
process-scoped actuator will honor it.

### 2. Functional core and imperative shell

Add a small pure background-input decision module in
`cua-driver-core`. It owns no macOS objects and performs no I/O. Its inputs are
facts collected by a platform shell:

- exact target and WindowServer ownership;
- AX scope (`matched`, `not_found`, `owner_mismatch`, `unresolved`);
- requested action semantics and trust class;
- target element ancestry and available semantic actions;
- target state (`visible`, `occluded`, `offscreen`, `minimized`, `hidden`, or
  unknown where macOS cannot distinguish safely);
- same-PID top-level-window cardinality;
- exact browser-binding eligibility;
- exact window-local pointer eligibility;
- keyboard destination and available verifier;
- baseline foreground PID, real pointer position, and Space posture; and
- permission/private-symbol readiness.

The pure output is one route with prerequisites and a verification plan, or a
typed refusal reason. Keep names internal, but model at least:

```text
ExactWindowTarget(pid, window_id)
TargetResolution
BackgroundRoute
VerificationPlan
BackgroundInputDecision = Execute(...) | Refuse(...)
```

Use a pure truth table for route choice and result classification. Extend the
existing `ActionExecutionRecord` projection rather than adding macOS wire
semantics. Platform adapters translate a chosen internal route to the existing
public `ActionRoute` values.

The macOS imperative shell:

1. serializes mutation planning per PID;
2. gathers fresh WindowServer, AX, browser, and foreground facts;
3. asks the pure core for exactly one decision;
4. revalidates route-specific facts immediately before dispatch;
5. invokes only that actuator;
6. polls only the planned exact-target verifier within a bounded deadline;
7. checks foreground PID, real pointer, and Space posture; and
8. emits the existing action result or a structured refusal.

The core never says "try all routes." The shell cannot improvise a second
actuator after a failed proof. A caller may take a newly reported route after
fresh state, preserving the existing agent-owned action ladder.

### 3. Background routing ladder

The decision order is:

| Order | Route | Required exactness | V1 behavior |
| --- | --- | --- | --- |
| 1 | AX semantic action/value | Exact AX window and exact element ancestry | Prefer `AXPress`, `AXConfirm`, selection, or value mutation with target-bound readback. AX API acceptance alone is unverified. |
| 2 | Browser/CDP | Existing exact native-window, endpoint, CDP-window, tab, generation, and ref proof | Reuse typed browser tools. Do not guess or auto-convert an AX token into a DOM ref. |
| 3 | Window-local routed pointer | Exact owner, AX-window presence, validated pixel frame, stamped CGWindowID, supported target state, and exact postcondition | Never move the real pointer. Private SPI absence or failed revalidation refuses. |
| 4 | PID keyboard | Exact target element/window, no competing same-PID keyboard destination, and an exact verifier available before posting | Native single-window text insertion may qualify. Generic keys, multi-window apps, minimized/hidden windows, and unresolved off-Space windows refuse. |
| 5 | Refusal | No safe route | Return the exact reason and available alternatives without sending input. |

The ladder preserves action semantics. It must not silently replace a trusted
browser click with a synthetic DOM event, infer a submit button from a label,
or turn a desktop request into a browser request without an exact capability.
Where request shapes cannot be translated exactly, the result advises the
explicit tool the caller may use next.

Global HID is not in this table. Only an explicit foreground request may use
the existing guarded foreground/HID path.

### 4. Semantic AX behavior and commit policy

For a token/index action, revalidate the retained element's ancestry against
the requested top-level AX window. A cached element under the right cache key
is not sufficient after a lifecycle or Space transition.

Safe commit behavior is explicit:

- use an advertised `AXConfirm` on the exact field when available;
- use `AXPress` on an explicitly addressed commit button;
- use an exact browser action when the caller has a live page/ref capability;
- otherwise refuse a background `Return` for minimized, hidden, unresolved,
  or multi-window targets.

Do not heuristically find the first default button or press another window's
button. Do not turn `press_key(Return)` into `AXPress` unless the request itself
identifies the semantic target.

Native `AXValue` equality can confirm a value operation on the exact retained
element. AX values under `AXWebArea` remain untrusted because the accessibility
shim can echo a write that the renderer did not consume. Electron/web content
requires browser/DOM readback or another exact application effect.

### 5. Electron policy

Electron support is part of v1, not a generic Chromium exception.

For each observed process lifetime:

1. Create the AX application element and try `AXManualAccessibility=true`.
2. Fall back to `AXEnhancedUserInterface=true` only when the modern attribute
   returns `kAXErrorAttributeUnsupported`; transient failures do not trigger a
   second private write or a success claim.
3. If enablement is accepted, wait the existing bounded settle and perform a
   fresh exact-window walk. Key the enablement cache by process fingerprint or
   generation, not an immortal numeric PID.
4. Build the top-level `BrowserWindow` map only from `_AXUIElementGetWindow`.
   Window title, array order, focused-window status, and first-match selection
   are not identity proofs.
5. Require the requested BrowserWindow to be in `AXWindows` before native
   mutation. An absent target returns no elements and no input route.
6. Before an element action, prove its ancestry to that BrowserWindow. After
   the action, poll a fresh bounded exact-window tree or exact CDP/DOM state.
   Never verify against process-global focus or a stale cached tree.
7. Use a delayed bounded verifier because renderer and AX state publication are
   asynchronous. Timeouts produce `unverifiable` or a proven delivery failure,
   never a fabricated confirmation.
8. Retain the existing distrust of web-area AXValue echo. Normal DOM text and
   key semantics should use the exact browser rung when a binding is available.
9. With two BrowserWindows, disable the single-page/single-native-window CDP
   shortcut and every raw PID-key route. A route is available only when the
   exact native/CDP mapping or exact semantic AX action survives revalidation.

The signed fixture launches once without `--force-renderer-accessibility` to
prove the driver's per-PID enablement works for a normal `BrowserWindow`, and
once with the existing fixture flag as a diagnostic control. It also covers
default `backgroundThrottling` and `backgroundThrottling:false`. The setting
may affect changing pixels, but it must not weaken exact-target input policy or
become a driver prerequisite.

### 6. Minimized and hidden windows

For minimized or application-hidden targets:

- keep exact one-shot capture as observation when it succeeds;
- label capture freshness as unknown rather than promising live frames;
- allow exact semantic AX actions with target-bound verification;
- allow an already-exact browser/CDP action only if all native and browser
  binding checks still pass;
- use semantic commit rather than a raw Return key;
- refuse PID keyboard and native routed pointer in v1; and
- do not deminiaturize, unhide, activate, reorder, move, or re-minimize the
  window as an implementation detail.

This preserves foreground state by construction. A no-activate restore
transaction can be investigated as a later opt-in mode, but it is not a v1
fallback.

### 7. Other-Space policy

Observation is allowed: `list_windows`, exact one-shot capture, and any honest
degradation metadata may still be returned.

For mutation, query a fresh application `AXWindows` and map every candidate via
`_AXUIElementGetWindow`:

- If the requested CGWindowID is absent, refuse all input with an
  `off_space_or_ax_unresolved` reason. Do not inspect or act on a sibling
  window, even if it is on the current Space.
- If the exact target is present, v1 may use a semantic AX or exact browser
  route whose own preconditions and postcondition pass. A native pixel or
  PID-key route is not promoted merely because capture succeeded.
- Unknown Space metadata remains unknown. Do not claim `on_current_space`
  without a real source.

Explicit switch/restore, temporarily adding a window to the current Space, and
sticky-window behavior require a later opt-in design with its own visible-state
and cleanup contract. V1 adds no SkyLight Spaces manipulation.

### 8. Exact pointer route

The existing window-local pointer implementation may be selected only when:

- WindowServer ownership and AX-window identity both match the requested
  target immediately before dispatch;
- the current screenshot dimensions have a validated mapping to the requested
  WindowServer bounds;
- the local point falls inside that exact window frame;
- the target is neither minimized, hidden, nor AX-unresolved/off-Space;
- the required private symbols resolve on the running macOS version;
- baseline frontmost PID and real pointer position have been captured; and
- an exact postcondition exists for the selected control.

Use the existing stamped local coordinate and CGWindowID fields. Treat private
no-raise activation as part of the actuator, not as delivery proof. If it
changes the user's frontmost application, pointer, or Space, mark the route
failed, restore only state owned by the driver when that is safe, and disable
the route for the remainder of the process session.

The v1 router does not use unverified pixel input for a mutation whose only
result is "event posted." It may remain available through current APIs as
`unverifiable` outside the new exact capability, but it must not be advertised
as exact background input.

### 9. PID keyboard gate

PID-routed keyboard is a last background rung because macOS accepts a process,
not a CGWindowID.

Before posting text, require all of:

- a live explicit element proven to descend from the requested AX window;
- that exact element is the PID's focused UI element and the PID's focused AX
  window maps to the requested CGWindowID;
- the requested target is the only eligible same-PID keyboard window at the
  pre-dispatch revalidation point;
- the target is visible or occluded on the current Space, not minimized or
  hidden;
- a target-bound value verifier is readable before dispatch; and
- the value surface is native, not an `AXWebArea` echo surface.

Post the event only after those facts pass, then poll the same retained target
or a freshly reacquired equivalent inside the exact requested tree. If any
precondition cannot be proven, refuse before input. Generic `press_key` and
hotkeys normally lack a safe target-specific postcondition and therefore
refuse in background exact mode unless a future action-specific verifier is
added.

The singleton rule is a conservative v1 proof, not a claim that
`SLEventPostToPid` is window-addressed. It prevents the known class of
same-process sibling delivery while semantic and browser routes cover most
ordinary controls.

### 10. Capability and result reporting

Add an additive `background_input` section to the read-only exact-window state
output. It should be produced from the same pure decision facts and contain no
window titles, control values, or private symbol addresses. A representative
shape is:

```json
{
  "background_input": {
    "exact_window": {
      "status": "matched",
      "pid": 123,
      "window_id": 456,
      "ax_window": "matched"
    },
    "routes": [
      {"route": "accessibility", "status": "available"},
      {
        "route": "pid_keyboard",
        "status": "refused",
        "reason": "same_pid_sibling_windows"
      }
    ],
    "observation": {
      "one_shot_capture": "available",
      "frame_freshness": "unknown"
    }
  }
}
```

The capability route and status strings above are illustrative and are not new
values for the closed public `ActionRoute` enum.

The exact enums and generated types should be reviewed with the implementation,
but the semantics are fixed:

- `available` means route prerequisites are currently proven, not that a
  future call is guaranteed to succeed;
- every action revalidates instead of trusting this report;
- unknown facts stay unknown;
- route refusal includes one stable machine-readable reason; and
- the report does not expose user content.

Keep successful action outputs on the current `ActionResult` contract:

- `route` uses the existing enum, including accessibility, synthetic events,
  system API, DOM, and trusted input for relevant background actions;
- `delivery.mode` reports what actually occurred;
- `confirmed` requires publishable exact-target readback;
- `unverifiable` means the route ran but exact effect is unknown;
- `refused` means no actuator ran; and
- escalation is advice for an explicit caller decision.

Structured refusals should include `code`, `effect: "refused"`, the requested
`pid` and `window_id`, the failed exactness fact, and the safe next route when
one exists. Initial codes should distinguish at least ownership mismatch,
AX-window unresolved/off-Space, same-PID keyboard ambiguity, unavailable
verification, and unavailable private route.

Do not report `delivery_failed` merely because the driver lacks evidence.
Use it only when exact negative evidence proves delivery did not occur. A
missing/unreadable postcondition is `unverifiable` with
`effect_unconfirmed`; an unchanged readable exact target after the bounded
delivery window may be `suspected_noop` or delivery failure according to the
existing action's semantics. API acceptance is never confirmation.

## State policy matrix

This is the required decision policy, not a claim that every route already has
native evidence:

| Target state | Observe | AX semantic | Exact browser | Window-local pointer | PID keyboard |
| --- | --- | --- | --- | --- | --- |
| Frontmost | Current state/capture | Yes when exact | Yes when exact | Yes when exact and verified | Only with singleton exact element and verifier |
| Background visible | Current state/capture | Yes when exact | Yes when exact | Yes when exact and verified | Same conservative gate |
| Fully occluded | One-shot capture; freshness honest | Yes when exact | Yes when exact | Yes only after signed proof and exact verifier | Same conservative gate |
| Fully offscreen on current Space | One-shot capture when valid | Yes when exact | Yes when exact | Signed-evidence gated; no geometry guess | Refuse in v1 |
| Minimized | One-shot capture, freshness unknown | Yes when exact | Yes only if exact binding revalidates | Refuse | Refuse; semantic commit instead |
| Application hidden | One-shot capture, freshness unknown | Yes when exact | Yes only if exact binding revalidates | Refuse | Refuse; semantic commit instead |
| Other Space, exact AX window present | Observation allowed | Yes when exact | Yes when exact | Refuse in v1 | Refuse in v1 |
| Other Space/AX window absent | Observation only | Refuse | Refuse | Refuse | Refuse |
| Owner mismatch/stale window | Refuse stale target; report owner when safe | Refuse | Refuse | Refuse | Refuse |

## Compatibility expectations

These are route expectations to certify, not unsupported parity claims:

| Surface | V1 expectation | Evidence required |
| --- | --- | --- |
| AppKit | Primary supported lane: exact AX actions/value, verified window-local pointer where semantic AX is unavailable, narrowly gated native text | Runtime AppKit fixture, including two windows under one PID |
| SwiftUI | Exact AX on the current Space; truthful refusal when an off-Space window disappears from `AXWindows` | Runtime SwiftUI fixture on current and another Space |
| Electron DOM | AX enablement and exact BrowserWindow tree; exact CDP route when bound; no multi-window PID keys | Runtime two-BrowserWindow fixture with per-window DOM journals |
| Mac Catalyst | Semantic AX or verified pointer only; raw keyboard remains gated | Source-supported candidate; runtime row required before advertising |
| Qt/Java | Use exposed exact AX semantics; refuse absent/custom AX and raw-key ambiguity | Source-based until dedicated signed fixtures exist |
| Browser canvas/WebGL | Exact browser action when a supported ref/route exists; otherwise no semantic claim | Source-based browser binding plus route-specific runtime tests |
| Games/Metal/custom event loops | No v1 background guarantee | Explicitly unsupported until an exact app-owned route exists |
| Secure/protected surfaces and secure input | No capture or input bypass | Refusal/permission tests; never weaken platform protection |
| System UI and out-of-process panels | Require actual WindowServer owner and exact AX window; never follow a PID silently | Existing owner-mismatch tests plus signed panel coverage |

`backgroundThrottling:false` may preserve Electron painting in cases where the
default renderer throttles. That is an application compatibility option, not a
Cua Driver route guarantee. Canvas/WebGL, trusted-event filters, and custom
event loops can still behave differently from ordinary DOM controls.

## Staged implementation

### Stage 0: Lock the safety regression

- Extend the AppKit and Electron fixtures with two fixed-title windows, one
  field and one commit control per window, and a run-owned per-window journal.
- Reproduce process-scoped keyboard delivery with target A unresolved or
  minimized and sibling B focused.
- Add a test that requests A by exact `(pid, window_id)` and requires a refusal
  before dispatch; both journals and both field values must remain unchanged.
- Add a unit regression showing that a sibling's focused-element readback can
  never satisfy A's verification plan.

Do not refactor routes until this failing behavioral shape and pure regression
are both captured.

### Stage 1: Pure exact-target router

- Add the target facts, route decision, verifier requirement, and refusal
  reasons to common core.
- Encode the complete state matrix as table-driven unit tests.
- Make "explicit `window_id`" distinct from "exact delivery proven."
- Extend internal action records with diagnostic exact-target proof state only
  as needed; do not change the public `ActionResult` shape.
- Add a per-PID mutation coordinator so two concurrent background decisions
  cannot change process focus underneath one another.

### Stage 2: Fresh macOS target acquisition

- Centralize WindowServer owner preflight and fresh `AXWindows` mapping.
- Add a bounded ancestry check from a retained element to its exact AX window.
- Make Chromium/Electron AX enablement process-lifetime aware and preserve the
  modern-attribute/fallback error distinction.
- Reacquire bounded exact-window state for delayed postconditions and clear
  stale caches on every unresolved result.
- Change the `AxUnresolved` input advice from unconditional pixel mutation to
  observation-only/refusal when exact input cannot be proven.

### Stage 3: Semantic actions and exact verification

- Route exact `AXPress`, `AXConfirm`, selection, and native value mutations
  first.
- Bind every readback to the requested target element/window.
- Preserve web-area AX echo distrust and add browser/DOM verification where an
  exact capability already exists.
- Make process-level window changes diagnostic only unless the changed window
  is the exact target postcondition.
- Normalize `confirmed`, `unverifiable`, `suspected_noop`, and proven delivery
  failure through the functional core.

### Stage 4: Browser, pointer, and keyboard rungs

- Reuse browser mutation revalidation without a second Electron-only binding
  implementation.
- Gate the existing window-local pointer route on exact target, state, pixel
  frame, private-symbol readiness, and verifier facts.
- Route `press_key`'s existing pixel-focus-then-key form through the same pure
  decision. A successful focus click cannot waive the exact AX-window,
  singleton destination, target-bound verifier, or state gates for the key.
- Remove PID-global focused-element readback from window-scoped keyboard
  verification.
- Permit native PID text only under the singleton exact-element/verifier gate.
- Refuse generic keys and every unsafe minimized/hidden/off-Space raw-key case.
- Assert that the background router has no call edge to global HID or
  foreground assist.

### Stage 5: Capabilities, documentation, and rollout

- Add the read-only per-window capability report and structured refusal codes.
- Keep request and `ActionResult` schemas compatible; regenerate/check SDK
  artifacts only if the read-only output has a generated typed surface.
- Update the background-delivery and known-limit documentation to match the
  certified matrix, not the intended matrix.
- Ship the high-risk same-PID refusal before expanding routes. If a new route
  regresses, disable that route while retaining the exact-target guard.
- Run the full desktop acceptance matrix once against the final exact SHA, as
  required by repository guidance.

## Test and acceptance plan

### Ordinary CI

Run these on every implementation pull request without a GUI or TCC grants:

- pure route-decision table for every state and route;
- owner mismatch, stale ID, AX-unresolved, and exact matched scope;
- explicit-window versus exact-delivery distinction;
- same-PID sibling rejection before actuator invocation;
- wrong-sibling readback ignored by verification;
- pixel-focus-then-key cannot bypass keyboard refusal or dispatch a second
  actuator after target facts change;
- no background decision can produce a global-HID transport;
- native value, missing readback, unchanged value, partial text, delayed
  success, and negative-delivery result classification;
- Electron PID-lifetime enablement cache and fallback behavior;
- exact BrowserWindow map cardinality and no first/title-only match;
- existing CDP binding/revalidation, including one-page/one-window fallback
  refusal after a second native window appears;
- minimized/hidden/off-Space policy truth tables;
- read-only capability and structured-refusal schema snapshots;
- existing closed `ActionResult` invariants and generated SDK parity; and
- fixture build/lint tests.

### Signed macOS behavior matrix

The native harness must use only repository fixtures and a fixture-owned
occlusion sentinel. Each action records pre/post foreground PID, real pointer
position, requested CGWindowID, AX window IDs, route, result, exact fixture
journal entry, and cleanup state.

Required rows:

1. **AppKit:** frontmost, background visible, fully occluded, offscreen,
   minimized, and hidden. Cover AXPress, AXConfirm, AXValue, exact pointer, and
   singleton native text.
2. **AppKit same PID/two windows:** request each window independently; minimize
   or make one AX-unresolved while the sibling is focused; raw key/text must
   refuse and neither sibling journal may change.
3. **SwiftUI:** background AX actions and value readback on the current Space;
   move only the fixture window to another preconfigured Space, query from the
   other Space, and require refusal whenever its CGWindowID is absent from
   `AXWindows`.
4. **Electron DOM/two BrowserWindows:** run without force-renderer AX first,
   then the diagnostic flag; run default throttling and
   `backgroundThrottling:false`; prove exact AX window mapping, AXPress,
   semantic commit, exact browser type/click, delayed readback, and raw-key
   refusal for minimized/unresolved targets. Every effect must appear in the
   addressed window's own fixture journal and not its sibling's.
5. **Browser routes:** exact bind and mutation, stale generation, geometry
   mismatch, second native window, ambiguous tab/window, and unavailable
   endpoint. Every ambiguous row refuses without mutation.
6. **Foreground preservation:** for every background success and refusal, the
   fixture sentinel remains frontmost, the real pointer is unchanged, and the
   current Space is unchanged.
7. **Capture:** record whether one-shot capture succeeds in each state and
   never infer live animation from a successful PNG alone.
8. **Permissions/private APIs:** missing Accessibility, missing Screen
   Recording, and missing private symbols return an honest unavailable/refused
   result without prompting or global fallback.
9. **Cleanup:** close fixture windows, stop all fixture/driver processes,
   restore the initial fixture foreground, pointer, and Space, and leave no
   run-owned artifact outside the selected artifact directory.

The other-Space setup belongs to the disposable runner image or a test-only
harness. Product code must not create, join, or switch Spaces. The test records
its starting Space and always restores it in a trap/finally path.

### Canonical Lume gate

The repository's canonical signed/TCC acceptance remains
[`tests/runners/macos-lume/README.md`](../tests/runners/macos-lume/README.md)
and [`run-all.sh`](../tests/runners/macos-lume/run-all.sh). It provides the
logged-in Aqua session, certificate-backed `CuaDriverLocal.app`, normal TCC
grant flow, and exact-source checks.

After focused Namespace evidence passes, add the new fixture rows to the Lume
entrypoint and run once on the final candidate SHA. Preserve its existing
requirements:

- use a disposable worker cloned from the stopped private seed;
- install with `install-local.sh --release --autostart
  --require-stable-signing`;
- never edit `TCC.db`;
- execute the GUI suite from the guest's logged-in Terminal, not SSH;
- retain exact SHA, environment, code-signing requirement, permission status,
  structured results, fixture journals, screenshots/recording where allowed,
  and cleanup evidence; and
- retrieve evidence, then delete the worker.

Namespace validation complements this gate; it does not replace the Lume
golden-image contract.

## Namespace macOS runner workflow

### Discovery recorded for this plan

The planning checkout was inspected on 2026-08-04:

- project `.amp/plugins` was absent;
- the surrounding workspace `.amp/plugins` was absent;
- user `~/.config/amp/plugins` contained
  `namespace-mac-runners.ts` and `namespace-mac-runners.js`;
- no repository-local Namespace runner instructions were found; and
- the TypeScript source registered the exact tool name
  `namespace_mac_runner`.

The user plugin is not versioned with this repository, so a later agent must
repeat discovery before use. Check project `.amp/plugins`, user
`~/.config/amp/plugins`, the active workspace plugin location, Amp plugin
documentation, and the loaded plugin source. If the tool is absent, record
those checks and make plugin discovery/setup step one; do not guess an API.

The discovered tool accepts:

```text
action: list | spawn | destroy
runner_id: optional hostname-compatible ID for spawn
repository_url: optional HTTPS github.com URL for spawn
duration_minutes: optional integer from 5 through 720
instance_id: required for destroy
```

The discovered implementation defaults to macOS `26.x`, arm64, 6 vCPU,
14,336 MB, and a 60-minute TTL. It requires `nsc login`. Spawn creates billable
infrastructure immediately without confirmation, clones into
`$HOME/workspace/repository` through Amp's GitHub credential helper, starts
`amp --no-tui --runner-id "$AMP_RUNNER_ID" --remote-control-terminal` in tmux,
and waits for `Registered. Create threads`. Failed provisioning attempts to
destroy the instance it created. Destroy accepts only plugin-managed instance
IDs and asks for confirmation. These details must be re-read from the installed
plugin because it can change independently of this plan.

### Exact later validation workflow

1. Re-run the plugin discovery above and record the resolved source/config.
2. Call `namespace_mac_runner` with `{"action":"list"}`. Do not modify or
   destroy unrelated runners.
3. Obtain explicit approval for the billable spawn. The approval for this plan
   PR does not authorize a later infrastructure charge.
4. Push the implementation candidate and record its full SHA. Spawn with a
   unique hostname-compatible ID and bounded TTL, for example:

   ```json
   {
     "action": "spawn",
     "runner_id": "cua-bg-input-<unique-suffix>",
     "repository_url": "https://github.com/trycua/cua.git",
     "duration_minutes": 180
   }
   ```

5. Record the returned runner ID, Namespace instance ID, URL, resolved macOS
   selector, repository URL, and deadline. TTL is a backstop, not cleanup.
6. Call Amp's `list_runners` immediately before `create_thread`. Start a child
   with `executor: "runner"` and that exact live runner ID; do not silently use
   an orb. Instruct it to fetch and detach-checkout the exact candidate SHA,
   because the plugin's shallow clone starts from repository default HEAD.
7. On the Namespace Mac, record `sw_vers`, OS build, architecture, SIP status,
   Xcode/Rust/Node versions, exact git SHA, and a clean worktree before testing.
8. Use repository-supported `CuaDriverLocal.app` setup only. Follow
   [`scripts/README.md`](../scripts/README.md), use an ephemeral
   certificate-backed local signing identity, and install with
   `--require-stable-signing`. Never copy a maintainer private key or signing
   keychain to Namespace. Never edit `TCC.db` or bypass TCC.
9. Grant Accessibility and Screen Recording only through normal macOS UI if the
   disposable Namespace environment and explicit test approval support it.
   Record `permissions status --json` and the `codesign -d -r-` designated
   requirement. The Namespace plugin provisions neither a signing identity nor
   TCC. If stable signing, interactive UI, or either grant is unavailable,
   mark the signed native lane blocked and run only pure/build tests; do not
   claim native behavior passed.
10. Use only fixed-title repository fixtures and run-owned artifact paths.
    Capture the matrix evidence listed above, including exact fixture journal
    effects, requested and sibling window IDs, AX counts/mapping, result
    payloads, foreground PID, real pointer, Space posture, and action timing.
    Do not collect unrelated application state, window titles, or user data.
11. In a finally/trap path, stop fixture and driver processes, close fixture
    windows, restore fixture focus/pointer/Space state, remove run-owned
    temporary files, and write a cleanup assertion. Retrieve only sanitized
    evidence needed for review.
12. From the controlling thread, call
    `namespace_mac_runner` with
    `{"action":"destroy","instance_id":"<recorded-instance-id>"}` and confirm
    destruction. Call `{"action":"list"}` again and require the instance to be
    absent.

## Risk analysis

### macOS and private APIs

`_AXUIElementGetWindow`, `AXManualAccessibility`,
`AXEnhancedUserInterface`, SkyLight event posting, private event fields, and
no-raise activation may change between macOS releases. Every lookup must be
bounded and fail closed. Missing symbols remove a route; they never cause a
global or PID-only fallback.

V1 adds no private Spaces API. Space membership remains incomplete, and the
policy deliberately refuses when AX cannot prove the exact target.

### TCC and signing identity

Accessibility and capture evidence is valid only for the process identity that
performed it. Ad-hoc rebuilds can invalidate grants. Native acceptance requires
the certificate-backed `CuaDriverLocal.app` flow and normal permission UI.
Neither CI mocks nor a successful build substitutes for this evidence.

### Application semantic differences

AX API success may be a no-op, Chromium AXValue may be an echo, and custom
renderers may reject synthetic input. Confirmation always comes from an
independent exact-target effect. Unsupported applications receive narrower
capabilities, not a misleading parity claim.

### Races and lifecycle

Windows can close, change owner, add a modal, or restart between observation
and input. Per-PID serialization and immediate revalidation reduce the window;
they do not make stale capabilities permanent. Any changed fact invalidates
the decision and requires fresh state.

### Compatibility and rollout

Request schemas and successful action results remain stable. Some calls that
currently post best-effort PID keys will begin refusing when exact delivery is
not provable. That is an intentional safety correction. Refusals include a
recoverable semantic/browser/foreground alternative when one actually exists.

Roll out one route at a time after its fixture lane passes. The rollback is to
disable the new route decision while retaining the same-PID/unresolved-target
guard. Do not restore the unsafe process-scoped fallback to improve success
rates.

Telemetry and retained artifacts may contain route, refusal reason, OS build,
target PID/window ID, timing, and boolean invariants. They must not contain
window titles, field values, typed text, screenshots of unrelated apps,
browser URLs, or fixture logs outside the fixed test vocabulary.

## Alternatives rejected for v1

- **Post to the PID and verify any focused field.** This is the wrong-window
  bug, not a verification strategy.
- **Assume explicit `window_id` makes `post_to_pid` exact.** The keyboard
  transport has no window parameter.
- **Restore without activation, order behind, act, and re-minimize.** This
  changes target state, can still change internal key-window routing, and adds
  cleanup races. It belongs in a later opt-in design.
- **Move the window offscreen or to another Space.** This mutates user desktop
  state and does not solve AX or renderer suspension reliably.
- **Temporarily make a window sticky or add it to the current Space.** This is
  full Spaces manipulation, outside v1's contract.
- **Treat capture success as input exactness.** A CGWindowID image proves the
  capture target, not the destination of a process-scoped key.
- **Treat AX API acceptance or event-post return as confirmation.** Both can
  report success without the intended application effect.
- **Silently use foreground HID.** It violates the background contract and can
  act on the user's current application.
- **Add request echoes to the closed `ActionResult`.** Callers already own the
  request; a wire change would require coordinated strict-SDK migration without
  improving route safety.

## Open implementation questions

Resolve these during Stage 0/1 review, before enabling a route:

1. Which existing read-only output surface should own the additive
   `background_input` capability in every SDK without duplicating native
   window state?
2. Which stable process fingerprint should replace the PID-only Chromium AX
   enablement cache on macOS?
3. Which native text-field properties are sufficient to predeclare a safe
   postcondition for the singleton PID-key rung?
4. Can the current private no-raise pointer preparation satisfy foreground,
   pointer, and Space invariants on every supported macOS build? Builds without
   signed evidence must keep the route disabled.
5. What bounded delayed-verification deadline gives Electron enough time while
   staying below the public tool timeout? Use evidence from the fixture rather
   than reusing an unrelated settle constant blindly.
6. How will the disposable Lume/Namespace image provide a deterministic second
   Space for tests while guaranteeing restoration? This is a test-environment
   decision, not a product Space API.
