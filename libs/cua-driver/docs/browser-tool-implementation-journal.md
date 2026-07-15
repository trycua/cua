# Browser Tool v1 Implementation Journal

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
  snapshots the page, performs a trusted ref click, verifies the external
  fixture journal, snapshots again, types through the new ref, and proves the
  older ref returns `browser_ref_stale` without changing state.
- The required evidence is fixture state, foreground focus, z-order, no leaked
  input, cursor preservation where observable, and a playable video.
- Fixed the native Sway preflight rather than bypassing it. Manual VM runs now
  export the selected Wayland session, temporarily clear the fullscreen
  sentinel for the deliberate focus-loss canary, prove focus can move, and
  restore the sentinel before product rows execute. Unit coverage distinguishes
  a view's own fullscreen state from Sway's inherited workspace metadata.

## Validation log

### Shared and macOS unit/protocol validation

- `cargo test -p cua-driver-core -p platform-macos`: 168 core tests, 3 session
  lifecycle tests, and 123 platform-macOS tests passed.
- `cargo test -p cua-driver --test protocol_schema_test --test
schema_consistency_test --test protocol_session_test`: 1 schema, 1
  consistency, and 6 session protocol tests passed.
- `cargo build -p cua-driver` and the release build used by the documentation
  generator passed.
- The installed app currently reports Accessibility and Screen Recording as
  not granted after its local signing identity changed. The final macOS harness
  row therefore remains permission-gated until the app is reauthorized; unit
  and protocol results are not presented as a substitute for that E2E row.

### Windows interactive validation

- Source revision: `650ad376471fd423803d95e10271d1539ea84352`.
- `cargo check -p platform-windows -p cua-driver --tests` passed in an
  interactive Windows desktop.
- The canonical focused Electron browser row passed: 1 delivered, 0 refused,
  0 failed, 0 skipped. Fixture state, focus, z-order, cursor, no-leaked-input,
  and video evidence all passed.
- Later commits touch only Linux/Sway test setup and documentation, so they do
  not change the validated Windows product route.

### Linux X11 validation

- Final implementation revision: `7744993829ee8eeb28da733d536dd74949d8a7fd`.
- `cargo check -p platform-linux -p cua-driver --tests` passed.
- All 53 Linux testkit tests passed.
- The canonical focused Electron row passed under X11/Openbox: 1 delivered, 0
  refused, 0 failed, 0 skipped, with fixture-state, focus, z-order, cursor,
  no-leaked-input, and playable-video evidence.

### Linux native Wayland validation

- Final implementation revision: `7744993829ee8eeb28da733d536dd74949d8a7fd`.
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

## Deferred by design

- Legacy `page` facade migration.
- Firefox and Safari mutation through this CDP-first API.
- Cross-origin iframe references and browser tab lifecycle tools.
- Mutation on a compositor that cannot prove the requested native window maps
  to the selected browser window.
- Profile creation, browser restart, and preference changes inside
  `browser_prepare`; callers currently use explicit `launch_app` arguments.
- Full Chrome/Edge multi-window adversarial release lanes and Tauri, WebView2,
  WKWebView, and WebKitGTK browser-tool rows. Existing native harness coverage
  for those surfaces remains unchanged.
