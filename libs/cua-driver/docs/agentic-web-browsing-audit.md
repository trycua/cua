# Agentic Web Browsing with Cua Driver

## Current-state and user-friction audit

- **Audit target:** `trycua/cua` at `f6aa3dfc24f376c793a0292f15847ee946104231`
- **Audit date:** 2026-07-14
- **Release context:** the latest tagged Rust driver is `cua-driver-rs-v0.7.1`; this audit targets newer `main` and calls out fixes that are not yet in that release.
- **Method:** static implementation, documentation, test, Git history, and all-issue audit. Interactive browser lanes were not rerun for this document.

## Executive summary

Cua Driver is already a capable native computer-use layer for browser windows. An agent can discover a browser, capture its window and accessibility tree, act through accessibility (AX) or pixels (PX), and verify the result without assuming the browser is foreground. The converged harnesses exercise these native paths extensively on Electron, Tauri, WebView2, WKWebView, and WebKitGTK.

The browser-specific `page` experience is much less mature. It exposes useful DOM, JavaScript, and CDP actions, but behavior and setup differ substantially by OS. A native target (`pid` plus `window_id`) does not identify a CDP tab, `query_dom` does not always query a real DOM, and Windows/Linux lack macOS's CDP input support. Agents must currently know which route is available before choosing an action.

The largest sources of user friction are:

1. **No first-class browser preparation or capability discovery.** The caller has to infer whether Apple Events, CDP, UIA/AX/AT-SPI, or PX will work and how to enable it.
2. **Ambiguous targeting across windows and tabs.** Native windows and CDP pages use separate identities. A URL substring is the only page disambiguator, and PID-only native actions can select the wrong window.
3. **Cross-platform `page` contract drift.** The schema advertises seven actions everywhere, but several return `not implemented` on Windows or Linux.
4. **Real browsers are under-tested.** The canonical suites prove embedded browser runtimes, not standalone Chrome, Edge, Safari, or Firefox behavior.
5. **Documentation and issue state lag `main`.** Some setup guidance is stale, generated MCP documentation is macOS-shaped, and several open issues are already fixed or materially changed on `main`.

The highest-leverage product direction is to make browser control capability-driven: prepare and bind an exact browser page, report the available routes, use a shared cached CDP implementation on every OS, and escalate from AX/PX to `page` before asking to foreground the browser.

## What agentic web browsing means today

Cua Driver is not a Playwright-style browser automation server. It controls the real desktop application and offers two complementary paths.

### Native window path

This is the dependable cross-platform loop:

1. Start a named Cua Driver session and pass the same `session` on every subsequent action.
2. Launch or discover the browser application.
3. Select an exact `pid` and `window_id`.
4. Call `get_window_state` to obtain a screenshot and accessibility elements when both routes succeed.
5. Prefer an `element_index` AX action in background mode.
6. Fall back to a window-relative PX action when no semantic element is available.
7. Resnapshot and verify page state, focus, and side effects.

The public action-selection guidance captures this order in [action-selection-policy.mdx](../../../docs/content/docs/reference/cua-driver/action-selection-policy.mdx). The implementation routes both CLI and MCP calls through the same platform tool registry.

This recovery loop currently has an important weakness: capture and accessibility traversal are coupled. macOS can return before capture when AX fails, a Windows UIA timeout can also lose the screenshot, and Linux can discard a valid AT-SPI result when capture fails. PX is therefore a fallback only when `get_window_state` actually returns a trustworthy image, not a guaranteed recovery path from accessibility failure.

Session behavior also depends on transport. Daemon/proxy paths mirror the public session into connection-owned state with TTL cleanup. Direct stdio MCP does not have the same connection boundary, and one-shot CLI fallback creates fresh state. `start_session` does not establish ambient identity: omitting `session` from a later action loses per-run cursor and isolation semantics.

### Page path

The multiplexed `page` tool adds browser-aware operations:

- `get_text`
- `query_dom`
- `execute_javascript`
- `click_element`
- `insert_text`
- `type_keystrokes`
- `enable_javascript_apple_events`

The schema is defined in [page.rs](../rust/crates/cua-driver-core/src/page.rs). The route may be a true DOM/CDP operation or an accessibility approximation depending on the OS and browser.

`page` actions do not currently accept `delivery_mode`. A successful CDP or in-page JavaScript action is normally focus-independent, but that is not the same contract as a native background AX/PX action. The caller still needs an exact browser target and must verify that no focus or input leaked.

`page.click_element` on macOS and Windows animates the agent cursor for evidence but ultimately invokes JavaScript `el.click()`. It does not produce trusted pointer events and cannot stand in for hover, drag, coordinate-sensitive behavior, or every transient user-activation flow.

### The targeting model

Today there are three related but separate identities:

| Identity | Used by | Lifetime and risk |
| --- | --- | --- |
| `pid` + `window_id` | Capture, accessibility, and native input | Identifies a desktop window, provided both values are validated together |
| `element_index` or element token | Accessibility actions | Snapshot-sensitive; should be refreshed after meaningful UI changes |
| CDP port + selected page | JavaScript and CDP input | Identifies a debugging endpoint and tab, not inherently the supplied native window |

The shared CDP helper enumerates `/json` and usually chooses the first page. `target_url_contains` can narrow targeted JavaScript and CDP text-input actions, but `get_text`, `query_dom`, and `click_element` cannot receive it. There is no stable `page_target_id`, tab enumeration tool, or proof that an explicit CDP port belongs to the supplied PID/window. This is the main wrong-target risk for multi-profile, multi-window, and concurrent-agent browsing.

## Cross-platform capability map

This table describes current `main`, not only the latest tagged release.

| Capability | macOS | Windows | Linux X11 and Wayland |
| --- | --- | --- | --- |
| Native state | AX tree plus window capture | UIA tree plus window capture | AT-SPI tree plus X11/Wayland capture routes |
| Native browser actions | AX and PX, foreground/background subject to surface | UIA/Win32 and PX, foreground/background subject to surface | AT-SPI and X11 or Wayland input, subject to toolkit/compositor |
| `page.get_text` | JavaScript first for supported browsers; AX fallback | UIA `TextPattern`/tree walk | AT-SPI tree extraction |
| `page.query_dom` | Real CSS query through JavaScript when available; AX approximation otherwise | UIA role mapping with limited selector/attribute support | AT-SPI role mapping for simple tags only |
| `page.execute_javascript` | Apple Events for Chrome-family/Safari; CDP for Electron or an explicit port | Bookmark/UIA route or manually configured CDP | Manually configured CDP |
| `page.click_element` | Implemented through JavaScript and a visual cursor pulse | Implemented through JavaScript and a visual cursor pulse | Not implemented |
| `page.insert_text` | CDP, with PID port discovery or explicit port | Not implemented | Not implemented |
| `page.type_keystrokes` | CDP, with PID port discovery or explicit port | Not implemented | Not implemented |
| CDP launch/setup | `cdp_debugging_port`, alternate profile, or user-approved Chrome remote debugging | Browser argument plus driver-level port configuration; `cdp_debugging_port` is currently a no-op | Caller must launch a debug-enabled browser and configure the driver port |
| Exact CDP tab binding | URL hint for targeted JS/CDP input; no hint for reads or click | URL hint for targeted JS; no hint for reads or click | URL hint for targeted JS; no hint for reads |

### macOS

macOS has the richest page implementation, but the route is browser-specific:

- Chrome, Brave, Edge, and Safari can execute JavaScript through Apple Events. This may require a browser setting, Automation consent, and a restart.
- Electron can use CDP with process-port discovery.
- WKWebView and Tauri have read-only AX fallback and no general JavaScript bridge.
- CDP text insertion and keystrokes require a non-default automation profile unless the user explicitly enables Chrome's remote-debugging flow and supplies its port.
- Safari title matching can fall back to `document 1`; Electron probes can encounter globally available ports. Both deserve stronger page-to-window binding.

Current real-browser failures include [#2202](https://github.com/trycua/cua/issues/2202), where foreground Chrome drag records pointer-down but does not complete the motion/drop. [#1489](https://github.com/trycua/cua/issues/1489) also tracks wrong-window capture in Safari and other multi-window applications.

### Windows

Windows provides useful read access without CDP, but setup and reliability are uneven:

- `get_text` and `query_dom` use UIA. `query_dom` maps common CSS-like selectors to UIA control types; it is not a full DOM query.
- JavaScript first tries a bookmark-based UIA workflow. It depends on a pre-existing `cua-driver-eval` favorite and may temporarily activate browser UI. Restoration is best effort.
- CDP requires a browser launched with `--remote-debugging-port=N` and the driver configured with the same port. `launch_app.cdp_debugging_port` is accepted but currently does nothing on Windows.
- CDP text insertion and keystroke actions are not implemented; see [#2084](https://github.com/trycua/cua/issues/2084).
- Real Chrome background click is not yet reliable and can miss or disturb the foreground sentinel; see [#2201](https://github.com/trycua/cua/issues/2201).
- UIA performance and blocking providers remain a practical agent timeout risk; see [#2100](https://github.com/trycua/cua/issues/2100), [#2110](https://github.com/trycua/cua/issues/2110), and [#2113](https://github.com/trycua/cua/issues/2113). Current `main` bounds `get_window_state`, but a timed-out blocking provider is not cancelled and should be revalidated on affected systems.
- Unsigned binaries and Smart App Control can prevent startup or UIAccess operation; see [#2066](https://github.com/trycua/cua/issues/2066) and [#2118](https://github.com/trycua/cua/issues/2118).

An interactive user desktop remains required. A healthy MCP process in Session 0 can otherwise expose an empty or unusable desktop.

### Linux X11

On X11, native browser discovery and accessibility reads work, but synthetic delivery varies by toolkit:

- `page.get_text` and the limited `query_dom` use AT-SPI.
- JavaScript requires a manually debug-enabled Chromium/Electron process and configured CDP port.
- `page.click_element`, `insert_text`, and `type_keystrokes` are not implemented.
- Some GTK menus, SDL, Allegro, and other surfaces reject `XSendEvent`; see [#2022](https://github.com/trycua/cua/issues/2022). Newer routes improve honesty and can use stronger injection where available, but generic background delivery is not uniform.
- The X11 cursor overlay can consume high idle CPU in `0.7.1`; see [#2204](https://github.com/trycua/cua/issues/2204).

### Linux Wayland

Wayland support has improved considerably on `main`, but it remains compositor-dependent and runtime opt-in through `CUA_DRIVER_RS_ENABLE_WAYLAND=1` in the implementation. Portal/libei input also requires the `portal-input` Cargo feature. Official release builds enable it; an ordinary source build with default features does not.

- Semantic AT-SPI actions can target native Wayland application content without raw global coordinates.
- Pixel delivery uses compositor protocols, portal/libei, or supported nested-compositor routes. A standard Wayland desktop does not expose a universal API for arbitrary input to a specific occluded surface.
- Native toplevel protocols may omit PID or geometry. In that state, `get_window_state` can silently return the entire output without metadata identifying it as a fallback. The caller cannot distinguish it from a window capture, so window-relative PX is unsafe.
- Sway has the strongest accepted matrix. GNOME has targeted evidence; KDE and general compositor portability remain less complete.
- Generic cursor-preservation proof is unavailable because Wayland intentionally hides global pointer state; see [#2194](https://github.com/trycua/cua/issues/2194).

Main already fixes or substantially changes several still-open Wayland reports, including [#2105](https://github.com/trycua/cua/issues/2105) and [#2145](https://github.com/trycua/cua/issues/2145). The issue tracker should distinguish unreleased fixes from remaining compositor gaps.

## User-friction issue audit

### P0: target safety and truthful capability discovery

| Issue | User impact | Recommended outcome |
| --- | --- | --- |
| [#2200](https://github.com/trycua/cua/issues/2200) ambiguous PID-only targets | Input can land in the wrong browser window, document, or dialog | Require `window_id` when a PID owns multiple viable windows and validate PID/window ownership on every route |
| No stable CDP page identity | A port or first-page selection can address an unrelated tab/profile | Add `page.list_targets` and a stable `page_target_id` bound to browser PID, native window where possible, URL, and CDP target |
| No `page.prepare`/capabilities call | Agents learn setup requirements only after failed actions | Return available read, DOM, JS, input, and navigation routes plus explicit user/setup requirements |
| `page` has no Cua Driver session | Page actions use shared state or the `default` cursor and weaken concurrent-agent isolation | Carry `session` through page dispatch, CDP state, recording, and cursor behavior |
| No uniform capture-validity contract | AX/UIA failure can remove PX recovery; Wayland can return an unlabeled whole-output image; Windows can discard an occlusion signal | Decouple capture from tree traversal and return source, bounds, scale, occlusion, and fallback metadata |
| Cross-platform schema overpromises | Agents call actions that can only fail with `not implemented` | Capability-gate actions or implement the shared CDP route everywhere; never advertise unsupported behavior without a structured availability result |

### P0: real-browser reliability

| Issue | Surface | Current state |
| --- | --- | --- |
| [#2176](https://github.com/trycua/cua/issues/2176) | All web runtimes | Agents should escalate from failed background native delivery to an available `page` route before foregrounding |
| [#2192](https://github.com/trycua/cua/issues/2192) | Authenticated Chrome profiles | User-approved remote debugging needs a first-class prepare, status, and target-selection flow |
| [#2201](https://github.com/trycua/cua/issues/2201) | Windows Chrome | Background click is nondeterministic and may disturb the foreground application |
| [#2202](https://github.com/trycua/cua/issues/2202) | macOS Chrome | Foreground drag does not complete in real Chrome |
| [#1616](https://github.com/trycua/cua/issues/1616) | Windows Chromium/Electron | Repo fixtures force renderer accessibility, but already-running real apps may expose only browser chrome |
| [#2101](https://github.com/trycua/cua/issues/2101) | Windows Firefox | Minimized and modal states remain less reliable than the visible Firefox path |

### P1: setup, performance, and observability

| Issue | Friction |
| --- | --- |
| [#2084](https://github.com/trycua/cua/issues/2084) | Windows/Linux lack cached CDP text input, keystrokes, and PID-to-port discovery |
| [#2100](https://github.com/trycua/cua/issues/2100), [#2110](https://github.com/trycua/cua/issues/2110), [#2113](https://github.com/trycua/cua/issues/2113) | Windows UIA scans can be very slow or block on provider code |
| [#2015](https://github.com/trycua/cua/issues/2015) | Windows secondary-monitor bounds can make capture and input target invalid coordinates |
| [#1882](https://github.com/trycua/cua/issues/1882) | Screenshot resize metadata does not fully explain the PX coordinate transform to callers |
| [#1902](https://github.com/trycua/cua/issues/1902) | Agent cursor is not composited into window screenshots, reducing trajectory explainability |
| [#1975](https://github.com/trycua/cua/issues/1975) | Tool-level telemetry is insufficient to quantify route success and fallback friction |
| [#1799](https://github.com/trycua/cua/issues/1799) | Multi-agent and HTTP transport behavior needs a clearer concurrency contract |
| [#2160](https://github.com/trycua/cua/issues/2160) | Session startup can fail through some Codex transport paths |

### P1: Linux environment portability

| Issue | Friction |
| --- | --- |
| [#2022](https://github.com/trycua/cua/issues/2022) | X11 background events are rejected by some toolkits |
| [#1922](https://github.com/trycua/cua/issues/1922) | Native Wayland work remains an umbrella of compositor-specific gaps |
| [#1946](https://github.com/trycua/cua/issues/1946) | `wf-recorder` covers supported wlroots environments; GNOME/KDE portal-video coverage remains incomplete |
| [#1982](https://github.com/trycua/cua/issues/1982) | KWin target activation and end-to-end input remain less proven despite portal/libei and packaging fixes |
| [#2041](https://github.com/trycua/cua/issues/2041), [#2198](https://github.com/trycua/cua/issues/2198) | PID, app identity, and geometry vary across X11, compositor, and AT-SPI namespaces |
| [#2194](https://github.com/trycua/cua/issues/2194) | Cursor-preservation evidence cannot use a generic global observer on Wayland |

## Issue hygiene

The audit found open issues whose state no longer describes current `main`. They should be closed after a focused revalidation or rewritten around the residual gap:

| Issue | Current-main assessment |
| --- | --- |
| [#1704](https://github.com/trycua/cua/issues/1704) | WKWebView fixtures and the converged macOS matrix now exist |
| [#1819](https://github.com/trycua/cua/issues/1819) | The X11 overlay now uses an empty input region |
| [#1942](https://github.com/trycua/cua/issues/1942) | The referenced Electron recording suite was replaced by the converged harnesses |
| [#2010](https://github.com/trycua/cua/issues/2010) | GNOME accessibility enablement no longer claims a screen reader |
| [#2076](https://github.com/trycua/cua/issues/2076) | Linux responses now include `app_name` and visibility; canonical identity remains in #2198 |
| [#2079](https://github.com/trycua/cua/issues/2079) | macOS keyboard schemas now expose foreground/background delivery |
| [#2105](https://github.com/trycua/cua/issues/2105) | Portal/libei input and a GNOME GTK matrix landed after `0.7.1` |
| [#2145](https://github.com/trycua/cua/issues/2145) | The reporter confirmed the native Wayland discovery/capture fix on main |

Other reports are only partially superseded and should remain open with a narrower title: [#1616](https://github.com/trycua/cua/issues/1616), [#1922](https://github.com/trycua/cua/issues/1922), [#1946](https://github.com/trycua/cua/issues/1946), [#1978](https://github.com/trycua/cua/issues/1978), [#1982](https://github.com/trycua/cua/issues/1982), [#2081](https://github.com/trycua/cua/issues/2081), and [#2101](https://github.com/trycua/cua/issues/2101).

## Test and validation audit

### What the canonical harnesses prove

The shared cross-platform fixture currently defines 40 cells per browser-like host:

- left, right, and double click: AX/PX, foreground/background;
- text, text plus Return, key, hotkey, scroll, and child-window behavior: AX/PX, foreground/background;
- drag: PX, foreground/background;
- editor type/save/readback: AX, foreground/background.

The accepted evidence in [action-support.md](action-support.md) covers:

| Platform | Browser-like hosts | Accepted scope |
| --- | --- | --- |
| Windows | Electron/Chromium, Tauri/WebView2, native WPF WebView2 | Shared AX/PX behavior plus separate WebView2 probes |
| macOS | Electron/Chromium, Tauri/WKWebView, native WKWebView | Shared AX/PX behavior across all three hosts |
| Linux X11 | Electron/Chromium, Tauri/WebKitGTK under Xvfb/Openbox | Shared behavior; Xvfb does not prove real-Xorg MPX/uinput |
| Linux Wayland | Native Ozone/Chromium Electron and WebKitGTK Tauri under Sway | Shared behavior or exact refusal with XWayland disabled |

Background cells verify fixture state, focus, z-order, and leaked input. Before/after screenshots and state artifacts are required. This is strong evidence for native action routing on deterministic embedded surfaces.

### What the harnesses do not prove

The canonical Rust suite does not launch a standalone Chrome/Chromium browser, Edge, Safari, Firefox, Brave, Arc/Dia, Vivaldi, or Opera. Electron proves a Chromium renderer, not browser chrome, tabs, profiles, extensions, downloads, policies, or security prompts. WebView2 proves the Edge runtime, not Edge. WKWebView and WebKitGTK do not prove Safari.

The actual `page` GUI E2E suite is Windows-only. Four ignored tests cover these behaviors:

- a WebView2 PX background-click test uses `page.click_element` to resolve geometry, then verifies foreground and background native click routes;
- a WebView2 page roundtrip exercises `execute_javascript` and `click_element`;
- an Electron test exercises `execute_javascript`;
- a separate Electron regression test exercises `click_element`.

There is no retained GUI E2E for every `page` action, CDP discovery, multi-tab selection, authenticated profiles, or a page-to-native-window binding. There is no `page` E2E on macOS or Linux. CLI/MCP parity for browser actions is also unproven.

The matrix documentation additionally claims checkbox, radio, combo, and slider coverage because those controls exist in the fixture, but no shared behavior rows exercise them. Either add the rows or narrow [test-matrix.md](test-matrix.md).

### Current CI posture

The Windows, Linux X11, and Linux Wayland E2E workflows are manually dispatched. macOS has no GitHub-hosted E2E workflow because permissions and interactive desktop state are host-dependent. Accepted evidence is a ledger, not an automatic proof for every commit, and its cited SHAs predate this audit target.

That is appropriate for expensive interactive E2E, but releases should require a fresh exact-SHA browser evidence bundle rather than inheriting an older accepted baseline.

## Documentation and interface drift

The following documentation problems directly affect agent success:

- The generated MCP reference is built on macOS and therefore presents a macOS-shaped `launch_app` schema as if it were universal.
- Public guidance implies broad cross-platform JavaScript support, while setup and fallback semantics differ sharply.
- `WINDOWS.md` and `MACOS.md` describe older `page` action counts.
- Windows skill examples use `args`, while the runtime launch schema uses `additional_arguments`.
- Some macOS guidance passes a `javascript` field to `get_window_state`, which the current schema does not accept.
- Linux skill metadata still describes Wayland as opt-in/preview, while other public pages use broader supported language. The implementation is definitively opt-in today.
- The driver remains versioned as `0.7.1` on post-tag `main`, so `--version` alone cannot identify whether a user is running the released or source-built convergence work.

Schema examples should be executable tests generated separately for each OS. Product support claims should link to an exact-SHA evidence ledger.

## Recommended product plan

### Phase 0: make the current contract truthful

1. Triage and close the stale issues listed above, explicitly noting whether the fix is unreleased.
2. Generate platform-specific MCP schemas and validate every bundled skill example against the matching runtime registry.
3. Return structured `available`, `unavailable`, and `requires_setup` results for every `page` action instead of exposing methods that fail with generic `not implemented` errors.
4. Include build SHA and dirty/source-build metadata in `--version` and `doctor` output.
5. Decouple screenshot capture from accessibility traversal so an AX/UIA timeout still leaves an agent a safe PX recovery artifact when capture itself succeeds.

### Phase 1: introduce a browser preparation and targeting contract

Add a user-consented `page.prepare` flow that returns a stable page target:

```json
{
  "page_target_id": "browser-page-1",
  "browser": "chrome",
  "profile_kind": "default",
  "native_target": { "pid": 1234, "window_id": 5678 },
  "page": { "cdp_target_id": "...", "url": "https://example.com" },
  "routes": {
    "read": "cdp",
    "dom": "cdp",
    "javascript": "cdp",
    "input": "cdp",
    "native_background": "unavailable"
  },
  "requires_user_action": null,
  "limitations": []
}
```

The contract should:

- enumerate tabs/pages before selecting one;
- bind the debugging endpoint to the expected process;
- fail closed when URL/window/tab selection is ambiguous;
- preserve explicit user approval for authenticated default-profile debugging;
- report browser restart, profile isolation, policy, permission, and port requirements;
- accept the Cua Driver `session` so recordings, cursors, cleanup, and concurrent agents stay isolated.

### Phase 2: make CDP a shared cross-platform engine

1. Finish [#2084](https://github.com/trycua/cua/issues/2084): cached sessions, PID-to-port discovery, `insert_text`, and `type_keystrokes` on Windows/Linux.
2. Implement `click_element` on Linux through the same target/session abstraction.
3. Add first-class `navigate`, `reload`, `go_back`, `go_forward`, and DOM-aware `fill` operations.
4. Make `query_dom` report whether its result came from DOM, UIA/AX/AT-SPI approximation, or another fallback.
5. Avoid a focus-affecting bookmark workflow when a safe CDP route is available; keep any fallback explicit in the result.

### Phase 3: automate safe route selection

Implement [#2176](https://github.com/trycua/cua/issues/2176) as a capability-aware policy:

1. exact-target semantic background action;
2. exact-target page/CDP action for web content, with the result identifying whether it used trusted CDP input or an untrusted DOM event;
3. verified background PX action where the platform/surface supports it;
4. foreground action only after explicit policy permits it.

Every result should report the requested route, actual route, verification state, focus before/after, and whether fallback occurred. Silent route changes are unacceptable.

### Phase 4: add real-browser compatibility lanes

Keep repository-owned Electron/Tauri/WebView fixtures as the deterministic canonical matrix, then add optional release-gate lanes using a local deterministic website in real browsers:

| OS | Minimum real-browser lane |
| --- | --- |
| Windows | Chrome, Edge, Firefox |
| macOS | Chrome, Safari, Firefox |
| Linux X11 | Chrome/Chromium, Firefox |
| Linux Wayland | Native Ozone Chromium, Firefox where the compositor supports the required route |

Each applicable browser should cover:

- launch/attach and exact window/tab selection;
- navigation and page text readback;
- DOM query, click, fill, typing, scroll, and drag;
- tabs, popups, dialogs, file picker/upload, download, and permission prompts;
- isolated automation profile and user-approved authenticated profile;
- foreground and background native routes where advertised;
- full-occlusion focus sentinel, leaked-input oracle, before/after state, screenshots, video, and tool trace;
- explicit structured refusals for unsupported routes.

These lanes should be manually dispatched on PRs and required on release candidates. They should report product truth without weakening tests to match the driver.

### Phase 5: observability and concurrency

1. Add the `session` field to `page` and stop using the shared `default` cursor.
2. Key page, accessibility, resize, and zoom state by session plus exact target and snapshot generation where appropriate.
3. Split read-only and mutating page telemetry so `get_text`/`query_dom` do not create misleading mutation turns.
4. Emit route-level metrics for preparation failures, capability refusals, wrong-target prevention, fallback use, focus changes, and verification outcomes.
5. Establish CLI, direct MCP, daemon/proxy, and concurrent-agent conformance tests.

## Definition of done

The agentic browsing experience is coherent when:

- an agent can ask what is possible before attempting an action;
- one stable target identifies the native window and browser page together;
- the same page operations behave consistently across supported OSes or return a structured capability refusal;
- background claims are proven in real browsers with focus and input-leak oracles;
- authenticated-profile enablement is explicit, reversible, and user-approved;
- every release links exact-SHA evidence for embedded fixtures and real browsers;
- examples and generated schemas are tested against the platform binary that users actually install;
- a failure leaves enough screenshot, state, route, and trace evidence for the agent to choose a safe next action.

## Bottom line

Cua Driver's native browser control is substantially stronger than its current browser facade. The next step is not to replace AX/PX with CDP, but to unify them behind an exact, capability-aware page target. That would let agents use accessibility for ordinary controls, CDP for durable background web actions, and pixels only when needed, while retaining Cua Driver's cross-application focus and safety guarantees.
