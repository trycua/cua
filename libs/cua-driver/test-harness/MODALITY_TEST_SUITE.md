# Cua Driver — Modality Test Suite & Test-App Harnesses

Branch: `cua-driver/modality-recordings-and-fixes` (PR #2064).

> **Note on naming:** the path `TEST_SUITE.md` in this directory is already taken
> by a doc about the **Rust integration-test taxonomy** (`cua-driver-testkit`;
> the `protocol_/transport_/harness_/modality_/guard_` test families). This
> document is the complementary layer: the **modality recorder** and the
> **test-app harnesses** it drives. If you want this content at `TEST_SUITE.md`,
> rename/merge deliberately — don't clobber the integration-test doc.

This document describes the cua-driver test-harness: a cross-OS, cross-toolkit
rig that exercises every driver action against a controlled application and
scores each action **twice** — did it land an effect, and did it keep the app in
the background. It is the authoritative map of *which driver mechanisms work
where*, and why each limitation manifests in the mode/scope it does.

Grounded in:
`modality-recordings/FINDINGS.md`, the repo-root `CUA_DRIVER_LIMITATIONS_AND_TEST_MATRIX.md`,
`shared/scenarios.json`, the harness sources under `apps/`, the recorders under
`modality-recordings/{windows,linux,macos}/`, `vision-agent-test/README.md`, and
the fix-commit bodies (`79e546ca`, `e7145c18`, `80b4e3d7`, `531aa6de`,
`c3efb587`, `0d905038`, `010fdd78`). Anything not corroborated by a source is
marked **(unverified)**.

---

## 1. Overview — what the suite validates

The suite validates two things, per driver action, per surface:

1. **The no-foreground / background contract** — cua-driver's differentiator.
   The driver is supposed to drive an app **without bringing it to the
   foreground** and **without moving the visible cursor** (it dispatches via the
   accessibility tree or via background coordinate injection, and paints a
   per-session overlay cursor instead of warping the real pointer). The suite
   measures, for every action, whether the target app stayed in the background
   (`held`) or jumped frontmost (`STOLE FOCUS`).

2. **Cross-toolkit dispatch coverage** — whether an action *actually lands* on
   each UI toolkit. The same 8-action × 6-control matrix runs against WPF,
   WinUI3, WebView2, Electron (Windows), GTK3 + Electron (Linux), and AppKit,
   SwiftUI, WKWebView, Electron (macOS). Toolkits route input differently (UIA
   Invoke vs AT-SPI `doAction` vs AX actions vs synthetic pixels vs composition
   input-sites), so "click works" is a per-toolkit, per-dispatch-path fact, not
   a global one.

**Scope caveat — this tests driver *mechanisms*, not agent task success.** A cell
that reads ✓ means the driver delivered the action and the target's own
instrumented state changed (a checkbox toggled, a slider moved, `last_action=`
updated). It does **not** mean an LLM agent located the control correctly or
chose the right action. Agent locator quality and agent self-judgment are
explicitly kept on separate axes (see §7) so a bad locator can never mask — or be
masked by — a driver regression.

---

## 2. How the suite works — the modality recorder

The core instrument is the **modality recorder** (one per OS:
`windows/wpf-recorder.ps1`, `linux/lin-rec.py` + `lin-rec-electron.py`,
`macos/mac-rec.py`). Each run produces one annotated screen recording:

- **Test harness on the LEFT**, **`cua-driver-panel` dashboard on the RIGHT**
  (always-on-top). The dashboard is a tiny HTML page served over loopback and
  shown via a `chrome --app` (Windows/macOS) or WebKit (Linux) window; it polls a
  `status.json` the recorder writes (~5 Hz) and renders the live verdict.
- The dashboard shows the run's **modality**, a live **foreground/background
  indicator** (current frontmost window title), and a **per-action** dual verdict.
- The pink/cyan overlay is the driver's **per-session agent cursor** — proof the
  driver is acting without a real pointer move.

### The 5 modality modes (per surface)

Capture method × foreground/background × scope = 5 single-modality runs:

| Mode             | Capture (`capture_mode`) | Dispatch        | Foreground? | Scope     |
|------------------|--------------------------|-----------------|-------------|-----------|
| `ax-fg`          | `ax` (accessibility tree)| `element_index` | app kept FRONT | window  |
| `ax-bg`          | `ax`                     | `element_index` | app stays BACKGROUND (contract measured) | window |
| `vision-fg`      | `vision` (pixels)        | pixel coords    | app kept FRONT | window  |
| `vision-bg`      | `vision`                 | pixel coords    | app stays BACKGROUND (contract measured) | window |
| `vision-desktop` | `vision`                 | screen pixels (window-less) | FRONT | desktop |

The fg/bg split only matters for the contract (measured in the `*-bg` runs).
`vision-desktop` sets `capture_scope=desktop` and issues window-less,
screen-absolute actions.

### Dual scoring per action

Every action row carries **two independent measurements** (rendered as two
badges on the dashboard):

1. **✓ worked / ✗ no-op (effect)** — did the action change the harness's *own*
   state? Read back after the action via the harness's instrumented status
   labels (`agreed=`, `slider_value=`, `last_action=`, `mirror=`, `menu_action=`,
   `scroll_offset=`).
2. **held / STOLE FOCUS (contract)** — sampled for ~1.5 s after the action: did
   the target window's title become the frontmost window? In fg modes this badge
   is replaced by `foreground` (steal is expected/intentional).

The dashboard tally at the bottom is the live count: `effects: N/M actions
changed the app` and, in bg modes, `focus contract: S/A stole focus`.

### How the verifier reads each toolkit's instrumented labels

The harnesses all expose the **same set of status labels** (single source of
truth: `shared/scenarios.json` + `shared/web/index.html`), but the verifier
reads them differently per platform:

| Platform / toolkit            | How the verifier reads state |
|-------------------------------|------------------------------|
| Windows WPF / WinUI3          | **UIAutomation** — finds the harness window by title substring, scans `ControlType.Text` descendants, regex-extracts `agreed=`, `slider_value=`, `last_action=`, `mirror=`, `menu_action=`, `scroll_offset=` |
| Windows WebView2 / Electron   | Same UIA scan, but the window is matched by **substring** because Chromium titles carry a `[cdp=NNNN]` suffix; the web-AX tree only surfaces with `--force-renderer-accessibility` |
| Linux GTK3                    | The harness writes a **state file** (`/tmp/cua-lin-state.json`); the recorder's `hstate()` reads it (AT-SPI text isn't reliably enumerable for all labels) |
| Linux Electron                | Chromium **web-AX over AT-SPI** (`--force-renderer-accessibility`), read via `get_window_state(capture_mode=ax)` |
| macOS AppKit / SwiftUI        | **AX** — status labels render as AX text nodes in `tree_markdown`; `do_native()` drives the action, then re-reads the labels |
| macOS WKWebView / Electron    | Chromium/WebKit web-AX text (`WEBISH` path), same label contract |

The per-action `verify(action, before, after)` logic is consistent across
recorders, e.g.: `click` → `agreed` flipped; `double` → `last_action ==
double_click`; `right` → `last_action == right_click` **or** `menu_action`
changed; `drag` → `slider` increased; `scroll` → `scroll_offset` increased;
`setval` → `mirror == set-by-cua`; `type` → `mirror` contains `typed-by-cua`;
`press-key` (Tab) → no asserted effect (`na`).

> **Honesty principle (FINDINGS.md):** the harness/recorder stays honest — real
> driver gaps are documented as driver work, fixed in the driver first, then
> re-recorded. The recorder does not paper over a no-op.

### Session lifecycle & disposal

Each recorder declares a **session** (the agent-cursor identity, e.g. `d1` /
`macd-…`) via `start_session`, drives every action under it, and must
`end_session` it on teardown so the cursor + per-session state are disposed —
not leaked.

- **Disposal is double-guarded.** `end_session` and the **idle-TTL sweep**
  (`evict_idle`, default **300 s**, overridable via
  `CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS`, swept every 30 s) both dispose through
  the same `fire_session_end` hook fan-out: agent cursor removed, recording
  stopped, per-session config cleared. A reused id starts **fresh, not poisoned**.
- **Test:** `cua-driver-core/tests/session_lifecycle.rs` asserts start → use →
  `end_session` disposes (and a reused id starts fresh), and that idle-TTL
  eviction disposes the **same** way — verified via the disposal-hook contract
  both paths fan out to (the concrete cursor/config registries are a daemon-layer
  assertion). It passes a short TTL straight to `evict_idle`; the **300 s
  production default is never lowered** for tests.
- **The recorders dispose explicitly** — `end_session` in a `try/finally` before
  they kill the daemon (`mac-rec.py` via an `atexit` net, since it leaves the
  daemon running) — so a session can't leak even if a run errors.

---

## 3. Test-app harnesses

Each harness is a self-contained host app the driver drives. They share fixtures
via `shared/scenarios.json` (AutomationIds / AX identifiers / DOM ids /
window titles) and, for the web hosts, `shared/web/index.html`, so identifiers
never drift between apps and the Rust integration tests under
`rust/crates/cua-driver/tests/harness_*.rs`. Built outputs stage into
`rust/test-apps/harness-{name}/` via `build/{macos.sh,windows.ps1,linux.sh}`.

### The 6 parity controls

The "control-parity pass" brought every surface up to the WPF baseline of **6
controls**, so the same 8-action matrix is exercisable everywhere:

1. **checkbox** (`chk-agree*`) → `agreed=`
2. **click-target** (left / right / double) → `last_action=`, `clicks=`
3. **slider** (`sld-value`) → `slider_value=`
4. **scroll-target** (tall `scroll-tall` region) → `scroll_offset=`
5. **text-input** (`txt-input`, mirrored) → `mirror=`
6. **context-menu** (`btn-context`) → `menu_action=`

(Plus a `counter=` increment button used as a generic AX-press probe.) WPF
carries an extended scenario set on top (modal MessageBox, owned/layered popups,
native Win32 child HWND, menu bar, combo/list boxes) — it is the "fullest
harness".

### Per OS / toolkit

| OS      | Harness   | What it is | Build | Source |
|---------|-----------|------------|-------|--------|
| Windows | **WPF**       | WPF / UIA host — popups, layered windows, modal MessageBox, HwndHost native child | `build/windows.ps1` → `harness-wpf` | `apps/windows/wpf/` (C#/XAML) |
| Windows | **WinUI3**    | WinUI3 unpackaged — DirectComposition popups, XAML Popup primitive, CommandBarFlyout; HWND subclassed to translate `WM_VSCROLL` → `ChangeView` | `build/windows.ps1` → `harness-winui3` | `apps/windows/winui3/` |
| Windows | **WebView2**  | WPF host + `Microsoft.Edge.WebView2` loading the shared DOM; CDP on `:9222` | `build/windows.ps1` → `harness-webview` | `apps/windows/webview2/` |
| Windows | **Electron**  | Electron host loading the same shared DOM; CDP on `:9223` | `apps/cross-platform/electron/build.ps1` | `apps/cross-platform/electron/` |
| Linux   | **GTK3**      | PyGObject GTK3 app; AT-SPI exposes accessible **NAME** (not an AutomationId); writes `/tmp/cua-lin-state.json` for the verifier | `build/linux.sh` → `harness-gtk3` | `apps/linux/gtk3/main.py` |
| Linux   | **Electron**  | Same Electron bundle run from `node_modules` under Xvfb + dbus with `--force-renderer-accessibility` | run via `lin-run-electron.sh` | `apps/cross-platform/electron/` |
| macOS   | **AppKit**    | Cocoa — NSButton, NSScrollView, NSMenu, NSSlider, menubar item | `build/macos.sh` → `harness-appkit` | `apps/macos/appkit/main.swift` |
| macOS   | **SwiftUI**   | SwiftUI — `.popover()`, `.contextMenu`, declarative `Slider`/`Toggle`/`ScrollView` | `build/macos.sh` → `harness-swiftui` | `apps/macos/swiftui/main.swift` |
| macOS   | **WKWebView** | Apple WebKit host loading the shared DOM — the native analogue of WebView2 | `build/macos.sh` → `harness-wkwebview` | `apps/macos/wkwebview/main.swift` |
| macOS   | **Electron**  | Same shared DOM under Chromium-on-macOS | `apps/cross-platform/electron/build.sh` | `apps/cross-platform/electron/` |

### Instrumented labels (the verifier contract)

Every harness updates these labels in response to a successful action; the
verifier asserts on them:

| Label            | Set by                         | Verifies |
|------------------|--------------------------------|----------|
| `last_action=`   | click-target (left/right/double)| click / double / right landed |
| `clicks=`        | click-target                   | click count |
| `slider_value=`  | slider                         | drag / set_value on a range control |
| `scroll_offset=` | scroll-target                  | scroll landed |
| `agreed=`        | checkbox                       | click toggled a toggle |
| `menu_action=`   | context-menu item              | right-click → context menu → item invoked |
| `mirror=`        | text-input                     | type / set_value into a text field (`typed-by-cua` / `set-by-cua`) |
| `counter=`       | increment button               | generic AX-press probe |

---

## 4. Capture modes & scopes

### Capture mode (`get_window_state` `capture_mode`)

| Mode     | What the agent sees                  | How actions dispatch |
|----------|--------------------------------------|----------------------|
| `ax`     | accessibility tree only (element indices) | **`element_index`** — UIA Invoke (Windows) / AT-SPI `doAction` (Linux) / AX actions (macOS) |
| `vision` | raw screenshot / pixels only         | **pixel coordinates** — synthetic/injected pointer events at (x,y) |
| `som`    | screenshot + indexed overlays (set-of-marks) | hybrid: indices map back onto pixel regions |

> `som` (set-of-marks) is a capture/presentation variant — a screenshot with
> numbered overlays on detected elements. The modality recorder runs `ax` and
> `vision` only; `som` is part of the driver's capture surface (and is covered by
> the Rust `modality_capture_mode_test`) but is **not separately scored by the
> modality recorder** **(unverified — no `som` recorder run exists in this branch)**.

### Capture scope (`capture_scope`)

| Scope     | Meaning                        |
|-----------|--------------------------------|
| `window`  | single target window (default) |
| `desktop` | full display                   |

### Why mode/scope determines where a limitation shows

This is the central organizing idea (from `CUA_DRIVER_LIMITATIONS_AND_TEST_MATRIX.md`):

- **`ax` mode = element dispatch.** Limits in the element path (a toolkit with no
  `doAction` for right-click, a WinUI3 composition input-site the WPF path can't
  reach, an AXValue a SwiftUI slider rejects) show up in **`ax`**.
- **`vision` mode = pixel dispatch.** Limits in the pixel path (GTK dropping
  synthetic X events, NSButton ignoring a synthetic mouseDown, a 2× Retina
  coordinate conversion) show up in **`vision`**.
- **fg/bg** isolates the *contract* (only `*-bg` measures focus steal).
- **`desktop` scope** is where window-less / coordinate-space-conversion bugs
  surface (e.g. the macOS 2× Retina desktop path, the macOS no-windowless-click
  gap).

---

## 5. Action × control matrix

**8 actions** × **6 controls**. Each modality run filters the plan:
`set_value` is dropped in `vision` modes (it's AX-only); `vision-desktop` keeps
only `{click, scroll, type, press-key}` (window-less screen actions).

| Action ↓ / Control → | checkbox | click-target | slider | scroll-target | text-input | context-menu |
|----------------------|:--------:|:------------:|:------:|:-------------:|:----------:|:------------:|
| **click**            | ● (toggle) | ● (left) |        |               |            |              |
| **double-click**     |          | ●            |        |               |            |              |
| **right-click**      |          | ● (web/WinUI3 records `last_action`) | | |   | ● (opens menu) |
| **drag**             |          |              | ● (thumb→X) |          |            |              |
| **scroll**           |          |              |        | ●             |            |              |
| **set_value** (AX)   |          |              | ● (range value) |     | ● (`set-by-cua`) |        |
| **type**             |          |              |        |               | ● (`typed-by-cua`) |      |
| **press-key** (Tab)  |          |              |        |               | ● (focus move; no asserted effect) | |

Notes:
- WPF has a **dedicated context-menu control**; WinUI3 / web harnesses have none,
  so right-click is aimed at the click-target and verified via
  `last_action=right_click`.
- SwiftUI's AX has no distinct right-press action, so right-click on its button
  coerces to a normal AXPress — right-click semantics there are covered by the
  dedicated `context_menu` scenario.

---

## 6. RESULTS MATRIX (the core)

Per cell: **✓ landed · ✗ no-op · ⚠ guarded/fail-loud** for the *effect*, with the
reason. The **Contract** column is the focus verdict measured in the `*-bg` runs.
"land" = the harness's own instrumented label changed. Verification status is
drawn from FINDINGS.md, the limitations doc, and the fix-commit bodies; runtime
status is flagged where a commit deferred it.

> Legend: ✓ lands · ✗ no-op (honest) · ⚠ guarded clean-fail / fail-loud ·
> — not exercised · n/a not in plan / no asserted effect.

### macOS

Contract: **HOLDS — 0/8 stole** on AppKit in both `ax-bg` and `vision-bg`
(pixel left-click/double/drag landed while Chrome stayed frontmost). WKWebView
also holds — confirming the Windows Chromium steal is Windows-specific, not
WebKit-on-macOS.

| Harness | mode | click | double | right | drag | scroll | set_value | type | press-key | Contract | Notes |
|---------|------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|------|
| AppKit  | `ax` | ✓ | ✓ (fixed bg, `click_at_xy_with_window_local`, `80b4e3d7`) | ✓ (AXShowMenu→delivering-pixel fallback, `80b4e3d7`) | ✓ | ⚠ element scroll no-ops on content-height containers (use x,y) | ✓ CFNumber NSSlider (`92d8aebf`, verified 0→50) | ✓ | n/a | **0/8 stole** | ax-bg 5/7 effects |
| AppKit  | `vision` | ⚠ **no-op on standard NSButton even frontmost** (modal mouseDown reads window-server queue, not `postToPid` → use `element_index`) | ✓ | ✓ now fires `rightMouseDown` (window-number + button + primer, `e7145c18`) | ✓ | ✓ via per-pid pixel-wheel `scroll_wheel_at_xy` (`e7145c18`) | n/a | ✓ | n/a | **0/8 stole** | needs target frontmost; vision-bg 4/6 |
| SwiftUI | `ax` | ✓ | ✓ | ✓ (via dedicated context-menu) | ✗ slider drag is a composition no-op | ⚠ nested scroll no-op | ⚠ **AXValue unsettable** (`-25200`); needs AXIncrement/AXDecrement stepping (`d2b29230`) | ✓ | n/a | **0/8 stole** | full 6-control parity confirmed live |
| WKWebView | `ax`+`vision` | ✓ | ✓ | ✓ | ✓ slider (pixel drag at live frame) | ⚠ **page scroll works, nested `overflow:auto` div no-op** (div has no `tabindex` → never keyboard-focusable; needs pixel-wheel) | ✗ **AXValue set, no DOM `input` event** (web mirror unchanged) | ✓ | n/a | **HOLDS** | native analogue of WebView2. Recorder fix (`mac-rec.py`): web dispatch now uses the **live element frame**, not a stale hardcoded coord map that was ~185px off → every WKWebView pixel action used to miss. ax-fg re-measured **5/6** (click/double/right/drag/type land; only `set_value` the honest web no-op). |
| Electron (mac) | `ax`/`vision` | ✓ | ✓ | — | — | ✗ (web scroll, same as WKWebView) | — | ✓ | n/a | holds (Chromium-on-mac does **not** steal like Windows) | local-only recordings |

macOS gotchas filed as driver bugs: **`end_session` poisons a reused session id**
(subsequent actions silently no-op); **AppKit window height drifts between
launches** (store targets as window-local points, convert to live screenshot px).

### Linux

| Harness | mode | click | double | right | drag | scroll | set_value | type | press-key | Contract | Notes |
|---------|------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|------|
| GTK3 | `ax` (AT-SPI) | ✓ left-click via hit-test→`doAction` (`7d358283`) | ✗ no `doAction` equiv | ✗ no `doAction` equiv | ⚠ value-only (no Action) — driven via `set_value` | ⚠ surfaced now (`is_indexable = actions \|\| has_value`); driven via `set_value` | ✓ slider/scroll value widgets now surface | ✓ | n/a | ax-bg **1/8 stole** (only `set_value`; corrected via genuine-anchor baseline `5c0a1d3c` — was 3/8) | left/set_value/type land |
| GTK3 | `vision` (Xvfb) | ✗ | ✗ | ✗ | ✗ | ✗ | n/a | ✗ | n/a | **0/7 stole** | **GTK drops synthetic XSendEvent**; XTEST core events don't reach its XInput2 path |
| GTK3 | `vision` (**real Xorg**) | ✓ | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | n/a | holds focus | right/double/middle-click + scroll **land via uinput/XInput2-MPX + shield-grab** (`79e546ca`); capability auto-detected via `real_pointer_input_available()` — **runtime-verified on a real Xorg server (dummy-driver): the probe flips TRUE, all four actions LAND and HOLD focus (`_NET_ACTIVE_WINDOW` unchanged), confirmed by the harness oracle + a middle-click PRIMARY paste. Xvfb can't bind uinput as an X slave, so the path auto-skips there.** |
| Electron (Linux) | `ax` | ✓ click | ✗ | ✗ | ✓ drag | ✗ scroll resolves but no-op (AT-SPI synthetic) | ✗ | ✓ type | n/a | ax-bg **2/8 stole** (`set_value` + `drag`; was 3/8 — recorder baseline artifact, `5c0a1d3c`) | drag lands a value-change but its synthetic window-coord activates Chromium (never reaches the slider thumb); still holds far better than Windows Electron's reported 7/8 (also an artifact) |
| Electron (Linux) | `vision` | ✗ | ✓ pixel double-click fires on click-target | ✗ | ✗ | ✗ | n/a | ✗ | n/a | vision-bg **6/7 stole** (pixel dispatch foregrounds Chromium) | |

### Windows

Contract: **HOLDS on WPF / WinUI3 / WebView2.** **Electron** — see the caveat
below (the prior "7/8 stole" is a recorder measurement artifact).

| Harness | mode | click | double | right | drag | scroll | set_value | type | press-key | Contract | Notes |
|---------|------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|------|
| WPF | `ax` | ✓ (UIA Toggle) | ✓ | ⚠ off-screen at 556px reflow → clean no-op (guarded; UIA Invoke works off-screen) | ✓ slider 0→48 | ⚠ off-screen reflow → guarded no-op | ✓ (`mirror=set-by-cua`) | ✓ (recorder focuses the box first) | n/a | **2/8 stole** | "fullest harness"; ax-bg ~5/7 effects |
| WPF | `vision` | ✓ | ✓ | ⚠ guarded (`point_in_window_bounds` refuses off-screen point; off-screen controls now ScrollIntoView + actuate via ancestor ScrollPattern, `c3efb587`, runtime-verified counter 0→1) | ✓ | ⚠ off-screen reflow | n/a | ✓ | n/a | 1–2/7 stole | off-screen guard prevents taskbar misfire |
| WinUI3 | `ax` | ✓ left-click (UIA Invoke) | ✓ **now LANDS** — double UIA-Invoke under `WS_EX_NOACTIVATE` guard (`531aa6de`, runtime-verified live) | ⚠ **fail-loud `background_unavailable`** — no contract-safe path (pen injection steals; WM_*BUTTON doesn't land) | ⚠ fail-loud (same composition input-site gap) | ✓ (HWND subclass: `WM_VSCROLL`→`ChangeView`) | ✓ | ✓ | n/a | **0/7 stole** | gap needs a WinUI3-specific composition InputSite path |
| WebView2 | `ax` | ✗ checkbox below the fold (web won't scroll in ax) | ✓ | ✓ records `last_action=right_click` | ✓ | ✗ host HWND doesn't route scroll to renderer | ✓ | ✓ | n/a | **0/7 stole** | needs `--force-renderer-accessibility`; 4/7 effects |
| Electron | `ax` | ✓ | ✓ | — | ✓ | ✗ web scroll | ✓ | ✓ | n/a | **see caveat** | 5/7 effects |

Other Windows: **UIA element-cache use-after-free** (concurrent `ax` sessions on
the same window) — fixed with a `RetainedElement` retain-under-lock guard
(`d95b89a1`); proven by a deterministic `cache_uaf_repro` test (pre-fix path
takes a real access violation, fixed path survives a 6-thread stress loop,
`531aa6de`). Recording the VM needs **Session 2 attached** (`tscon … /dest:console`)
and **ffmpeg on PATH before `serve`**.

> **Windows Electron contract caveat — RESOLVED.** FINDINGS.md records Electron as
> stealing **7/8** in `ax-bg`. That was a **recorder MEASUREMENT ARTIFACT**: the
> recorder re-asserted its dashboard panel with `SetWindowPos` (z-order only, no
> activation), so it never held a real foreground baseline — after the first inject
> action click-activated Chromium, the harness stayed frontmost and every later
> step false-positived as a steal. The recorder was fixed (`49bdb41b`) to genuinely
> `SetForegroundWindow` a real non-harness anchor (mspaint) before each step.
> **Re-measured: Electron `ax-bg` = 0/8 (held)** — corroborated by `baseline.log`
> (anchor held ×8), an independent `metric.log` `MEASURE=0/8`, and frame f03 (actions
> landed while the harness stayed non-foreground). So **Electron HOLDS the contract**,
> consistent with Linux/macOS Electron. (The committed `matrix-electron-ax-bg.mp4`
> still shows the old 7/8 until re-recorded with the fixed recorder.)

---

## 7. Vision-agent coordinate test

`vision-agent-test/` (`vision_agent_test.py`, `010fdd78`) is a newer test that
hits the driver **the way a vision agent actually does** — and removes the
modality recorder's overfit (hand-tuned window-local points run through a private
ratio, which never exercises the driver's image→screen mapping).

**Invariant under test:** *the pixel an agent reads off the returned screenshot is
the pixel that gets clicked* — verified by the target's own instrumented state
changing.

The loop (no cheating in locate or click):
1. **capture** — `get_window_state(capture_mode=vision)` (window) /
   `get_desktop_state` (desktop, true pixels): the exact image an agent receives.
2. **locate** — a deterministic pixel **in the returned-image coordinate space**
   (`PixelRegistryLocator`: a pre-measured pixel read off the real PNG, with a
   dims-guard that fails loud if the pinned geometry drifts). No `element_index`,
   no hand-converted window-local points. `locate(image, target, dims) → (x,y)`
   is pluggable (an LLM locator can drop in later).
3. **act** — `click`/`right_click`/`scroll` at that pixel (scope matches capture).
4. **verify** — the harness oracle (`last_action=`, `clicks=`, …) → objective
   pass/fail. A coordinate mis-map leaves the oracle unchanged → FAIL.

Run: `python3 vision_agent_test.py {wkwebview-click-window|wkwebview-click-desktop|appkit-click-window|safari-learnmore-desktop|all}`.

**Three axes kept separate** so none can mask the others: (a) the **driver
coordinate invariant** (this test, deterministic → the regression guard); (b)
**agent locator quality** (future, LLM, same `locate()` signature, scored
separately, never gates the regression); (c) **agent self-judged success with no
oracle** (future, isolated track).

What the deterministic version caught that the modality suite structurally
couldn't:
- **2× Retina desktop path now correct + guarded** — pixel (340,1358) in the
  3024×1964 desktop PNG converts to screen-point (170,679) and lands (oracle +
  a real Safari navigation confirm it). This turns the Retina escape into a
  permanently-guarded one-liner.
- **Vision pixel-click is a no-op on AppKit `NSButton` even frontmost** (modal
  mouseDown reads the window-server queue, not the per-pid `postToPid` queue) —
  the modality suite drives this via AXPress, so it never saw it.
- **The pixel path requires the target app frontmost** (the AX path doesn't).
- **The AppKit harness window AX returns only the menu bar** — its `clicks=`
  oracle is unreachable that way; WKWebView exposes it fine.

---

## 8. Known overfitting caveats

The suite's own authors flag two ways the modality recorder could have lied, and
the de-risking work for each:

**(a) Recorder hand-coords masked the desktop 2× Retina bug.** The modality
recorder stores targets as hand-tuned window-local points and converts them
through a private ratio. That conversion is *self-consistent* with the driver's
own assumption, so it never exercised the driver's real image→screen mapping —
and the **desktop-scope 2× Retina off-by-backing-scale bug** (a center-pixel pick
warping to the corner) slipped through every `vision-desktop` run. De-risk: the
**vision-agent coordinate test** (§7) reads a pixel straight off the returned PNG
with a dims-guard, caught the bug, and now guards it (`80b4e3d7` fixes the desktop
branch to divide x,y by the native/logical ratio; `010fdd78` adds the regression
test).

**(b) Recorder contract measurement had no real foreground baseline → the false
"Electron 7/8 stole".** The recorder judges the contract by checking whether the
target's title is the frontmost window, but it never established a genuine,
distinct foreground window to begin with — so Windows Electron read as stealing
**7/8** when the corrected understanding is that Chromium-on-Windows holds the
contract (matching macOS/Linux Electron). De-risk: the **recorder-contract fix**
(`49bdb41b`) now genuinely `SetForegroundWindow`s a real anchor before each step;
**re-measured Electron `ax-bg` = 0/8 (held)**, so the claim is now verified and the
legacy FINDINGS "7/8" is a confirmed measurement artifact. The Linux recorders had
the same flaw — also fixed (`5c0a1d3c`): GTK3 + Linux-Electron `ax-bg` corrected
3/8 → 1/8 (only `set_value` genuinely steals); macOS was already fine.

> Both caveats reflect the same lesson: a recorder that hand-feeds the driver its
> own assumptions can hide bugs in exactly the path a real agent uses. The
> deterministic, oracle-checked vision-agent test and the re-baselined contract
> measurement are the two structural fixes.

---

## 9. Edge cases — real closed-source apps

The synthetic harnesses pin down the heuristics; real apps surface behaviours a
single-process test window never can. These were found driving live closed-source
apps (Finder, System Settings, Calculator, Safari on macOS; Calculator/Notepad UWP,
Edge, Explorer on Windows) and are why the suite is a floor, not the ceiling.

### macOS (Finder, System Settings, Calculator, Safari)

1. **Pixel click hits the right pixel but the wrong window.** With ~8 overlapping
   same-pid Finder windows, a crosshair dead-centre on the target file still posts to
   *screen* coordinates, so an occluding sibling intercepts it → no-op. A per-window
   screenshot masks it. Strong argument for the `ax`/element path, which is z-order
   independent.
2. **`set_value` and `type_text` both falsely succeed on a background search field**
   (System Settings, SwiftUI). `set_value` writes `AXValue` (text appears, the search
   action never fires); `type_text` posts keys that a non-first-responder window drops.
   Neither drives the field; **both report success.**
3. **Finder column filenames don't advertise `AXPress`** → a default click is an honest
   no-op; the driver surfaces `AXOpen`/`AXShowMenu`/`AXConfirm`, so an agent must pick
   `action:"open"`/`"pick"`.
4. **The Calculator result is AX-invisible** (no AX node for the display) → an AX-only
   agent can't read the answer; the keypad itself is labelled. A `vision` readout is
   required.
5. **AppKit AX-tree duplication** (System Settings ~half duplicated, Safari a duplicated
   toolbar) plus whole-menu-bar walking inflates real-app context versus synthetic.

### Windows (Calculator/Notepad UWP, Edge, Explorer)

A. **UWP window-identity split.** `launch_app("Calculator")` returns the package backing
   PID and a `window_id` that's stale by the next call; the real top-level HWND belongs to
   `ApplicationFrameHost.exe`. Driving by the returned pid+window_id errors `No window with
   window_id … exists`. The driver should resolve UWP windows to their `ApplicationFrameHost`
   host / relink the churned HWND.
B. **Real UWP is drivable via the element path without the uiAccess worker.** Calculator's
   `num5Button` drove the display 0→5 via UIA Invoke from the Medium-IL daemon with no
   `cua-driver-uia.exe` running. This refines the `#1602`/`serve.rs` assumption: only the
   **pixel/SendInput** path needs the uiAccess worker for AppContainer apps — the
   **`element_index` UIA Invoke/ValuePattern path works on real UWP as-is.**
C. **Real Edge (Chromium) holds the foreground contract.** With Notepad pinned top, 3×
   click + double-click + right-click on background Edge left the z-order unchanged
   (`(background, no foreground swap)`). The shield validated on synthetic Electron (0/8)
   **generalises to a real closed-source Chromium** — and the double/right-click that needed
   the WinUI3 composition fix synthetically just work on real Chromium in the background.
D. **`element_index` requires `pid` (+`window_id`).** Element actions with `element_index`
   alone fail-fast with `Missing required integer field: pid`. Correct, but the MCP tool
   descriptions under-emphasise it — an agent that omits `pid` and filters stderr perceives a
   silent no-op. The `element_index` tool schemas should state `pid`'s necessity explicitly.
E. **`get_screen_size` under-reports desktop width** (1024 reported vs 1824 actual span,
   likely an RDP dynamic-resolution artifact). The element path is unaffected (window-local
   frames stay consistent); a pixel/`vision` agent computing against 1024 width misplaces
   clicks in the right ~800 px band.

F. **⚠ Unhandled-protocol `launch_app` deadlocks the whole daemon (DoS-class).** Launching an
   app whose protocol has no registered handler (e.g. `bingmaps:` with Maps uninstalled) spawns
   Windows' "you'll need a new app" modal **on the daemon's session desktop** and blocks the
   worker thread inside the shell-launch call. The client wedges indefinitely and a subsequent
   `stop` reports "daemon is not running" because the wedged daemon can't service the pipe;
   recovery needs the modal dismissed from inside that session (Session-0 SSH can't see it —
   per-session window stations). **Any bad app name/protocol is a full-daemon DoS.** The driver
   should launch via a non-blocking path with a timeout and/or validate handler registration
   before `ShellExecute`. (Needs a fix + a tracking issue.)
G. **One `ApplicationFrameHost` pid multiplexes N unrelated UWP apps** (Settings, Calculator,
   and Store were all hosted under the same pid). Anything keying state/cursor/element-cache by
   `pid` alone is ambiguous across apps — only `window_id` is a real identity. (Generalises A.)
H. **`launch_app`'s return contract is effectively per-app-architecture** — three topologies
   seen across five apps: brokered/`pid:0` (Settings, Store), real-pid-but-empty-`windows` race
   (Photos exposes its pid a beat before its HWND), and clean pid+window (Snipping Tool, the
   well-behaved baseline). Store is a three-way split: launch-pid `0` ≠ window-pid (AFH) ≠
   content-pid (`WinStore.App`), yet UIA still walks fully into the content provider.
I. **The AFH UIA root is a caption-only ~188×32 strip** for Settings and Store while the content
   frames are full and correct — the AppFrame chrome and the XAML content are different UIA
   providers stitched at the window. A single-provider synthetic WinUI harness won't reproduce
   this dual-provider root.

> Methodology note: on a disconnected-RDP console session, `GetForegroundWindow` returns `0`
> (no foreground window), so focus-steal can't be measured by that probe there — verify "landed"
> by content-state change instead. A single-process harness never hits this.

---

## Linux dispatch ladder & the XFCE container lane

Linux (X11/XFCE) now validates the same `delivery_mode` background/foreground dispatch ladder that macOS and Windows exercise, closing the parity gap. The matrix below shows each modality's path and whether the driver can verify the action landed without a screenshot.

### Validated modality matrix

| Modality | `delivery_mode` | Path reported | Driver-verifiable? |
|----------|----------------|---------------|-------------------|
| Element click (`element_index`) | background | `x11_atspi` (AT-SPI `doAction`) | yes — a11y action confirmed |
| Pixel/vision click | background | `x11_atspi` (AT-SPI `doAction`-at-point) for AX apps; else MPX `x11_pixel` | yes when AT-SPI-at-point lands; best-effort otherwise |
| Pixel click (escalated) | foreground | `x11_pixel_fg` (EWMH activate → inject → restore) | no — confirm via screenshot |
| `type_text` into editable | background | `ax` (AT-SPI `insertText`) | yes (`verified: true`) |
| `type_text`, non-editable focus | background / foreground | `key_events` / `key_events_fg` | no — confirm via screenshot |

Background pixel/vision clicks do land on X11: apps that expose AT-SPI take the focus-free `doAction`-at-point path (`x11_atspi`), matching the macOS/Windows background-click behavior. The fallback to MPX `x11_pixel` (which requires a real Xorg server with `/dev/uinput` — unavailable under Xvnc or minimal containers) fires only for non-AX surfaces; escalate to `delivery_mode: foreground` there.

### The XFCE container lane

The `trycua/cua-xfce` Docker image is a reproducible, WM-equipped X11 target — xfwm4 + EWMH + full AT-SPI stack — that exercises both the foreground/EWMH and background AT-SPI modalities. It is richer than the existing Xvfb harness (no window manager, no EWMH) for those paths.

The harness script lives at `libs/cua-driver/test-harness/linux-container/calc.sh`. It drives galculator through four modalities: background AX element click, background pixel/vision click, foreground EWMH type, and background type with focus fallback.

**Gotchas:**

- **AT-SPI session-bus auto-discovery.** AT-SPI lives on the desktop session's D-Bus (`DBUS_SESSION_BUS_ADDRESS`). When the daemon starts outside the session (container entrypoint, headless, `runuser`/`su`, systemd system unit, VNC ad-hoc bus), that variable is unset, yielding an empty AT-SPI tree. The driver now auto-discovers the bus at startup — preferring `/run/user/<uid>/bus`, else reading it from a running desktop-session process's `/proc/<pid>/environ` (`xfce4-session`, `gnome-session`, etc.). An a11y bus must be running with toolkit-accessibility enabled, and the daemon must run as the desktop user — running the daemon as root against a user session is the Linux analogue of the Windows Session 0 isolation problem.
- **`install-local` refuses root.** Use `runuser -u <desktop-user> -- cua-driver serve` to start the daemon as the session owner.
- **Single-instance apps + zombie children.** Unreaped zombie children can pollute `pgrep`; a full daemon restart reaps them.
- **Stale daemon, stale tool schema.** A daemon that was not restarted after an update can serve an outdated tool schema. Verify with `cua-driver describe <tool>` from a fresh shell.

---

## Appendix — file map

| Path | Role |
|------|------|
| `shared/scenarios.json` | single source of truth: control ids, window titles, scenarios |
| `shared/web/index.html` | shared DOM for WebView2 / Electron / WKWebView harnesses |
| `apps/` | per-OS/toolkit harness sources (see §3 table) |
| `build/{macos.sh,windows.ps1,linux.sh}` | stage harnesses into `rust/test-apps/harness-*` |
| `modality-recordings/FINDINGS.md` | authoritative per-action findings |
| `modality-recordings/windows/wpf-recorder.ps1` | Windows recorder (`-Mode`, `-Toolkit`) |
| `modality-recordings/windows/run-one.ps1` | single-run launcher |
| `modality-recordings/linux/lin-rec.py`, `lin-rec-electron.py` | Linux recorders (+ `lin-run*.sh`, `lin-dash*`, `lin-harness.py`) |
| `modality-recordings/macos/mac-rec.py` | macOS recorder (`MODE SURFACE`) |
| `vision-agent-test/vision_agent_test.py` + `README.md` | coordinate-invariant regression test |
| `smoke/macos.sh` | macOS smoke check |
| `rust/crates/cua-driver/tests/harness_*.rs` | Rust integration tests consuming staged harnesses |
| `TEST_SUITE.md` (this dir) | sibling doc: the Rust integration-test taxonomy / `cua-driver-testkit` |
| `CUA_DRIVER_LIMITATIONS_AND_TEST_MATRIX.md` (repo root) | limitations + mode/scope mapping |
