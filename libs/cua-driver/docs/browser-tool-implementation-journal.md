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

## Validation log

Validation entries will include the exact command, source revision, result, and
the behavioral claim the result supports. A route is not described as supported
until its platform harness or a representative real-browser lane passes.

## Deferred by design

- Legacy `page` facade migration.
- Firefox and Safari mutation through this CDP-first API.
- Cross-origin iframe references and browser tab lifecycle tools.
- Mutation on a compositor that cannot prove the requested native window maps
  to the selected browser window.
