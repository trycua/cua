# Exact Target-App Input Bridge

**Status:** Proposed implementation plan; no code on this branch yet

**Base:** `origin/main` at `986b6f257b1afddef0cbd4815bb2744eab7eadba`

**Date:** 2026-09-03

## Goal

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

## Why this route is needed

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

## Product boundary

### What users get

- The normal `click`, `drag`, `scroll`, `press_key`, `hotkey`, and `type_text`
  tools can select an app-owned route when the exact requested window
  advertises the needed capability.
- The target can remain behind another application. The foreground
  application, physical pointer, window order, and current Space are not
  changed as implementation details.
- The driver reports the route and effect honestly. A bridge queue
  acknowledgement is delivery evidence, not proof that application state
  changed.
- Unsupported actions refuse or use a route selected by the existing planner.
  They do not partially execute through the bridge and then retry through a
  different actuator.

### What this does not promise

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

## Architecture

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

### 8. Authenticate both ends and bound the protocol

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

## Staged implementation

### PR 1: Protocol, discovery, and production Blender click

- Add the typed core protocol, strict decoder, capability validation, exact
  window binding, and disposition model.
- Add authenticated macOS discovery and a single bridge route in the existing
  background planner.
- Port Blender `click` from `7fc9dcda7` through the typed route.
- Add a repository-owned two-window custom-canvas fixture and an adapter
  conformance harness.
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

## Definition of Done

The universal mode is ready to advertise only when all of these are true:

- [ ] Standard production Cua Driver tools operate the bridge. No demo-only
  direct socket client is needed.
- [ ] Blender and a repository-owned custom-canvas fixture implement the same
  versioned protocol without Blender conditionals in the driver core.
- [ ] One process with two same-sized windows proves that an action mutates
  only the requested macOS window.
- [ ] Foreground application, key window, physical pointer, window order, and
  current Space remain unchanged during covered background actions.
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

## Test and evidence plan

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

## Known integration risk

Draft PR #3530 changes the same macOS `click`, mouse, and private SkyLight
window path that the prototype touched. Universal bridge work should not copy
those files wholesale from `7fc9dcda7`. Land or rebase the target-only focus
fix first where practical, then add bridge capability collection and one-route
selection around its current behavior. Until that reconciliation and fresh
native proof exist, this branch is **prepared only**, not a working universal
mode.
