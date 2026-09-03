---
title: Exact target-app input bridge for custom canvases
authors:
  - "@0xjohnnydev"
created: 2026-09-03
last_updated: 2026-09-03
status: draft
discussion: https://github.com/trycua/cua/issues/3532
rfc_pr:
implementation: []
supersedes:
superseded_by:
---

# RFC: Exact Target-App Input Bridge for Custom Canvases

## Summary

Let Cua Driver operate custom canvases such as Blender's 3D Viewport without
moving the real pointer or activating the target application. The driver keeps
one public set of input tools. An application or framework adapter translates
those typed actions into the application's own event loop and binds every
action to the requested operating-system window.

This is the practical meaning of "universal" in this plan: one driver contract,
one routing policy, and thin adapters for applications or common frameworks.
There is no reliable operating-system API that can inject background input into
every hardened game, renderer, or custom event loop. An application that
rejects normal macOS synthetic events must expose, embed, or load an adapter
before this route can support it.

Ordinary Accessibility, browser, and routed native input remain available.
The bridge is an exact app-owned route inside the existing background-input
ladder, not a second fallback system and not permission to activate an app.

## Motivation

The existing exact macOS background-input work intentionally does not promise
support for games, Metal surfaces, or custom event loops. Those applications
can discard process-routed mouse and keyboard events even when Cua Driver
addresses the correct process and leaves the user's session alone.

**Observed in the Blender prototype:** Blender accepted background clicks when
its Python helper queued events on Blender's own main thread. The production
`click` tool selected objects in both the 3D Viewport and Outliner while a
separate foreground application stayed active and the physical pointer stayed
still. The input route did not start the macOS screen-capture consent process.

That prototype is retained at commit `7fc9dcda7` on
`feat/cua-driver-macos-input-bearing-agent-cursor`. It proves the application
event-loop route, not a universal implementation:

- only production `click` reaches the bridge;
- key, hotkey, and text support exist only in the Blender helper and were
  called directly in the simultaneous-workflow demo;
- discovery and dispatch are click-shaped rather than shared typed primitives;
- the branch is stacked on closed Agent View work rather than current `main`;
  and
- Blender must start with `--enable-event-simulate` so its supported simulation
  API is available.

The universal work should port the useful seam, not merge or extend that branch
as-is.

## Goals

- The normal `click`, `drag`, `scroll`, `press_key`, `hotkey`, and `type_text`
  tools can select an app-owned route when the exact requested window
  advertises the needed capability.
- The existing Agent Cursor follows the resolved app-local point and remains
  aligned with the target window even when the application does not consume
  operating-system pointer events.
- The target can remain behind another application. The foreground
  application, physical pointer, window order, and current Space are not
  changed as implementation details.
- The driver reports the route and effect honestly. A bridge queue
  acknowledgement is delivery evidence, not proof that application state
  changed.
- Unsupported actions refuse or use a route selected by the existing planner.
  They do not partially execute through the bridge and then retry through a
  different actuator.

## Non-goals

- No injection into an arbitrary unmodified application that rejects the
  operating system's available background routes.
- No bypass of secure input, protected content, app sandbox policy, code
  signing, or macOS permissions.
- No screen-capture replacement. Bridge-owned coordinate metadata can avoid a
  capture request during input planning, but live visual observation still
  needs an observation source with its own permission contract.
- No first-stage expansion to minimized, hidden, or unresolved off-Space
  targets. Existing exact-target state restrictions remain in force until a
  separate change has native evidence for those states.
- No framework auto-detection by window title, process name, or geometry.
  An adapter must identify itself and the exact windows it owns.

## Terminology

- **Target-app bridge:** the driver-owned protocol and routing contract between
  Cua Driver and an adapter inside or explicitly loaded by a target app.
- **Adapter:** thin app- or framework-specific code that translates typed Cua
  input into that application's supported event API.
- **App-owned route:** delivery through the target's event queue or input model,
  rather than a process-routed operating-system event.
- **Exact window:** one live operating-system window bound by process identity,
  process generation, native window identifier, adapter window token, and
  current coordinate revision.
- **Agent Cursor:** Cua's visual cursor overlay. It shows the agent's resolved
  point but does not itself deliver application input.

## Current state

Current `main` implements conservative exact-target background routing for
Accessibility, browsers, native window-local pointers, and narrowly gated
process keyboard input. It deliberately has no guarantee for custom event
loops. The pure planner chooses one actuator or refuses, and the macOS shell
serializes mutations per process.

The Blender prototype adds a process-owned Unix socket and queues clicks through
Blender's own event API. Its useful seam is app-owned delivery; its
click-specific discovery, direct demo keyboard client, and branch ancestry are
not the proposed production design.

## Compatibility research

The following rows identify likely compatibility gaps and supported in-process
conversion seams. Except for Blender, these are **source or bundle inspection
only**, not live Cua Driver results.

| App or framework family | Why ordinary background input may miss | Candidate adapter seam | Current evidence |
| --- | --- | --- | --- |
| Blender | Its custom editor event loop discarded the process-routed pointer path used by the driver | Blender's documented [`Window.event_simulate`](https://docs.blender.org/api/current/bpy.types.Window.html#bpy.types.Window.event_simulate) on its main thread | Production click observed on the prototype branch |
| FreeCAD, OpenSCAD, IDA, and other Qt/OpenGL tools | Canvas widgets may consume Qt or renderer-local events rather than a process-level macOS event | A loaded Qt adapter can map the native window to a `QObject` and use [`QCoreApplication::postEvent`](https://doc.qt.io/qt-6/qcoreapplication.html#postEvent); this machine's app bundles confirm Qt in those three applications | Bundle and API inspection only |
| Godot editor and Godot applications | The engine owns propagation into viewports and game nodes | [`Input.parse_input_event`](https://docs.godotengine.org/en/stable/classes/class_input.html#class-input-method-parse-input-event) feeds an event into the game without moving the operating-system cursor | Official API inspection only |
| Unity editor extensions and Unity applications using the Input System | Input is integrated into engine device state on frame updates | [`InputSystem.QueueEvent`](https://docs.unity3d.com/Packages/com.unity.inputsystem@1.14/manual/Events.html) queues an event for a later input update | Official API inspection only |
| SDL applications and games | Apps poll SDL's queue or device state rather than accepting a native event sent to the process | [`SDL_PushEvent`](https://wiki.libsdl.org/SDL3/SDL_PushEvent) feeds the queue and preserves event filters | Official API inspection only; SDL warns that pushed device events do not change device state |
| Dear ImGui tools | The UI is driven from an immediate-mode input queue rebuilt each frame | The official backend contract accepts `AddMousePosEvent`, `AddMouseButtonEvent`, `AddMouseWheelEvent`, `AddKeyEvent`, and text input through [`ImGuiIO`](https://github.com/ocornut/imgui/blob/master/docs/BACKENDS.md) | Official source inspection only |
| Java AWT/Swing custom canvases | Input targets Java components on the AWT dispatch thread | [`EventQueue.postEvent`](https://docs.oracle.com/en/java/javase/25/docs/api/java.desktop/java/awt/EventQueue.html#postEvent(java.awt.AWTEvent)) with an exact component source | Official API inspection only |
| Electron and browser canvas/WebGL | Native Accessibility may be sparse even though the renderer has an exact browser target | Keep the existing exact browser/CDP route ahead of the bridge; use an adapter only for native modules the browser route cannot express | Existing driver architecture; no new bridge evidence |
| Hardened proprietary creative apps with no plug-in API | The app may reject synthetic events and offer no supported in-process integration point | No universal hook is claimed; use an existing exact route or refuse | Compatibility boundary, not a tested failure |

This research changes one part of the protocol design: adapters must declare a
delivery model, not only an action name. `event_queue` means handlers may see an
event, while `device_state` means polling APIs also observe the corresponding
state. SDL demonstrates why the driver cannot treat those as equivalent.

The first adapters should be Blender and a small repository fixture, followed
by Qt or Dear ImGui. Qt covers several installed creative/developer tools;
Dear ImGui has the cleanest cross-platform input conversion surface. Live app
support still requires the app to load the adapter and pass the conformance
suite.

## Proposal

### 1. Put typed protocol data in the functional core

Add protocol types and validation to `cua-driver-core`; keep Unix-socket and
platform process inspection in `platform-macos`. The pure layer should model:

```text
TargetAppAdapterIdentity
TargetAppWindowBinding
TargetAppCapabilities
TargetAppInputAction
TargetAppDispatchDisposition
TargetAppEffectEvidence
TargetAppCursorTransform
```

The core validates version negotiation, capability/action agreement, target
identity, coordinate bounds, payload limits, and disposition transitions
without opening a socket. The platform shell discovers and authenticates an
adapter, gathers fresh target facts, asks the existing background policy for
one route, revalidates, and dispatches once.

Do not make the protocol a bag of adapter-defined JSON commands. Every input
kind is a closed driver-owned variant so an adapter cannot turn this socket into
an arbitrary remote procedure interface.

### 2. Negotiate structured capabilities

The current prototype advertises opaque action strings. Replace them with a
versioned, structured description. Capabilities state exactly which buttons,
modifiers, repeat counts, key namespace, coordinate space, and payload sizes an
adapter accepts.

An illustrative description is:

```json
{
  "protocol": 2,
  "adapter": {
    "id": "org.blender.cua-event-adapter",
    "version": "1.0.0",
    "process_generation": "54402:818806378704"
  },
  "windows": [
    {
      "window_token": "blender-main-38c5",
      "logical_size": { "width": 1512, "height": 916 },
      "backing_scale": 2,
      "frame_revision": 17,
      "coordinate_space": "window_content_top_left"
    }
  ],
  "capabilities": {
    "delivery_model": "event_queue",
    "pointer": {
      "click": {
        "buttons": ["left", "right", "middle"],
        "max_count": 3,
        "modifiers": ["shift", "control", "option", "command"]
      }
    },
    "keyboard": {
      "key": { "namespace": "cua_key_v1" },
      "hotkey": { "max_keys": 5 },
      "text": { "max_utf8_bytes": 4096 }
    }
  }
}
```

Protocol types should use integer pixel or logical coordinates with an
explicit origin and scale. They should not assume all frameworks use AppKit
window coordinates. A driver conversion is valid only when the adapter's
fresh logical size and scale match the exact target's validated frame.

The binding also publishes a content rectangle and transform used by both
input and Agent Cursor rendering. The driver converts the requested point once,
then gives the app-owned route and cursor overlay the same resolved local and
screen coordinates. A stale transform refuses input rather than showing the
cursor in one place and acting in another.

### 3. Bind the adapter to one exact live window

Every dispatch carries all of:

- the requested process identifier and macOS window identifier;
- a kernel-authenticated socket peer process;
- a process-generation fingerprint so a reused numeric process identifier
  cannot inherit a binding;
- an adapter-owned opaque window token;
- a fresh frame revision; and
- the capability version used to plan the action.

WindowServer must still attribute the requested window to that live process
immediately before dispatch. The adapter must describe exactly one compatible
window binding. Zero matches, multiple matches, stale revisions, changed
ownership, or changed process generation refuse before input.

Size and scale are correlation facts, not identity by themselves. The first
implementation can use them to cross-check a token, but never to choose among
same-sized sibling windows. The repository fixture must include two identical
windows under one process to lock this rule.

### 4. Dispatch one closed input action

An illustrative request is:

```json
{
  "action": "dispatch",
  "protocol": 2,
  "request_id": "0199579d-7bb7-7df3-82bd-36cd43e81dd2",
  "target": {
    "pid": 54402,
    "window_id": 9031,
    "process_generation": "54402:818806378704",
    "window_token": "blender-main-38c5",
    "frame_revision": 17
  },
  "input": {
    "kind": "pointer_click",
    "point": { "x": 742, "y": 418 },
    "button": "left",
    "count": 1,
    "modifiers": ["shift"]
  }
}
```

The complete action union should cover:

- pointer move, click, bounded drag path, and two-axis scroll;
- key down/up as one balanced driver-owned operation;
- a bounded hotkey chord with explicit modifier ordering; and
- UTF-8 text, distinct from key events.

Drag paths and text are bounded before serialization. Unknown fields,
duplicate request identifiers, invalid modifier combinations, coordinates
outside the declared content rectangle, and unsupported action variants are
rejected without queueing an event.

### 5. Separate pre-dispatch refusal from post-dispatch uncertainty

The adapter response must distinguish whether application input could have
started:

```json
{
  "request_id": "0199579d-7bb7-7df3-82bd-36cd43e81dd2",
  "disposition": "queued",
  "effect": "unverifiable",
  "target": {
    "window_token": "blender-main-38c5",
    "frame_revision": 17
  }
}
```

Initial dispositions should be closed values:

- `unsupported_before_dispatch`: the action was not queued;
- `rejected_before_dispatch`: validation failed and nothing was queued;
- `queued`: the exact app event loop accepted the action; and
- `effect_observed`: a separate target-bound verifier observed the requested
  postcondition.

Capability discovery, not a post-dispatch fallback response, decides whether
the bridge is a candidate. Once the planner selects the bridge, any timeout,
disconnect, malformed response, rejection, or capability race ends that action.
The shell does not try a native actuator afterward because it cannot prove the
application did not already receive the input.

`queued` maps to `unverifiable` unless the driver has an independent
target-bound readback. An echoed request, a socket write, and an adapter's
generic success reply cannot produce `confirmed`. A verifier may be an exact
semantic state readback, an app-owned state query that is separate from the
mutation acknowledgement, or another existing target-bound source accepted by
the action-result policy.

### 6. Insert one route into the existing background planner

For a custom canvas action, route order becomes:

1. exact semantic Accessibility action;
2. exact browser action;
3. exact target-app bridge;
4. exact native window-local pointer;
5. narrowly gated process keyboard; or
6. refusal.

The precise order is action-dependent: a requested raw pointer gesture must not
be silently replaced by a different semantic operation. The important rule is
that discovery contributes fresh facts to the same pure decision, which emits
one actuator and one verification plan. Tool implementations must not call the
bridge opportunistically before or after their existing fallback ladders.

Bridge dispatch stays under the current per-process background mutation lease
so two simultaneous actions cannot invalidate window focus, capability, or
frame facts between planning and dispatch. Foreground mode remains an explicit
caller choice; the bridge never activates it as an escalation.

Add `MacosTargetAppBridge` as an exhaustive internal action transport and map
it to the existing public `system_api` route. Keep the closed public
`ActionResult` shape unchanged.

Agent Cursor updates remain in the driver's existing visualization path. An
adapter does not draw another cursor and does not gain access to session overlay
state. For an exact app-owned pointer action, the selected route commits the
fresh cursor transform and dispatch record together under the same per-process
lease. A rejected or stale target does not animate a misleading successful
click.

### 7. Keep discovery independent from pixels

The prototype currently reaches adapter discovery through pixel-frame code.
Split that responsibility:

- adapter discovery authenticates a process and enumerates window bindings;
- window binding supplies optional coordinate metadata;
- observation independently supplies pixels and freshness; and
- each tool requests only the facts its action needs.

`press_key`, `hotkey`, and `type_text` must not initialize capture merely to
find an adapter. A pointer action can use adapter-provided size and scale when
they correlate exactly with current WindowServer bounds. If visual coordinates
come from a screenshot, its frame identity must still match the action target.

This separation prevents an input-only workflow from causing a screen-recording
permission prompt. It does not claim that Agent View can show fresh pixels
without an authorized observation path.

## Security, privacy, and telemetry

The Blender prototype already rejects non-sockets, wrong owner users, loose
filesystem modes, and a kernel peer process that differs from the target. The
production design also needs the adapter to authenticate Cua Driver rather than
trusting any same-user client.

Before implementation, lock one launch and authentication contract that works
for both development and signed releases. The preferred shape is:

- a per-driver-instance rendezvous directory accessible only to the current
  user;
- kernel socket peer credentials checked in both directions;
- a short-lived registration capability passed when the driver or its helper
  launches the adapter;
- an adapter and driver signing requirement in installed builds, with an
  explicit development policy rather than silently accepting ad-hoc code; and
- a process-generation fingerprint rechecked before each mutation.

Never put a reusable secret on a command line or in a world-readable `/tmp`
name. Limit message size, action count, path points, text bytes, connection
count, and response time. Close on unknown protocol versions or fields that
change action semantics. The protocol exposes no filesystem, shell, Python
evaluation, arbitrary method name, or adapter-defined command surface.

If attaching to an already-running app cannot meet the mutual-authentication
contract, the first release should require a Cua-provided launcher rather than
weakening authentication. That is a product limitation to surface, not a
reason to accept an unauthenticated local control socket.

Adapter telemetry may contain adapter identifier/version, capability names,
input kind, disposition, timing, and stable refusal code. It must not contain
typed text, window titles, control values, screenshots, raw payloads, or
application documents.

## Adapter model

The driver contract is shared; event-loop integration is necessarily specific.
Keep adapters thin and prefer one maintained adapter per framework when the
framework exposes a stable event API:

| Surface | Likely adapter boundary | Initial status |
| --- | --- | --- |
| Blender | Python add-on using Blender event simulation | Prototype observed; port first |
| Repository canvas fixture | Small app with an explicit event journal | Build with the protocol; required conformance lane |
| Browser canvas/WebGL | Existing exact browser/CDP route first; bridge only for capabilities CDP cannot express | Existing route preferred |
| Electron | Existing Accessibility/CDP route first; optional preload/native adapter only when needed | Follow-up candidate |
| Qt/QML | Application plug-in or framework event posting API | Theory only |
| SDL/game loop | Linked event-queue adapter | Theory only |
| Unity | Package that enqueues input into the player loop | Theory only |
| Java/AWT | In-process agent or supported event-queue API | Theory only |
| AppKit/Metal custom view | Small application-owned adapter protocol implementation | Conformance candidate |

Do not advertise a framework row until its adapter passes the same exact-window,
foreground-preservation, negative-control, and crash tests as the repository
fixture.

## Alternatives considered

### Continue adding operating-system injection recipes

This keeps each change inside the platform adapter, but cannot make a custom
event loop consume an event it intentionally ignores. It also accumulates
framework guesses and private focus behavior without giving the target app a
stable exact-window contract.

### Ship a Blender-only helper

This is the shortest path to the observed demo, but leaves every new app to
invent discovery, authentication, coordinates, action semantics, and result
reporting. The prototype should become the first adapter, not the architecture.

### Inject one universal dynamic library into arbitrary apps

A loaded shim can intercept common framework functions without app changes,
but universal binary injection conflicts with hardened runtime, library
validation, sandboxing, runtime versions, and application support contracts.
It also makes Cua responsible for safely patching unrelated processes. The
proposal permits explicit, app-authorized adapter loading but does not make
covert library injection the default compatibility mechanism.

### Activate, interact, and restore

This can use ordinary input but visibly races the user, changes keyboard
destination, and cannot reliably restore application state. It remains an
explicit foreground mode, never a background compatibility fallback.

## Implementation plan

### PR 1: Protocol, discovery, and production Blender click

- Add the typed core protocol, strict decoder, capability validation, exact
  window binding, and disposition model.
- Add authenticated macOS discovery and a single bridge route in the existing
  background planner.
- Port Blender `click` from `7fc9dcda7` through the typed route.
- Add a repository-owned two-window custom-canvas fixture and an adapter
  conformance harness.
- Route Agent Cursor visualization through the same exact window transform as
  adapter input.
- Keep `drag`, `scroll`, keyboard, and text capability variants defined only if
  the first implementation can validate them without widening the PR. Otherwise
  reserve protocol feature identifiers rather than shipping dead handlers.
- Keep current minimized, hidden, and off-Space refusal policy.

### PR 2: Keyboard and text through production tools

- Route `press_key`, `hotkey`, and `type_text` through shared discovery and
  dispatch rather than the prototype's direct socket client.
- Define the cross-framework key namespace and explicit text/key distinction.
- Add focused-field, shortcut, UTF-8, repeat, modifier, and same-process sibling
  tests.
- Remove the direct helper path from the demo workflow.

### PR 3: Pointer sequences and adapter kit

- Add bounded drag paths, hover/move, and scrolling with cancellation and
  balanced button state.
- Publish a small adapter kit plus conformance tests for lifecycle, capability,
  target binding, and effect reporting.
- Add the next framework only after the shared kit reduces application-specific
  code and the negative tests remain framework-neutral.

Minimized, hidden, and other-Space support is separate work. It must not enter
these stages merely because an adapter appears capable of queueing an event.

## Test and acceptance plan

### Definition of Done

The universal mode is ready to advertise only when all of these are true:

- [ ] Standard production Cua Driver tools operate the bridge. No demo-only
  direct socket client is needed.
- [ ] Blender and a repository-owned custom-canvas fixture implement the same
  versioned protocol without Blender conditionals in the driver core.
- [ ] One process with two same-sized windows proves that an action mutates
  only the requested macOS window.
- [ ] Foreground application, key window, physical pointer, window order, and
  current Space remain unchanged during covered background actions.
- [ ] Agent Cursor appears at the same resolved target point used by the
  adapter across scale factors and does not move the physical pointer.
- [ ] Adapter discovery and keyboard/text planning do not start screen capture
  or trigger a screen-recording consent prompt.
- [ ] Click, drag, scroll, key, hotkey, and text either work through declared
  capabilities or refuse before dispatch with a specific reason.
- [ ] Spoofed socket, wrong user, wrong peer process, process-identifier reuse,
  stale window token, stale frame revision, ambiguous binding, oversized input,
  adapter crash, timeout, and disconnect cases fail closed.
- [ ] A post-dispatch failure never causes a second actuator to run.
- [ ] `confirmed` requires independent exact-target evidence; queue acceptance
  alone reports `unverifiable`.
- [ ] Applications without an adapter retain their existing route selection
  and action-result behavior.
- [ ] Signed native evidence covers Blender and the generic fixture with a
  deliberate foreground-loss/input-leak canary and retained per-window event
  journals.
- [ ] The implementation is rebased onto or deliberately reconciled with
  [PR #3530](https://github.com/trycua/cua/pull/3530), which changes the
  overlapping macOS click, mouse, and private-window route.

### Evidence plan

Pure tests should cover protocol decoding, capability/action mismatches,
payload bounds, target and revision comparison, process-generation changes,
one-route planning, and action-result classification. Socket tests should use
real Unix sockets where practical and assert kernel peer identity rather than
mocking the security boundary away.

The repository canvas fixture should expose two identical-looking windows,
separate event journals, and deterministic controls for accept, reject,
delayed acknowledgement, partial write, disconnect, crash, stale revision, and
effect/no-effect. Every negative result must record whether dispatch was
possible; only proven pre-dispatch failures may be considered safe to replan on
a fresh caller action.

Native Blender and fixture runs should record:

- tested commit and installed artifact identity;
- requested and observed process/window identities;
- adapter identity, protocol version, process generation, token, and revision;
- before/after foreground process, key window, pointer, window order, and Space;
- target and sibling journals plus exact postcondition;
- action result and internal transport; and
- the screen-capture consent-process inventory before and after input-only
  rows.

The prior Blender recording remains evidence for the prototype and visual
presentation only. It must not be cited as production keyboard/text proof.

## Compatibility and migration

The bridge is additive and initially disabled unless an authenticated adapter
describes an exact compatible window. Removing or disabling the adapter returns
an application to the pre-bridge route ladder without changing request or
result schemas. Unsupported platforms keep an explicit unavailable capability;
they do not report a universal route that is not implemented.

Protocol versions negotiate exact compatible ranges. A new semantic field or
delivery model requires a new version or capability; adapters never ignore an
unknown field that can change input meaning. The first implementation can be
rolled back by removing route advertisement while leaving adapters inert.

The main integration risk is concurrent work in the same macOS actuator:

Draft PR #3530 changes the same macOS `click`, mouse, and private SkyLight
window path that the prototype touched. Universal bridge work should not copy
those files wholesale from `7fc9dcda7`. Land or rebase the target-only focus
fix first where practical, then add bridge capability collection and one-route
selection around its current behavior. Until that reconciliation and fresh
native proof exist, this branch is **prepared only**, not a working universal
mode.

## Unresolved questions

- Should the first release require Cua to launch every adapter, or can an
  already-running app establish equally strong mutual authentication?
- Which cross-framework key namespace preserves keyboard-layout and shortcut
  semantics without exposing platform-native key codes as the contract?
- What app-owned readback is sufficiently separate from dispatch
  acknowledgement to count as independent effect evidence?
- Should the first protocol version ship only click, or include keyboard/text
  capabilities even if their production routes land in a second PR?
- Does Agent Cursor update on a proven pre-dispatch refusal, or only after the
  route commits to dispatch? The default proposal is the latter.
- Which component owns adapter distribution, compatibility ranges, and updates?

## Decision record

No decision yet. This RFC is a draft for review under
[issue #3532](https://github.com/trycua/cua/issues/3532). Production routing
implementation waits for the recorded RFC decision.
