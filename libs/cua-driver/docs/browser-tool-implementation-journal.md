# Browser Tool Implementation Journal

This journal records implementation and validation evidence for the first-class
browser tools. It intentionally omits machine identities, credentials, private
URLs, profile paths, and raw DevTools endpoint identifiers.

## Definition of done

- Add a capability-aware browser API with five typed tools:
  `get_browser_state`, `browser_prepare`, `browser_navigate`, `browser_click`,
  and `browser_type`.
- Keep `get_browser_state` read-only and make setup explicit.
- Bind native `(pid, window_id)` targets to browser tabs exactly or refuse all
  mutation. Never promote a heuristic match into an action route.
- Scope browser targets, tabs, and page references to a named driver session.
- Validate with unit and protocol tests, source-built web harnesses, macOS
  locally, Windows in an interactive desktop, and Linux on the strongest
  representative X11 and Wayland lanes available.
- Keep the existing `page` tool compatible until the new browser routes have
  sufficient release evidence.

## 2026-07-14: architecture and base

- Started from `origin/main` at `4e9b26ed3077089c1812a62909b82ac5c5ae2150`
  on local branch `codex/browser-tool-v1`.
- Confirmed the implementation split follows the existing `PageBackend`
  inversion: shared contracts and CDP behavior in `cua-driver-core`; process,
  endpoint, native-window, and setup evidence in each platform crate.
- Accepted an exact-or-refused correlation route based on native ownership and
  bounds plus CDP `Browser.getWindowForTarget` and `Browser.getWindowBounds`.
- Kept two scope controls for v1: do not rewire the legacy `page` facade, and
  keep `browser_prepare` to explicit endpoint attachment or a minimal
  driver-owned debug launch.
- Platform evidence found in the existing source:
  - macOS: exact CGWindow owner, title, and bounds.
  - Windows: exact HWND owner plus DWM bounds.
  - Linux X11: geometry and client-declared `_NET_WM_PID`; require independent
    agreement with endpoint ownership before treating it as exact.
  - Sway: compositor-provided pid and rectangle.
  - Generic GNOME/KDE Wayland: no general exact pid-plus-geometry correlation;
    report read-only capability and refuse mutation.

## 2026-07-14: shared engine and security contract

- Added the five typed tools in `cua-driver-core` and registered the same
  schemas on every platform.
- Added a shared CDP transport, browser target store, structured refusal
  vocabulary, session cleanup, snapshot-scoped refs, and mutation
  serialization.
- Restricted endpoint discovery to loopback listeners whose operating-system
  owner matches the requested process. Browser websocket URLs and raw CDP
  target ids remain internal.
- Added process fingerprints and native-window ownership checks so a browser
  restart, pid reuse, closed window, moved tab, or endpoint change fails closed
  during mutation revalidation.
- Kept `get_browser_state` strictly read-only. `browser_prepare` detects an
  existing owned endpoint or returns an explicit setup/consent limitation; it
  does not silently relaunch or modify a profile.
- Added `cdp_debugging_port` launch support on Windows and Linux, matching the
  existing macOS launch surface.
- Made the public `session` argument authoritative while retaining the daemon's
  hidden session mirror for MCP transport compatibility.

## 2026-07-14: exact embedded-browser binding

- Real Electron 39 returned CDP method `-32601` for
  `Browser.getWindowForTarget`. Rather than guessing, added a bounded
  `embedded_single_page` route.
- The fallback is exact only while the endpoint exposes one page and the
  requested process owns one native window. Both facts are rechecked before
  every mutation. A second page or native window causes a structured refusal.
- Standard Chromium uses the stronger `native_cdp_window` route based on the
  browser window id and native/CDP geometry agreement.

## 2026-07-14: canonical harness coverage

- Added `<platform>-electron-browser-tool-roundtrip` to the established shared
  web behavior matrix instead of creating a separate test framework.
- The row starts a named session, binds the exact native Electron window,
  proves `browser_prepare` recognizes the owned endpoint, asserts the bounded
  `embedded_single_page` route, snapshots the page, performs a trusted ref
  click, verifies the external fixture journal, snapshots again, types through
  the new editable ref, and proves the older ref returns `browser_ref_stale`
  without changing state. It then navigates to `about:blank`, verifies the new
  URL, and proves navigation invalidates the latest page ref.
- The required evidence is fixture state, foreground focus, z-order, no leaked
  input, cursor preservation where observable, and a playable video.
- Fixed the native Sway preflight rather than bypassing it. Manual VM runs now
  export the selected Wayland session, temporarily clear the fullscreen
  sentinel for the deliberate focus-loss canary, prove focus can move, and
  restore the sentinel before product rows execute. Unit coverage distinguishes
  a view's own fullscreen state from Sway's inherited workspace metadata.

## 2026-07-15: independent review hardening

- An independent read-only implementation review found two trust-chain
  blockers: a verified HTTP listener could return a WebSocket URL on another
  loopback port, and two maximized browser windows with identical bounds could
  be title-tie-broken across different CDP window ids.
- Every platform adapter now requires the returned WebSocket URL to use the
  exact operating-system-attested listener port. Regression tests also pin
  loopback-only parsing and token-based browser product classification.
- Bounds tie-breaking is now legal only within one proven CDP window id.
  Distinct same-bounds windows remain ambiguous, and geometry without a CDP
  window id cannot mint an exact binding.
- Transient or malformed CDP window-enumeration responses now fail the whole
  proof instead of shrinking the candidate set into a false unique match.
- `browser_type` now requires a current ref and proves that the focused node is
  editable before dispatch. It refuses instead of reporting success against
  unproven page focus.
- A second, narrow read-only review traced all six remediations through the
  shared engine and all three adapters and found no remaining completion
  blocker. Standalone multi-window Chrome/Edge remains a documented release
  evidence gap rather than an advertised v1 route.

## Validation log

### Shared and macOS unit/protocol validation

- `cargo test -p cua-driver-core -p platform-macos`: 170 core tests, 3 session
  lifecycle tests, and 126 platform-macOS tests passed at the final revision.
- `cargo test -p cua-driver --test protocol_schema_test --test
schema_consistency_test --test protocol_session_test`: 1 schema, 1
  consistency, and 6 session protocol tests passed.
- `cargo build -p cua-driver` and the release build used by the documentation
  generator passed.
- The initial permission-enabled macOS run failed closed because WindowServer
  reported several untitled layer-0 helper surfaces for Electron. Those
  process-owned helpers incorrectly defeated the single-native-browser-window
  proof even though CDP exposed exactly one page.
- Revision `0cf2dd9e1372a3b2c11822975752ddd04fe0b931` narrows the macOS
  fallback cardinality check to titled, non-empty browser surfaces. A second
  titled surface still defeats the proof, and a second CDP page independently
  defeats it in shared core. Both shapes have focused regression tests.
- That exact revision was installed through `install-local.sh` using the stable
  local signing identity. The daemon reported Accessibility, Screen Recording,
  and live capture capability as granted after restart.
- The canonical macOS Electron browser row then passed: 1 delivered, 0
  refused, 0 failed, 0 skipped. It exercised Page/Background/Window/CDP with
  fixture-state, focus, z-order, cursor, and no-leaked-input oracles, plus a
  playable video and trajectory. The generated environment record identifies
  the exact source revision above.

### Windows interactive validation

- Source revision: `a2e792a428308a80bb02a37ed4467ea073a8e8e5`.
- The three focused Windows browser-platform parser, classifier, and endpoint
  attestation tests passed.
- The canonical focused Electron browser row passed: 1 delivered, 0 refused,
  0 failed, 0 skipped. Fixture state, focus, z-order, cursor, no-leaked-input,
  and playable-video evidence all passed in an interactive user desktop.

### Linux X11 validation

- Final implementation revision: `a2e792a428308a80bb02a37ed4467ea073a8e8e5`.
- The three focused Linux browser-platform parser, classifier, and endpoint
  attestation tests passed. Earlier full validation also passed platform checks
  and all 53 Linux testkit tests.
- The canonical focused Electron row passed under X11/Openbox: 1 delivered, 0
  refused, 0 failed, 0 skipped, with fixture-state, focus, z-order, cursor,
  no-leaked-input, and playable-video evidence.

### Linux native Wayland validation

- Final implementation revision: `a2e792a428308a80bb02a37ed4467ea073a8e8e5`.
- Six focused Sway sentinel regression tests passed.
- The native Sway preflight passed its deliberate focus-loss and fullscreen
  restoration canary.
- The canonical focused Electron row passed under native Ozone Wayland with
  Xwayland disabled: 1 delivered, 0 refused, 0 failed, 0 skipped. Fixture
  state, focus, z-order, no-leaked-input, and playable-video evidence passed;
  global cursor preservation is omitted because standard Wayland cannot read
  the global pointer position.

## 2026-07-14: documentation and agent surface

- Generated MCP reference directly from the compiled Rust tool schemas.
- Added a Diataxis how-to for the session/bind/snapshot/action workflow and a
  separate explanation of exact browser targeting and full-background CDP
  delivery.
- Updated current limits and the bundled `cua-driver` skill. The legacy `page`
  tool remains available and is not presented as the preferred new mutation
  route.

## Initial v1 milestone audit

| Plan phase | Browser-tool v1 result |
| --- | --- |
| Phase 0: binding feasibility | Accepted. Native Chromium uses exact CDP-window geometry; Electron uses the separately bounded single-page/single-native-window proof. Ambiguous targets refuse. |
| Phase 1: shared CDP foundation | Implemented in `cua-driver-core`, with loopback listener ownership and exact WebSocket-port attestation in all three platform adapters. |
| Phase 2: session-owned targets | Implemented. Target, tab, snapshot, and ref capabilities are opaque and session-scoped; session teardown removes the namespace. |
| Phase 3: read-only state | Implemented. Bind and snapshot modes share `get_browser_state`; neither performs setup or mutation. |
| Phase 4: preparation | Intentionally narrow. Existing owned endpoints are recognized; setup, restart, and profile changes remain explicit structured refusals rather than hidden side effects. |
| Phase 5: typed mutations | Implemented for navigation, trusted or explicitly synthetic click, and ref-bound editable typing, with mutation-time revalidation. |
| Phase 6: legacy `page` migration | Deferred. The existing facade remains compatible and separate until adoption evidence supports migration. |
| Phase 7: release evidence and rollout | Partially complete. Canonical Electron evidence, generated reference, public Diataxis docs, and bundled skills are present. Standalone Chrome/Edge adversarial lanes and embedded-webview expansion remain deferred. |

The v1 milestone therefore satisfies the accepted local implementation scope,
including canonical macOS, Windows, X11, and native Wayland evidence. It does
not claim the later `page` migration or the broader real-browser release
matrix.

## Deferred at the v1 milestone

The v2 entries below supersede the completed items in this historical list.

- Legacy `page` facade migration.
- Firefox and Safari mutation through this CDP-first API.
- Cross-origin iframe references and browser tab lifecycle tools.
- Mutation on a compositor that cannot prove the requested native window maps
  to the selected browser window.
- Isolated profile creation and acting `browser_prepare` launch.
- Full Chrome/Edge multi-window adversarial release lanes and Tauri, WebView2,
  WKWebView, and WebKitGTK browser-tool rows. Existing native harness coverage
  for those surfaces remains unchanged.

## 2026-07-15: v2 truthfulness and standalone Chromium coverage

- `26c177d9` made browser-engine classification and route limitations
  consistent across platforms. Unsupported engines remain discoverable but do
  not advertise mutation.
- `0295b881` added repository-owned standalone Chromium harness coverage.
- `3287b44e` added adversarial multi-tab and same-bounds multi-window cases.
  The driver refuses ambiguity instead of selecting the first page or using a
  title tie-break across different CDP window ids.
- The complete macOS standalone suite passed in one uninterrupted run: exact
  roundtrip, isolated preparation, stale refs, composed frames, multi-tab, and
  same-bounds multi-window refusal.

## 2026-07-15: composed documents and event-aware CDP

- `c8fb38ef` added refs through open shadow roots and same-process iframes.
  Out-of-process iframes are attached only when the runtime exposes a proven
  CDP session; event messages are demultiplexed from command responses.
- Snapshot refs retain frame identity. A missing frame route is reported as a
  limitation and never flattened into the main document.
- The standalone composed-document row passed against real Chromium, including
  shadow DOM, same-process iframe, and capability-tested OOPIF behavior.

## 2026-07-15: approved isolated browser preparation

- `ca31f017` removed public consent/restart fields and marked
  `browser_prepare` destructive and non-idempotent.
- MCP calls require a live host-mediated approval marker. Direct CLI/raw calls
  require a five-minute, single-use token minted by the interactive
  `browser-approve` command and bound to pid plus profile request.
- Acting prepare launches a separate Chromium process with a driver-owned
  `isolated_new` or `isolated_named` profile. It never copies, modifies,
  restarts, or terminates the selected user profile.
- The endpoint is proved from the private profile's `DevToolsActivePort` and
  socket ownership. Temporary profiles and their processes are reaped when the
  owning session ends.
- Remote-debugging arguments passed through `launch_app` are rejected on all
  platforms so setup cannot bypass this boundary.

## 2026-07-15: embedded routes and legacy compatibility

- `1c4e3539` separated exact browser-route evidence from the broad shared
  action matrix. Electron proves an exact CDP mutation roundtrip on each
  supported OS. Tauri, WKWebView, WebKitGTK, and the common split-process
  WebView2 shape prove side-effect-free `browser_route_unavailable` refusals
  until their engine/native-host relationship can be bound exactly.
- `5b484098` migrated the legacy `page` CDP path to the shared pooled,
  event-aware transport without changing its first-page/URL-hint semantics,
  output format, timeouts, or AppleScript/UIA/AT-SPI fallbacks.
- A compatibility test injects an unsolicited CDP event before an evaluate
  reply and proves the legacy result still arrives in its historical format.
- The macOS embedded matrix passed against the locally installed source build:
  Electron mutation delivered; Tauri and WKWebView refused with focus, z-order,
  cursor, no-leaked-input, and fixture-state sentinels intact.

## 2026-07-15: harness process isolation

- `a6e4c6b8` places every Unix test-owned browser in its own process group and
  reaps the complete tree after each row. This removed late Chrome children
  that could contaminate the next exact-binding test.
- No product action is retried. A longer readiness budget and a macOS launch
  settle occur only between independently owned harness processes.
- `cua-driver-testkit` passed 41 tests. The full six-row standalone Chromium
  suite then passed in 66 seconds with no lingering harness browser process.

## Current explicit limitations

- Safari/WKWebView/WebKitGTK typed mutation is deferred until an exact WebKit
  engine-to-native-window route exists. Legacy/native reads remain available.
- Firefox is classified but has no WebDriver BiDi engine or typed mutation.
- Generic Wayland without compositor-provided exact pid and geometry cannot
  authorize browser mutation. Validated X11 and Sway configurations may do so;
  this does not imply arbitrary raw background PX delivery.
- Common WebView2 hosts split the native WPF window and Edge renderer across
  processes. The typed route refuses until that correlation can be proved.
- `browser_prepare` deliberately does not attach DevTools to, copy, or restart
  a person's existing profile. Authenticated-profile automation remains a
  product/security decision rather than an implicit setup shortcut.
- MCP approval proves that the request traversed the host approval path; it is
  not a same-user operating-system security boundary against another local
  process. Direct approval tokens remain terminal-only, short-lived,
  single-use, and request-bound.

## 2026-07-15: final v2 hardening and exact-head release evidence

- `ee8b75f3` split the standalone browser matrix into independent scenario
  processes. A failed launch or contaminated desktop can no longer prevent the
  remaining declared rows from producing evidence.
- `083859f3`, `5f3fca7a`, and `3336b343` made temporary Chromium startup and
  teardown deterministic. Every harness browser receives a clean profile and,
  on Unix, an owned process group whose descendants are reaped when the row
  ends.
- `33c8d533` made the harness route explicit in every result instead of
  inferring it from the expected outcome.
- `c70f5986` creates additional tabs and same-bounds windows through the
  harness-only CDP connection. This removes command-line handoff ambiguity
  while leaving the public driver path unchanged. It also lets macOS E2E use a
  dedicated daemon socket so another driver client cannot end the test
  session.
- `edc26080` made the loopback fixture server concurrent. Chromium idle
  preconnections can no longer hold the only server worker and starve the real
  page or journal request. A regression test opens an idle connection while a
  second client completes a fixture request.
- `418fb972` made the reporter deduplicate identical environment records from
  independently executed rows while continuing to reject conflicting records.
- `4fc4e00b` fixed a critical Unix harness teardown bug. Passing `-1` to the
  external `kill` utility had been parsed as the broad `kill(-1, SIGKILL)`
  syscall, which could terminate every process signalable by the test user.
  Teardown now calls guarded `libc::kill(-pgid, SIGKILL)` directly and refuses
  process ids 0, 1, or values outside the platform range.

### Exact standalone Chromium evidence

- Windows interactive evidence at `edc2608061360dadc258724045438922487753e2`:
  16 declared Chrome and Edge rows, 14 delivered, 2 exact
  `browser_binding_ambiguous` refusals, 0 failed, and 0 skipped. The matrix
  covers background typing, composed frames and capability-tested OOPIFs,
  multi-tab exact binding, isolated preparation, stale refs, full roundtrip,
  trusted background click, and same-bounds ambiguity. Every row includes a
  fixture-state oracle, focus and z-order guards, no-leaked-input and cursor
  checks, and a playable video.
- Linux X11 evidence at `4fc4e00b0630f5889e17e91ebe31b3508dc3818f`:
  8 declared Chrome rows, 6 delivered, 2 refused, 0 failed, and 0 skipped.
  Trusted pointer input refused with
  `browser_input_trust_unavailable` before dispatch because standalone
  Chromium activates its native window on that route. Same-bounds ambiguity
  refused with `browser_binding_ambiguous`. Explicit DOM click, ref-bound
  typing, navigation, composed frames, multi-tab binding, stale refs, and
  isolated preparation passed with the complete background oracle set and
  videos.
- macOS standalone rows passed through a dedicated source-installed daemon
  before final documentation. A later replacement of the local app invalidated
  its TCC grants; runs after that point are environment-inconclusive because
  WindowServer redacts browser titles without Screen Recording permission.
  Final exact-head macOS release evidence must therefore be regenerated after
  the installed app again reports Accessibility, Screen Recording, and live
  capture as granted. Shared-host focus changes are not attributed to browser
  mutation unless they reproduce in that permission-complete, uncontended
  setup.

### Embedded route evidence retained

- macOS: Electron delivered; Tauri and WKWebView returned the expected
  side-effect-free route refusal.
- Windows: Electron delivered; Tauri and the split-process WebView2 host
  returned the expected route refusal.
- Linux X11: Electron delivered; Tauri returned the expected route refusal.
- Native Sway: Electron delivered and Tauri refused. Global cursor preservation
  remains omitted because standard Wayland cannot read the global pointer
  position.

### Final trust model

- Standalone Chrome and Edge on Windows have passing trusted background pointer
  evidence.
- Standalone Chromium on macOS and Linux refuses trusted CDP pointer input
  before dispatch because that route activates the native browser window.
- `input_route: "dom_event"` is the explicit synthetic full-background click
  route. The driver never changes from trusted input to a DOM event silently.
- Electron's trusted route is a separately bounded proof and does not authorize
  a generic embedded-webview fallback.

## 2026-07-15: exact Windows and Linux browser release checkpoint

Revision `32c77f775bd16d50507fbedb54a9682b9ab5599d` passed the complete
browser-specific release matrix on the three available interactive cloud
desktops. Every declared result includes fixture-state and desktop-side-effect
oracles plus a playable video.

- Windows Win32, in a real RDP user session: 19 declared rows, 15 delivered,
  4 exact refusals, 0 failed, and 0 skipped. Chrome and Edge each passed all
  eight standalone scenarios. Electron delivered through its bounded CDP
  route; Tauri and split-process WebView2 refused with
  `browser_route_unavailable`.
- Linux X11/Openbox: 10 declared rows, 7 delivered, 3 exact refusals, 0
  failed, and 0 skipped. Standalone trusted pointer input refused before
  dispatch with `browser_input_trust_unavailable`; the explicit DOM event
  route remained available for synthetic full-background clicks. Electron
  delivered and Tauri refused exactly.
- Linux native Wayland/Sway with Xwayland disabled: 8 standalone rows, 6
  delivered, 2 exact refusals, 0 failed, and 0 skipped. The matrix proved
  isolated preparation, composed frames, exact multi-tab binding, stale refs,
  background typing, roundtrip mutation, trusted-input refusal, and
  same-bounds ambiguity refusal. A separate embedded run proved Electron
  delivery and Tauri's exact refusal. Global cursor preservation remains
  omitted because standard Wayland cannot read the global pointer position.

The broader native-Wayland shared-action diagnostic also found existing Tauri
WebKitGTK pointer and scroll failures. Those rows do not use the browser CDP
transport, and this branch does not change their injection paths. They remain
separate platform-action gaps and are not counted as browser-tool acceptance.

The reusable macOS VM work was merged before the final replay. The retained
Lume evidence below therefore validates the installed source in a disposable,
permission-complete guest rather than relying on host TCC state.

## 2026-07-16: final runner convergence

- Merged the reusable macOS Lume gate from main after preserving the browser
  branch with a backup ref. The browser branch remains the only source of
  commits; Windows, Linux, and macOS guests receive clean source snapshots with
  an exact SHA marker.
- The shared standalone runner now stages its own Electron focus sentinel,
  opts into native Wayland when the session requires it, discovers macOS
  browsers in both system and user Applications folders, and deduplicates
  executable aliases before declaring rows. Linux keeps a launcher path for
  execution while comparing its canonical target for deduplication, which is
  required for Snap-style browser launchers.
- Windows runs every standalone scenario in an independent process from the
  canonical PowerShell entry point. One failed row therefore cannot hide later
  Chrome or Edge evidence.
- The macOS Lume wrapper preserves previous standalone evidence before a repeat
  run. The Nix Wayland shell now supplies the Chromium binary used by the
  native Ozone lane.
- Sentinel setup waits for its final desktop geometry before recording the
  baseline. Dispatch-time focus, z-order, cursor, and leaked-input checks remain
  strict and are not retried.
- Standalone browser setup now relists windows until the public browser API can
  prove an exact binding. This handles a cold Sway toplevel that briefly has a
  protocol-local id before compositor pid/geometry metadata arrives. The driver
  continues to refuse the transient unprovable id; no mutation or behavior
  assertion is retried.

The cold native-Sway replay after that setup correction passed all 8 standalone
Chromium rows: 6 delivered, 2 expected refusals, 0 failed, and 0 skipped. Each
retained `environment.jsonl` is the authoritative source identity ledger; the
documentation-only completion commit is replayed separately so the final
branch identity is not inferred from predecessor evidence.

### Final browser acceptance matrix

| Environment | Standalone rows | Embedded rows | Accepted outcome |
| --- | ---: | ---: | --- |
| Windows interactive RDP | 16 | 3 | 19 passed: 15 delivered and 4 expected refusals; 19 playable videos |
| Linux X11 | 16 | 2 | 18 passed across Chrome, Snap Chromium, Electron, and Tauri; 18 playable videos |
| Linux native Wayland/Sway | 8 | 2 | 10 passed: 7 delivered and 3 expected refusals; 10 playable videos |
| macOS Lume | 8 | 3 | 11 browser rows passed; the complete canonical matrix also passed 148/148 with 149 playable videos including preflight |

The implementation candidate `19f1e09d55e2a595ccb17f1fcc48d65ceb5d181f`
produced those accepted counts with zero skipped rows. One earlier macOS
canonical attempt missed the Electron foreground drag while the fixture stayed
idle; a fresh, uninterrupted worker replay passed that row and all 147 others
without retrying any action inside a result cell. The final documentation-only
branch head is validated again after this record is committed.

Safari/WKWebView/WebKitGTK mutation, Firefox BiDi mutation, split-process
WebView2 mutation, and mutation on Wayland compositors without exact pid and
geometry remain deliberate structured limitations. Existing user profiles are
never copied, restarted, or attached implicitly by `browser_prepare`.

## 2026-07-16: macOS AX deadline correction

A fresh exact-head Lume replay invalidated the earlier macOS completion claim.
Electron completed all 40 canonical rows, but Tauri snapshots later exceeded
the testkit's 25-second public call deadline. A second clean replay reached the
same failure class at a different Tauri row, proving that this was not a single
action assertion or fixture expectation.

The driver had wrapped the native AX walk in a 30-second Tokio timeout. The
public client therefore timed out first, and dropping the `spawn_blocking`
future could not cancel an `AXUIElementCopyAttributeValue` call already blocked
in ApplicationServices. The AX walker now applies Apple's finite native
messaging timeout to the application and every descendant AX object. The
driver-level walk deadline is also 20 seconds, below the public client limit,
so an unresponsive application returns a structured driver error instead of a
generic client timeout and cannot retain a worker indefinitely.

All prior Windows, Linux, and macOS artifacts remain diagnostic only until this
correction is committed and every acceptance environment is replayed from that
exact source revision. No test oracle, expected outcome, or declared row was
weakened to accommodate the failure.

## 2026-07-16: macOS Tauri fixture navigation correction

The finite AX deadline exposed separate fixture defects instead of masking
them. The canonical build scripts compiled a release Tauri executable without
the production `custom-protocol` feature, so the native window opened but its
embedded frontend was not served. The shared Tauri feature now maps to
`tauri/custom-protocol`, and both Unix and Windows fixture builders enable it.

The Tauri package also did not declare its copied `../web` frontend directory
as a Cargo build input. A cached target could therefore retain an older page
after the shared fixture changed or a clean source snapshot was synced with
preserved mtimes. Its build script now emits `rerun-if-changed` for that
directory, making frontend embedding deterministic without deleting build
caches.

An official custom-protocol bundle exposed the second issue: starting the
first loopback fixture-state request before custom-scheme navigation finished
could keep the WKWebView load open. The shared web fixture now starts journal
publication from the window `load` event. It still publishes immediately after
that event and every 250 milliseconds thereafter, so the independent external
oracle is unchanged for actions. Electron, Tauri, WebView2, and WKWebView
continue to consume the same page; there is no macOS-only fixture or relaxed
readiness assertion. Exact-head cross-platform replay remains required before
release acceptance.

## 2026-07-16: Tauri loopback fixture and macOS VM diagnosis

Further replay showed that the production custom scheme was not a reliable
fixture transport on the current macOS WebKit runtime. Tauri completed the
custom-scheme and data-URL navigation callbacks, but the page remained empty.
The fixture now starts a server on an ephemeral loopback port, serves the same
checked-in `shared/web/index.html`, and creates its only webview against that
exact URL. It does not contact an external service or duplicate the shared
page. Cargo also watches `tauri.conf.json`, so a cached build cannot retain an
older window declaration.

The final fixture passed all 40 Tauri action cells on a physical macOS host,
covering foreground and background delivery across AX and PX addressing. Its
embedded browser row also returned the expected side-effect-free route
refusal. The independent fixture journal remained the action oracle.

The disposable macOS VM still renders both Tauri and the native WKWebView
fixture as an empty surface. System logs show WebKit finishing the page load
and then terminating an unresponsive GPU process. The same Tauri binary and
page pass on the physical host, so this result is an environment limitation in
that VM image rather than a browser-tool or fixture regression. The matrix
keeps its readiness assertion; it does not convert the blank surface into a
pass or an expected product refusal.

## 2026-07-16: existing-profile attachment and bounded reconnect

Implemented the reviewed existing-profile strategy without changing the
driver-owned isolated profile contract:

- `browser_prepare` accepts the tagged
  `strategy:{kind:"existing_profile"}` request only with an exact pid,
  native window, named session, and five-minute single-use artifact minted by
  the interactive `browser-approve` CLI. The ordinary MCP host marker cannot
  authorize this route.
- Grants live only in daemon memory and are scoped to public and transport
  session, process fingerprint, native window, browser product, endpoint, and
  connection generation. Idle/absolute expiry, consent dismissal, identity
  change, reconnect exhaustion, session end, and daemon restart revoke the
  grant and release its claimed socket.
- One browser-level CDP connection is owned per generation. Reconnect is
  single-flight per approved browser, has three attempts under one bounded
  deadline, re-proves endpoint/process identity, and invalidates every older
  target, tab, snapshot, frame, and ref capability.
- Browser mutations serialize by process fingerprint plus real CDP target, so
  two sessions cannot interleave actions in the same tab while independent
  tabs remain parallel. Keystroke typing reports exact requested and delivered
  character counts and returns `browser_input_incomplete` for a partial prefix.
- Consent-bearing recording turns persist redacted metadata and result only.
  Browser screenshots, AX snapshots, approval tokens, endpoint addresses,
  ports, profile data, and authenticated page content are suppressed.

The macOS Chrome adapter discovers a unique PID-owned loopback listener and
uses a bounded native AX adapter. It acts only on one browser-owned modal sheet
with remote-debugging corroboration and one semantic Allow button advertising
AXPress. During physical-host validation, Chrome exposed the same top-level
window through both AXChildren and AXWindows with different proxy pointers;
top-level deduplication now uses Core Foundation object equality so a single
sheet cannot become a false ambiguous match.

A fresh disposable Chrome profile provided the real acceptance evidence. The
user-facing remote-debugging toggle exposed one PID-owned listener; the exact
CLI artifact authorized one call; Cua Driver pressed Chrome's consent action;
`browser_prepare` returned `attached_existing_profile`; and the following
`get_browser_state(pid, window_id, session)` produced an exact
`native_cdp_window` binding without launching, restarting, copying, or changing
the profile. A separate disposable-profile standalone harness proves the same
public contract and a DOM mutation against an independent fixture oracle.

Final local verification passed 236 core tests, 134 macOS platform tests, 102
CLI tests, the protocol and session integration suites, and the ignored
source-built `standalone_browser_existing_profile` real-browser row. The shared
Windows/Linux/testkit packages compile with the additive contract. The exact
branch build was installed locally as Cua Driver 0.8.3; its stable app identity
retained both Accessibility and Screen Recording grants.

Windows and Linux compile the same contract, grant, generation, mutation, and
refusal model. Their native consent adapters remain strict refusals until an
interactive UIA or AT-SPI harness proves stable browser-owned prompt semantics;
no unsupported prompt route is advertised as working.

The committed source snapshot was also replayed natively on isolated remote VM
staging paths. The interactive Windows VM passed all 236 shared-core tests and
96 Windows platform tests, with the two pre-existing cache UAF repro cases
remaining ignored. The Wayland VM passed all 237 Linux shared-core tests and
all 100 Linux platform tests. These are native contract and platform-crate
results, not evidence of UIA or AT-SPI consent-dialog automation. An additional
X11 VM attempt reached dependency compilation but its root filesystem had only
4 MB free and returned `No space left on device`; it produced no behavioral
verdict and was not counted as product evidence.
