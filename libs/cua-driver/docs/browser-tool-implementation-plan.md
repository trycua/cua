# Cua Driver Browser Tool Implementation Plan

## Status

- **State:** Browser-tool v1 implemented; later migration and rollout phases remain open
- **Plan date:** 2026-07-14
- **Source revision:** `f6aa3dfc24f376c793a0292f15847ee946104231`
- **Companion audit:** [Agentic Web Browsing with Cua Driver](agentic-web-browsing-audit.md)
- **Architecture verdict:** Accept with major changes

The first implementation milestone covers the five typed tools, strict endpoint
ownership, session capabilities, exact-or-refused Chromium mutation, launch
parity, and the repository-owned Electron E2E row. It deliberately leaves the
legacy `page` facade on its existing implementation until adoption evidence is
available, keeps `browser_prepare` to endpoint detection and explicit setup
refusals rather than profile mutation/restart, and defers the broader
real-browser and embedded-webview matrix described in Phases 4, 6, and 7. See
[the implementation journal](browser-tool-implementation-journal.md) for exact
evidence and remaining work.

## Objective

Add a first-class browser control surface to Cua Driver that lets an agent start from the existing native discovery flow, bind an exact browser page when the platform can prove that relationship, inspect compact browser state, and perform typed browser actions without losing Cua Driver's native desktop, background-delivery, focus-preservation, and truthful-refusal guarantees.

The intended agent flow is:

```text
start_session
list_apps
list_windows
get_browser_state
browser_prepare       # only when get_browser_state reports requires_setup
get_browser_state
browser_navigate / browser_click / browser_type
get_browser_state     # verify
get_window_state      # browser chrome, prompts, downloads, pickers, native fallback
```

The implementation must improve exact targeting and agent ergonomics. It must not hide browser setup inside a getter, silently guess a tab, or create a second copy of native window state.

## Rationale

This proposal comes from recent cross-platform validation of browser automation on macOS, Windows, Linux X11, and Linux Wayland.

Cua Driver currently separates several [capture and delivery modalities](https://cua.ai/docs/concepts/capture-and-delivery-modalities): AX and PX targeting, background and foreground delivery, and window and desktop scope. The [agent action ladder](https://cua.ai/docs/reference/cua-driver/action-selection-policy) starts with background AX, then tries background PX, the `page` tool, and finally foreground delivery. This ordering remains the right safety model because the agent begins with the most semantic and least disruptive route, then escalates only when the previous route is unavailable or cannot be verified. The current OS-specific boundaries are listed in [platform support](https://cua.ai/docs/reference/cua-driver/platform-support).

Web content does not always fit this ladder cleanly. Some Chromium actions cannot be delivered reliably to an occluded or unfocused page through native synthetic pointer or keyboard routes. The same operations can often run in the background through the Chrome DevTools Protocol (CDP) or, on macOS, Apple Events. The current [`page` workflow](https://cua.ai/docs/how-to-guides/driver/drive-a-web-page) exposes some of these routes, but its capabilities vary by operating system, it does not reliably bind the native `pid` and `window_id` to one exact browser tab, and its setup requirements are difficult for an agent to discover before an action fails.

This leaves some browser actions with best-effort background behavior even when a browser-native route could provide full-background execution. Shared-workspace agents need the stronger contract: supported page actions must avoid raising the browser, moving the cursor, stealing focus, or leaking input into the user's active application.

The proposal keeps the existing action ladder and makes its browser rung first-class and capability-aware. `get_browser_state(pid, window_id)` identifies the exact tab when the relationship can be proven and reports the available routes. Supported page actions can then use CDP or Apple Events before falling back to native PX or foreground delivery. `get_window_state` remains the source of truth for browser chrome, permission prompts, downloads, file pickers, and native fallback.

The first implementation step is a binding feasibility spike. If Cua Driver cannot reliably bind, revalidate, or safely refuse multi-window Chromium targets across supported operating systems, the design will change before the public API is committed.

## Success Criteria

The browser tool is ready for a supported route when all of the following are true:

1. An agent begins with the normal `pid` and `window_id` discovered through Cua Driver.
2. `get_browser_state` reports browser identity, available routes, setup requirements, tabs, and compact page references without causing visible or persistent side effects.
3. A page action names a session-scoped `browser_target_id`, exact `tab_id`, and snapshot-scoped page reference where applicable.
4. The driver proves that a debugging endpoint belongs to the expected browser process before connecting.
5. The driver either revalidates the page/window relationship before mutation or refuses the action.
6. Every action reports the actual route, input trust class, and verification result.
7. Ambiguous, stale, unsupported, or consent-gated targets produce structured refusals.
8. Browser page state does not duplicate the native accessibility tree or screenshot by default.
9. Existing `page` clients continue working during migration.
10. Exact-SHA real-browser evidence exists for every advertised platform/browser route.

## Non-Goals

This project will not:

- replace `get_window_state` or the native AX/PX action surface;
- turn Cua Driver into a Playwright-compatible browser server;
- require an owned Chrome for Testing installation for the default workflow;
- assume that Cua Driver owns the user's browser profile or network stack;
- provide ambient "current browser" or "current tab" global state;
- silently retry, self-heal, or switch routes after an exact-target check fails;
- promise a stable tab model for browsers or embedded webviews that do not expose one;
- copy an agent-browser-style daemon or make a browser instance equivalent to a Cua session;
- add universal `browser_wait`, `browser_close`, or tab lifecycle operations in v1;
- deprecate `page` before the new browser surface has accepted cross-platform evidence.

## Locked Design Decisions

### 1. Native discovery remains the entry point

The browser API begins with the exact native target already used by Cua Driver:

```json
{
  "session": "research-1",
  "pid": 1234,
  "window_id": 5678
}
```

`pid` plus `window_id` is the connection anchor, not a guarantee that a browser tab can be derived. The browser binding response must state whether the relationship is exact, heuristic, or unavailable.

### 2. `get_browser_state` is strictly read-only

`get_browser_state` may:

- classify the process and browser engine;
- inspect already-known or prompt-free local endpoints;
- read `/json`-style target metadata;
- verify local TCP endpoint ownership;
- reuse an already-approved cached browser socket;
- return structured setup requirements.

It may not:

- signal a process;
- restart a browser;
- patch browser preferences;
- enable Apple Events JavaScript;
- manipulate favorites or browser UI;
- create or copy a profile;
- make a connection known to trigger a consent prompt;
- navigate or mutate page content.

This is required for truthful MCP `readOnlyHint`, host approval behavior, and trajectory recording.

### 3. Setup and consent are explicit

`browser_prepare` owns all setup that can mutate state or require user consent. It is idempotent and statically marked mutating. When restart is possible, the tool must be marked destructive for approval purposes even if a particular call does not restart.

### 4. Native and browser state remain separate

`get_window_state` remains the source of truth for:

- native screenshot and capture metadata;
- AX/UIA/AT-SPI elements and native element tokens;
- browser chrome, tab strip, omnibox, menus, permission prompts, downloads, and file pickers;
- window focus, foreground state, geometry, visibility, and native delivery evidence;
- every non-browser application.

`get_browser_state` owns only what the native state cannot express reliably:

- browser and engine identity;
- page/tab identity;
- URL, title, and route-specific load state;
- DOM-derived compact page references;
- browser capabilities and setup requirements;
- binding method and confidence;
- DOM focus, where available.

Screenshot is opt-in and must use the same capture provenance as `get_window_state`. AX-only surfaces do not get a duplicate browser ref namespace; they direct the caller back to `get_window_state`.

### 5. Bindings are session-scoped capabilities

A `browser_target_id` is not a portable identifier. It is a capability owned by one Cua Driver session and one live driver instance. It must not be serialized into trajectories, replayed, or accepted after session cleanup.

Every stateful browser tool requires an explicit public `session`. Internal connection identity may support transport bookkeeping but must not replace the caller-visible session contract.

### 6. Exact mutation or refusal

Read-only state may report `binding.confidence` as:

- `exact`: the driver can prove the native target, browser endpoint, browser window, and page relationship needed for the requested operation;
- `heuristic`: useful metadata exists, but exact mutation is unsafe;
- `none`: the relationship cannot be established.

V1 browser mutation requires `exact`. It must never automatically promote a heuristic binding. A future explicit product policy may permit narrowly defined heuristic reads, but not writes.

### 7. Browser actions are typed tools

Do not add generic `browser_act` or `browser_get` tools. They would reproduce the current `page` problem: unrelated read and mutation semantics forced into one schema and one set of MCP annotations.

V1 uses a small set of typed tools:

```text
get_browser_state
browser_prepare
browser_navigate
browser_click
browser_type
```

### 8. Input trust is explicit

Every browser input result identifies one of:

- `trusted_input`: a trusted browser input route such as CDP Input domain dispatch;
- `dom_event`: an in-page JavaScript operation such as `element.click()`;
- `native_ax`: a native accessibility action delegated to Cua Driver;
- `native_px`: a native pixel action delegated to Cua Driver;
- `unavailable`.

The driver must not represent a DOM event as a trusted pointer or keyboard event.

## Proposed Public Contract

### `get_browser_state`

Purpose: discover a browser route, bind an exact page when possible, return compact page state, and refresh existing bindings without side effects.

Proposed request:

```json
{
  "session": "research-1",
  "pid": 1234,
  "window_id": 5678,
  "tab_id": "t-a1",
  "include_refs": true,
  "refs": "interactive",
  "max_refs": 200,
  "include_screenshot": false
}
```

Rules:

- `session`, `pid`, and `window_id` are required for initial discovery.
- Existing callers may pass `browser_target_id` plus `tab_id` for refresh.
- Omitting `tab_id` is allowed for state discovery and tab listing.
- Mutation tools must reject multiple viable tabs when no `tab_id` is supplied.
- `refs` accepts `interactive`, `all`, or `none`; default is `interactive`.
- Output is bounded by `max_refs` and a tool-level text budget.
- `include_screenshot` defaults to false.
- Read-only discovery never runs `browser_prepare` implicitly.

Proposed response:

```json
{
  "browser_target_id": "bt-3f",
  "native_target": {
    "pid": 1234,
    "window_id": 5678
  },
  "binding": {
    "method": "cdp_window_bounds",
    "confidence": "exact",
    "port_owner_verified": true,
    "revalidated": true
  },
  "browser": {
    "kind": "chrome",
    "engine": "chromium",
    "profile_kind": "default"
  },
  "tabs": [
    {
      "tab_id": "t-a1",
      "url": "https://example.com",
      "title": "Example",
      "active": true,
      "load_state": "complete"
    }
  ],
  "page": {
    "tab_id": "t-a1",
    "refs_snapshot": "p1b3",
    "ref_source": "dom",
    "refs": [
      {
        "ref": "p1b3:4",
        "role": "button",
        "name": "Submit",
        "frame": [112, 340, 88, 32]
      }
    ]
  },
  "routes": {
    "read": "cdp",
    "navigate": "cdp",
    "input_trusted": "cdp",
    "input_dom": "cdp"
  },
  "requires_setup": [],
  "limitations": []
}
```

No universal `state_revision` is included in v1. Snapshot staleness is represented by `refs_snapshot`; tab and binding validity are checked directly.

### `browser_prepare`

Purpose: perform explicit consent-bearing or mutating setup needed to expose a safe browser route.

Proposed request:

```json
{
  "session": "research-1",
  "pid": 1234,
  "window_id": 5678,
  "enable": ["cdp"],
  "profile": {
    "mode": "existing"
  },
  "allow_restart": false,
  "user_confirmed": true
}
```

Supported setup operations are platform-specific and capability-gated:

- connect to a user-enabled Chromium debugging endpoint after explicit consent;
- launch or relaunch a driver-owned Chromium process with a debugging port;
- create or select an isolated automation profile;
- enable macOS JavaScript from Apple Events;
- coax an Electron inspector only through this mutating tool;
- report manual setup instructions when the driver cannot safely automate the step.

The result returns a refreshed browser target or a structured refusal. It must report whether it restarted an application, changed preferences, copied profile data, or displayed a consent prompt.

`user_confirmed` is illustrative, not a sufficient security mechanism by itself. A model can populate a Boolean. Phase 4 must bind confirmation to an MCP-host approval event, an existing Cua approval mechanism, or a short-lived token minted only after explicit user action. The implementation must not treat an unverified request field as proof of consent.

### `browser_navigate`

Purpose: navigate an exact tab.

Proposed request:

```json
{
  "session": "research-1",
  "browser_target_id": "bt-3f",
  "tab_id": "t-a1",
  "operation": "goto",
  "url": "https://example.com/dashboard",
  "wait_until": "domcontentloaded",
  "timeout_ms": 25000
}
```

Operations:

- `goto`
- `back`
- `forward`
- `reload`

`wait_until` is supported only on routes with reliable navigation events. Other routes return a structured route refusal rather than imitating a load event with an undocumented poll.

### `browser_click`

Purpose: click a snapshot-scoped page reference using an explicitly requested trust class.

Proposed request:

```json
{
  "session": "research-1",
  "browser_target_id": "bt-3f",
  "tab_id": "t-a1",
  "ref": "p1b3:4",
  "input": "trusted"
}
```

Rules:

- `input` defaults to `trusted`.
- If trusted input is unavailable, the driver refuses rather than silently using `element.click()`.
- The caller may explicitly request `dom_event` for applications where that semantic is acceptable.
- The result reports coordinates, route, trust class, and verification state.
- Stale references require a fresh `get_browser_state` call.
- Before trusted pointer dispatch, the driver refreshes the element box and performs a hit test. If an overlay, modal, or another element covers the target point, the action refuses and identifies the covering node when safely available.

### `browser_type`

Purpose: insert text or dispatch browser keystrokes to an exact page target.

Proposed request:

```json
{
  "session": "research-1",
  "browser_target_id": "bt-3f",
  "tab_id": "t-a1",
  "ref": "p1b3:8",
  "text": "hello@example.com",
  "mode": "insert"
}
```

Modes:

- `insert`: direct text insertion without individual key events;
- `keystrokes`: trusted key event dispatch;
- `dom_value`: explicit DOM value mutation, if added after v1 validation.

The result reports the actual mode and trust class. Browser-tool v1 requires a
fresh `ref` and proves that it resolves to the focused editable element before
typing; it refuses rather than reporting success against an unproven focus.

## Structured Refusals

Add browser-specific refusal codes to the shared structured result vocabulary:

```text
browser_route_unavailable
browser_requires_setup
browser_binding_ambiguous
browser_binding_stale
browser_wrong_target_refused
browser_tab_required
browser_tab_not_found
browser_ref_stale
browser_input_trust_unavailable
browser_endpoint_owner_mismatch
browser_consent_required
```

Every refusal includes:

- requested operation;
- native target and browser target when available;
- requested route or trust class;
- actual capabilities;
- whether setup can resolve the refusal;
- a structured `requires_setup` entry when applicable;
- no claim that input or navigation occurred.

## Internal Architecture

### Shared `BrowserEngine`

Create a shared browser subsystem in `cua-driver-core` rather than extending the current platform-specific `PageBackend` divergence.

Proposed responsibilities:

```text
cua-driver-core/src/browser/
  mod.rs              public internal API and engine composition
  types.rs            targets, tabs, routes, setup, results, refusals
  discovery.rs        endpoint and browser target discovery interfaces
  binding.rs          native-window-to-browser-window/page correlation
  cdp.rs              shared CDP transport and protocol operations
  pool.rs             browser socket and flat-session lifecycle
  refs.rs             DOM snapshot refs and stale-ref validation
  session_store.rs    session-scoped browser targets and cleanup
```

Final paths may follow existing crate conventions, but ownership boundaries must remain:

- core owns browser target semantics, CDP target selection, session lifecycle, route reporting, and page refs;
- platform crates own process identity, endpoint ownership proof, browser classification, native window metadata, and platform-specific setup;
- native action implementations remain in the platform crates;
- `page` becomes a compatibility adapter over the shared engine.

### Platform adapter contract

Each platform supplies:

```text
classify_browser(pid)
validate_window_owner(pid, window_id)
discover_pid_owned_endpoints(pid)
correlate_browser_window(pid, window_id, browser_window_metadata)
prepare_browser_route(request)
capture_provenance(pid, window_id)
```

Windows' existing HWND-to-PID validation becomes the behavioral baseline. Equivalent fail-closed validation is required on macOS and Linux wherever platform APIs expose ownership.

### Browser target store

Each target record contains:

```text
session
browser_target_id (opaque and unguessable)
pid
window_id
browser kind and engine
endpoint identity and owner proof
browser WebSocket UUID where applicable
binding method and confidence
binding fingerprint
tab mappings
created_at and last_used_at
```

Lifecycle rules:

- target records are scoped to one explicit Cua session;
- `end_session`, connection teardown, and idle TTL remove records and page refs;
- process exit or endpoint identity change invalidates the record;
- actions never silently create a replacement target;
- browser targets cannot be replayed from trajectory artifacts;
- log output excludes cookies, auth headers, storage, page secrets, and endpoint credentials.

### CDP connection and concurrency model

Replace the current mixture of one-shot core calls and macOS-only cached mutable targeting with one shared model:

1. One ref-counted browser WebSocket per approved, PID-owned endpoint.
2. CDP flat mode enabled.
3. One CDP session ID per Cua session and tab.
4. Every command routed with the exact CDP session ID.
5. Per-target mutation serialization.
6. Independent read requests may run concurrently when response correlation is safe.
7. Session cleanup detaches its CDP sessions without closing a socket still used by another Cua session.
8. Reusing an approved socket must not retrigger a browser consent prompt.

This removes the cross-session risk of a process-global socket with mutable "current target" state.

### Reference model

Reuse the existing snapshot-token concepts but keep browser refs in a distinct namespace:

```text
native ref:  s<snapshot>:<index>
browser ref: p<snapshot>:<index>
```

Rules:

- browser tools reject native refs;
- native tools reject browser refs;
- navigation invalidates page refs;
- meaningful DOM replacement invalidates affected refs where detectable;
- a stale ref never falls back to selector text or screen coordinates;
- refs are session- and tab-scoped;
- ref snapshots are bounded by count, memory, and TTL.

For Chromium, generate refs from the browser accessibility tree and DOM identity rather than inventing selectors:

1. Read the page accessibility tree through CDP.
2. Retain the corresponding backend DOM node identity when available.
3. Store tab, document, frame, backend node, role, accessible name, state, and bounds in the ref snapshot.
4. Before action, resolve the backend node again and verify that it still belongs to the same tab, document, and frame.
5. Refresh the box model and perform an element-at-point coverage check before trusted pointer input.
6. Refuse when the node is detached, covered, outside the viewport without a supported scroll route, or moved into an unsupported frame context.

Page refs must carry `frame_id`. Main-frame support is required for v1. Same-process and out-of-process iframe support must be capability-tested separately; unsupported frame routes are omitted with a limitation rather than flattened into an incorrect main-frame target.

## Endpoint Ownership and Security

Before any CDP or inspector connection, the driver must prove that the local listening endpoint belongs to the expected browser process or an allowed browser process in its verified parent/child process tree.

Platform work:

| Platform | Ownership strategy                                                                       |
| -------- | ---------------------------------------------------------------------------------------- |
| macOS    | Existing PID-to-port discovery via `lsof`, tightened to verify endpoint and process tree |
| Windows  | `GetExtendedTcpTable` plus process-tree validation                                       |
| Linux    | `/proc/net/tcp*` socket inode mapping plus `/proc/<pid>/fd` and process-tree validation  |

Security requirements:

- loopback endpoints only unless a separately reviewed remote-browser provider is introduced;
- reject caller-supplied ports whose owner cannot be proven;
- pin and revalidate the browser WebSocket identity;
- never probe a fixed default port and trust whichever process answers;
- never record authenticated endpoint URLs or profile secrets;
- require explicit confirmation for authenticated default-profile debugging;
- expose browser output as untrusted content with bounded size and source metadata;
- do not implement network allowlists or interception in the driver unless a separate policy design establishes ownership and enforcement semantics.

## Binding and Revalidation

### Chromium

The preferred binding algorithm is:

1. Validate the native `pid` and `window_id` relationship.
2. Discover only debugging endpoints owned by that browser process tree.
3. Enumerate page targets and browser window IDs through CDP.
4. Fetch browser window bounds.
5. Correlate browser window bounds and title metadata with native window metadata.
6. Record the binding method, confidence, and fingerprint.
7. Before every mutation, confirm the target still exists and re-check its browser-window relationship.

Tab drag is a required invalidation scenario: a CDP target may survive while moving to another browser window. The next action must re-correlate or refuse.

### Non-Chromium routes

- Safari Apple Events may expose active-window or mutable tab-index semantics; v1 must not mint a durable exact tab ID from a heuristic ordinal.
- Firefox has no current page engine; return structured unavailable capabilities while evaluating WebDriver BiDi separately.
- WKWebView and WebKitGTK may have one page per native host and no tab model; report that explicitly.
- Wayland bindings with missing PID or geometry return `heuristic` or `none` and do not permit browser mutation.

## Cross-Platform Delivery Scope

| Surface                 | V1 state                                                     | V1 mutation                                       | Required setup                                       | Initial confidence target       |
| ----------------------- | ------------------------------------------------------------ | ------------------------------------------------- | ---------------------------------------------------- | ------------------------------- |
| Chrome/Edge/Brave macOS | CDP metadata and refs where approved                         | Navigate, trusted input, explicit DOM input       | CDP port/profile or user-approved debugging          | Exact or refuse                 |
| Electron macOS          | CDP metadata and refs                                        | Navigate and input                                | Inspector exposure through prepare when needed       | Exact for single-window fixture |
| Safari macOS            | Identity, capabilities, active page metadata where read-only | Deferred unless exact route is proven             | Apple Events/TCC through prepare                     | Heuristic or none               |
| Chrome/Edge Windows     | CDP metadata and refs                                        | Navigate and input                                | Fix launch CDP support or explicit existing endpoint | Exact or refuse                 |
| WebView2 Windows        | CDP metadata and refs                                        | Navigate and input where fixture exposes endpoint | Launch-time debugging arguments                      | Exact for fixture               |
| Firefox Windows         | Identity and unavailable routes                              | Native `get_window_state` only                    | None in v1                                           | None                            |
| Chromium Linux X11      | CDP metadata and refs                                        | Navigate and input                                | Add launch CDP support                               | Exact or refuse                 |
| Chromium Linux Wayland  | CDP metadata; native correlation when available              | Only exact binding                                | Add launch CDP support and compositor metadata       | Exact or refuse                 |
| Electron/Tauri Linux    | CDP where the host exposes it; native fallback otherwise     | Capability-dependent                              | Inspector exposure through prepare                   | Exact for supported fixture     |
| Firefox Linux           | Identity and unavailable routes                              | Native `get_window_state` only                    | None in v1                                           | None                            |

Support documentation must distinguish browser identity, page read, DOM refs, navigation, trusted input, DOM input, native background input, and native foreground input. A single "browser supported" label is insufficient.

## Implementation Phases

### Phase 0: Binding feasibility spike

Purpose: prove or disprove the window-anchored design before exposing a public API.

Work:

- add an internal Chromium binding probe behind test-only code or an ignored diagnostic test;
- add PID-owned endpoint discovery prototypes on macOS, Windows, and Linux;
- query browser targets, browser window IDs, and bounds;
- correlate against Cua native windows;
- exercise real Chrome with multiple windows and tabs;
- record exact, heuristic, and ambiguous outcomes without mutating pages.

Required scenarios:

1. One window, one tab.
2. One window, multiple tabs.
3. Two windows, two tabs each.
4. URL-substring-colliding tabs.
5. Two same-sized and same-position windows.
6. Tab dragged between windows after binding.
7. Closed tab and closed window.
8. Browser process restart and PID reuse protection.
9. Fake `/json` endpoint on a common debugging port.
10. Native Wayland window with missing geometry.

Exit gate:

- deterministic binding or exact refusal on real Chromium across macOS, Windows, and Linux;
- endpoint-owner mismatch always refused;
- no consent prompt or browser mutation from the read-only probe.

Kill criterion:

If multi-window Chromium cannot be deterministically bound or honestly refused on all three OS families, stop the window-bound API implementation. Pivot to tab-first targeting, with native-window correlation exposed only as advisory metadata.

### Phase 1: Shared CDP foundation and launch parity

Work:

- create shared browser/CDP types and error vocabulary;
- unify core and macOS CDP implementations behind one transport;
- implement PID-owned endpoint discovery for each OS;
- enforce loopback and owner verification;
- implement flat-mode session routing and response correlation;
- fix Windows `launch_app.cdp_debugging_port` so it is no longer a no-op;
- add equivalent Linux launch arguments and debugging-port support;
- preserve existing macOS launch behavior;
- add unit tests for endpoint discovery, ownership, target selection, and connection pooling.

Exit gate:

- existing `page` tests still pass unchanged;
- no browser endpoint can be used without owner proof;
- two Cua sessions can use different tabs on one browser endpoint without cross-delivery;
- launch schema behavior is truthful and documented on all OSes.

### Phase 2: Session-owned browser target model

Work:

- add browser target store keyed by explicit Cua session;
- add target IDs, tab IDs, binding fingerprints, confidence, and TTL;
- extend PID/window ownership validation to macOS and Linux;
- add session-end and connection-end cleanup;
- add distinct page-ref storage and stale-ref errors;
- prohibit trajectory replay of browser target capabilities;
- add granular capability tokens and refusal codes.

Exit gate:

- target IDs cannot cross sessions;
- ending one session does not break another session sharing the same browser endpoint;
- process, endpoint, window, tab, and ref invalidation each fail closed;
- no browser secrets appear in logs or artifacts.

### Phase 3: Read-only `get_browser_state`

Work:

- register the platform-specific read-only tool schema;
- implement browser classification and route discovery;
- return browser target, binding confidence, tabs, capabilities, and `requires_setup`;
- generate compact DOM-derived refs for exact CDP targets;
- support bounded output and opt-in screenshot;
- return identity and structured unavailable routes on non-CDP surfaces;
- ensure no discovery path can signal, restart, patch, prompt, or navigate.

Exit gate:

- MCP annotations and trajectory recording treat the tool as read-only;
- repeated calls are idempotent except for refreshed observed state;
- multi-tab state is deterministic and bounded;
- real-browser state evidence exists on macOS, Windows, Linux X11, and Linux Wayland where applicable.

### Phase 4: Explicit `browser_prepare`

Work:

- define structured setup requirements and confirmation semantics;
- support driver-owned isolated Chromium profiles;
- support user-approved existing-profile debugging without silently copying or modifying profile data;
- move Electron inspector signaling behind prepare;
- move Apple Events preference changes and browser restart behind prepare;
- return exact side-effect evidence;
- make prepare idempotent and safe to retry after interruption.

Exit gate:

- every consent prompt or restart is attributable to a mutating recorded call;
- declining setup leaves browser and profile unchanged;
- accepted setup produces a refreshed target or a structured residual limitation;
- two sessions reuse an approved endpoint without duplicate consent prompts.

### Phase 5: Navigation and typed browser input

Work:

- implement `browser_navigate` for exact CDP targets;
- implement trusted `browser_click` through browser input dispatch;
- implement explicit `dom_event` click as a separate requested trust class;
- implement `browser_type` insert and keystroke modes;
- report route, trust, coordinates, and verification;
- add navigation and mutation invalidation for page refs;
- add per-target mutation serialization;
- refuse unsupported waits and trust classes structurally.

Exit gate:

- actions never use first-page fallback;
- stale tabs and refs never silently re-resolve;
- trusted input produces real browser event sequences in the fixture journal;
- DOM input is visibly labeled and separately tested;
- focus and leaked-input sentinels prove background behavior where advertised.

### Phase 6: `page` compatibility facade

Work:

- route all seven existing `page` actions through the shared BrowserEngine;
- preserve legacy argument behavior and first-page fallback only for legacy calls;
- add optional `session` and `browser_target_id` inputs additively;
- make read results identify DOM versus AX/UIA/AT-SPI approximation;
- remove duplicate macOS-only CDP state after parity is proven;
- document deprecation without removing the tool.

Exit gate:

- existing `page` tests and downstream schema tests pass;
- browser tools and legacy `page` calls share one underlying target and CDP implementation;
- no supported `page` behavior regresses;
- deprecation begins only after accepted browser-tool evidence exists.

### Phase 7: Release evidence, documentation, and rollout

Work:

- add exact-SHA browser result tables and artifacts;
- add real-browser maintainer-dispatched CI lanes;
- document the native-window versus browser-page mental model;
- add a browser preparation how-to for isolated and authenticated profiles;
- add browser API reference and platform capability tables;
- update bundled skills and validate examples against each OS schema;
- add upgrade guidance from `page`;
- add route-level telemetry without capturing page secrets.

Exit gate:

- public claims match retained real-browser evidence;
- every code example is schema-tested on the platform it describes;
- release candidates require the accepted browser matrix;
- `page` remains available for at least two releases after formal deprecation.

## Proposed PR Sequence

Keep each PR independently reviewable and avoid exposing incomplete public tools.

| PR  | Scope                                                       | User-visible change                          | Merge requirement                     |
| --- | ----------------------------------------------------------- | -------------------------------------------- | ------------------------------------- |
| 1   | Chromium binding spike and real-browser discriminators      | None                                         | Kill criterion passes                 |
| 2   | Shared CDP transport, endpoint ownership, and launch parity | More truthful/hardened existing CDP behavior | Existing page E2E green               |
| 3   | Session target store, flat-mode routing, refs, and refusals | None or additive structured internals        | Concurrency and cleanup tests green   |
| 4   | Read-only `get_browser_state`                               | New state tool                               | Cross-platform state matrix accepted  |
| 5   | `browser_prepare` and consent/profile flows                 | New mutating setup tool                      | Consent and restart evidence accepted |
| 6   | `browser_navigate`                                          | New typed navigation                         | Real-browser navigation lanes green   |
| 7   | `browser_click` and `browser_type`                          | New typed input tools                        | Trust/focus/leak oracles green        |
| 8   | Route legacy `page` through BrowserEngine                   | No intentional behavior change               | Compatibility suite green             |
| 9   | Docs, skills, release matrix, and deprecation notice        | Public rollout                               | Exact-SHA evidence linked             |

Do not merge PR 4 merely to allow later CI to run unless the state tool's own acceptance gate is complete. Use test-only probes or feature-gated internal code for the feasibility stage.

## Validation Strategy

### Unit and protocol tests

Run automatically on affected OS paths:

- browser and engine classification;
- endpoint parsing and loopback enforcement;
- PID and process-tree endpoint ownership;
- CDP target selection and ambiguity;
- browser-window correlation and confidence scoring;
- flat-mode command routing and response correlation;
- session isolation and cleanup;
- tab and binding invalidation;
- page-ref creation, namespace rejection, staleness, TTL, and bounds;
- structured refusal schema;
- MCP annotations and capability tokens;
- platform-specific tool schema parity;
- legacy `page` compatibility.

### Deterministic repository-owned E2E

Extend the existing harnesses without weakening their oracles:

| Host              | Platforms                                | Required browser-tool coverage                                                                       |
| ----------------- | ---------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Electron fixture  | macOS, Windows, Linux X11, Linux Wayland | Discovery, exact single-window binding, refs, navigate, click, type, stale refs, concurrent sessions |
| Tauri fixture     | macOS, Windows, Linux X11, Linux Wayland | Capability reporting, exact route where available, native fallback boundaries                        |
| WebView2 fixture  | Windows                                  | CDP binding, refs, trusted input, navigation                                                         |
| WKWebView fixture | macOS                                    | Identity, capability limitations, native-state handoff                                               |
| WebKitGTK fixture | Linux                                    | Identity, capability limitations, native-state handoff                                               |

Repository fixtures remain the canonical deterministic behavior matrix. They do not replace real-browser evidence.

### Real-browser release matrix

| Platform              | Required v1 browser evidence                                                                  |
| --------------------- | --------------------------------------------------------------------------------------------- |
| Windows interactive   | Chrome and Edge; Firefox structured-unavailable/native fallback                               |
| macOS authorized host | Chrome; Safari state and limitation reporting; Firefox structured-unavailable/native fallback |
| Linux X11             | Chromium/Chrome; Firefox structured-unavailable/native fallback                               |
| Linux Wayland Sway    | Native Ozone Chromium with exact-or-refused binding                                           |
| Linux Wayland GNOME   | Chromium route and compositor limitation evidence                                             |
| Linux Wayland KDE     | Best-effort discovery/binding evidence before claiming support                                |

Every real-browser lane records:

- exact commit SHA and driver build identity;
- OS, browser, browser version, window system, and profile kind;
- route and binding confidence;
- before/after browser state;
- native focus and foreground sentinel;
- leaked-input oracle;
- tool trace and structured results;
- screenshots and video where the platform can capture them safely;
- explicit expected refusals.

### Required adversarial scenarios

1. Multiple browser windows owned by one PID.
2. Multiple tabs with colliding titles and URL substrings.
3. Same-size and same-position windows.
4. Tab drag between windows after binding.
5. New popup window and opener relationship.
6. Permission prompt and native file picker handoff.
7. Browser restart and stale target reuse.
8. Closed tab and reused browser target metadata.
9. Fake local debugging endpoint.
10. Two Cua sessions acting on different tabs through one endpoint.
11. Concurrent read and mutation requests.
12. Authenticated profile consent accepted and declined.
13. Background target fully occluded by the sentinel.
14. Wayland target without stable geometry.
15. Legacy `page` call and new browser call against the same browser process.
16. Same-origin iframe and cross-origin out-of-process iframe refs, including a structured limitation where the route is not implemented.

## CI Policy

### Automatic PR checks

- shared Rust unit tests;
- core browser protocol tests;
- platform-specific compile and schema tests only when relevant paths change;
- deterministic noninteractive tests that do not require browser consent or a desktop session;
- documentation example/schema checks.

### Maintainer-dispatched interactive checks

- Windows real-browser matrix on an interactive runner;
- Linux X11 and Wayland browser matrices;
- macOS local authorized-host matrix;
- authenticated-profile and consent scenarios;
- full video/artifact collection.

### Release gate

A release that advertises the browser tool requires:

- fresh exact-SHA accepted evidence for every advertised route;
- no unexplained heuristic mutation;
- no unresolved wrong-target, endpoint-owner, or cross-session failures;
- public capability docs generated from the shipping platform binaries;
- compatibility evidence for the legacy `page` facade.

## Migration From `page`

### Compatibility policy

- `page` remains functional throughout v1.
- Existing action names and schemas remain accepted.
- Legacy first-page behavior remains only in the compatibility facade.
- New browser tools never use first-page fallback.
- `page` gains optional `session` and `browser_target_id` fields additively.
- `page` responses gain route metadata without removing existing text.
- Deprecation documentation begins only after browser tools reach accepted parity.
- Removal is not considered for at least two releases and requires usage evidence plus a separate decision.

### Documentation transition

During coexistence, docs should say:

- use browser tools for new agentic browsing integrations;
- use `get_window_state` for browser chrome and native dialogs;
- use `page` only for compatibility with existing clients;
- inspect route and trust metadata before relying on background semantics.

## Issue Mapping

The implementation should coordinate with these existing issues:

| Issue                                              | Relationship to this plan                                            |
| -------------------------------------------------- | -------------------------------------------------------------------- |
| [#2200](https://github.com/trycua/cua/issues/2200) | Exact PID/window targeting and ambiguous-target refusal              |
| [#2176](https://github.com/trycua/cua/issues/2176) | Browser/page escalation before native foreground fallback            |
| [#2192](https://github.com/trycua/cua/issues/2192) | Authenticated Chrome profile preparation and consent                 |
| [#2084](https://github.com/trycua/cua/issues/2084) | Shared CDP typing and Windows/Linux parity                           |
| [#2201](https://github.com/trycua/cua/issues/2201) | Windows Chrome background native input remains a separate route risk |
| [#2202](https://github.com/trycua/cua/issues/2202) | macOS Chrome native drag remains separate from browser DOM/CDP input |
| [#1616](https://github.com/trycua/cua/issues/1616) | Real Chromium accessibility exposure and native fallback             |
| [#2101](https://github.com/trycua/cua/issues/2101) | Firefox capability and minimized/native-state limitations            |
| [#2194](https://github.com/trycua/cua/issues/2194) | Wayland cursor-preservation evidence remains platform-limited        |

Create one browser-tool umbrella issue before implementation, then link each PR and any newly discovered platform bug to the relevant capability row rather than treating all browser work as one undifferentiated feature.

## Documentation Deliverables

Use the Diataxis structure for public documentation:

### Tutorial

- control a real browser from discovery through verified page action;

### How-to guides

- use an isolated Chromium profile;
- connect to an authenticated existing profile with explicit consent;
- handle `requires_setup`;
- move between browser page state and native browser chrome;
- debug a stale or ambiguous binding;
- run the browser validation harnesses.

### Reference

- browser tool schemas;
- binding confidence and refusal codes;
- platform/browser capability table;
- input trust classes;
- session, target, tab, and ref lifetimes;
- CI and release evidence ledger.

### Explanation

- why Cua separates native window state from browser page state;
- why browser preparation cannot happen inside a read-only getter;
- how exact targeting and CDP endpoint ownership work;
- why some Wayland, Safari, Firefox, and embedded-webview routes are intentionally unavailable.

Bundled skills must derive their examples from the same platform-specific schemas and must not claim unsupported actions.

## Telemetry and Diagnostics

Add privacy-preserving counters and structured diagnostics for:

- browser classification success/failure;
- route availability;
- setup required, accepted, declined, and failed;
- binding method and confidence;
- wrong-target and owner-mismatch refusals;
- stale tab, target, and ref refusals;
- requested and actual input trust;
- route fallback requests and refusals;
- action verification outcome;
- endpoint reuse and consent-prompt count;
- session cleanup and leaked-target detection.

Do not record URLs, page text, selectors, cookies, profile paths, auth state, endpoint tokens, or JavaScript payloads in aggregate telemetry.

`doctor` should report browser support readiness without mutating state:

- recognized browsers;
- whether a safe endpoint is already available;
- whether setup is required;
- platform permission prerequisites;
- source/release build identity;
- Linux window system and required feature flags;
- Windows interactive-session status.

## Risks and Mitigations

| Risk                                        | Mitigation                                                                      |
| ------------------------------------------- | ------------------------------------------------------------------------------- |
| Wrong native window mapped to a tab         | Owner proof, exact correlation, per-call revalidation, ambiguity refusal        |
| Consent prompt triggered by read-only state | Hard separation between state and prepare; reuse approved sockets               |
| Cross-session CDP delivery                  | Flat mode and one CDP session ID per Cua session/tab                            |
| Browser tab moved after binding             | Re-check browser window relationship before mutation                            |
| DOM click mistaken for trusted input        | Required `input_trust` result and explicit caller choice                        |
| Duplicate native/browser context            | Browser state excludes native tree and screenshot by default                    |
| Wayland missing identity/geometry           | Confidence `none`, structured refusal, native semantic fallback only            |
| Existing profile compromised                | Explicit consent, loopback owner proof, TTL, no target replay, secret-safe logs |
| Public API outruns evidence                 | Chromium-only v1 mutation and exact-SHA release matrix                          |
| `page` migration regresses users            | Compatibility facade, retained tests, multi-release deprecation                 |
| Tool surface overwhelms MCP context         | Five typed v1 tools, compact refs, bounded output, granular registration        |

## Open Product Decisions

These decisions are intentionally deferred until the feasibility and state phases produce evidence:

1. Whether Safari should gain deeper Apple Events tab enumeration or a WebDriver route.
2. Whether Firefox demand justifies a WebDriver BiDi engine.
3. Whether v1 should ship trusted CDP pointer input immediately or initially expose read/navigation plus explicit DOM input.
4. Whether browser tab open, close, and activate belong in v1.1 or later.
5. Whether a CDP-event-backed `browser_wait` is valuable enough to become a separate typed tool.
6. Whether `state_revision` should exist only for event-backed targets or remain omitted in favor of snapshot staleness.
7. Whether AX-only embedded surfaces should ever expose browser-prefixed refs.
8. Whether domain/content policy belongs in Cua Driver, the MCP host, or a separate policy layer.
9. The exact `page` deprecation timeline after adoption data exists.

## Definition of Done

The browser tool project is complete when:

- the feasibility spike passes or the design has pivoted according to the kill criterion;
- every supported endpoint is owner-verified and loopback-bound;
- browser targets, tabs, and refs are exact, session-scoped, expiring capabilities;
- `get_browser_state` is demonstrably side-effect-free;
- preparation, consent, restart, and profile operations are explicit and recorded;
- typed navigation and input tools report exact routes and trust classes;
- ambiguous and stale targets refuse without acting;
- concurrent sessions cannot cross-deliver browser commands;
- native browser chrome and dialogs remain cleanly handled through `get_window_state`;
- deterministic fixture and real-browser matrices are accepted on advertised platforms;
- legacy `page` behavior remains compatible through the shared engine;
- public docs, bundled skills, schemas, diagnostics, and release evidence agree;
- no known wrong-target or silent-success browser issue remains in an advertised route.

## First Review Decision

Before implementation begins, reviewers should approve or reject these four points:

1. `get_browser_state` is strictly read-only, with all setup delegated to `browser_prepare`.
2. Browser mutation requires an exact binding; heuristic binding is informational only.
3. V1 exposes five typed tools rather than a multiplexed browser command surface.
4. The Phase 0 kill criterion is binding: failure to prove exact-or-refused Chromium correlation causes a tab-first design pivot.

Once those are approved, Phase 0 is the only authorized implementation work. The remaining phases are contingent on its evidence.
