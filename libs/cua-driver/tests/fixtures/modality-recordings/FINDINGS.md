# Modality recordings — findings (cua-driver behavior surfaced by the per-action verifier)

These recordings run the 8-action matrix (click, double-click, right-click, drag, scroll,
set_value, type, press-key) against a controlled harness on Windows (WPF) and Linux (GTK3), in
5 modalities each (ax-fg, ax-bg, px-fg, px-bg, px-desktop). Each action now carries
**two independent measurements**:

- **✓ worked / ✗ no-op** — did the action change the harness's own state (verified by reading the
  harness status: `agreed=`, `slider_value=`, `last_action=`, `mirror=`, `menu_action=`,
  `scroll_offset=`). Read via UIAutomation on Windows, via a harness state file on Linux.
- **held / STOLE FOCUS** — the no-foreground contract.

Principle followed: the harness/recorder stays honest (no overfitting); real gaps are documented
here as cua-driver work, to be fixed in the driver first and then re-tested.

**Historical note:** the Electron **7/8** and Linux **3/8** violation figures below
are recorder-baseline artifacts from an earlier pass and have since been addressed.
See the newer fix note later in this file and the canonical
`reference/cua-driver/modality-test-suite.mdx`.

## What the verifier surfaced (to triage as driver work)

### Windows / WPF (ax-bg, representative) — after fixes: 5/7 land
| action | effect | note |
|--------|--------|------|
| left-click checkbox | ✓ worked | UIA toggle |
| double-click button | ✓ worked | last_action=double_click |
| right-click | ✗ no-op (clean) | **Root cause: off-screen target, not a driver coordinate bug.** At 1024×768 the panel forces the harness to 556px wide, so the form reflows taller than the screen and the right-click button lands at y≈786 (below the screen). The synthetic tap previously clamped onto the **taskbar** (opened its menu). **DRIVER FIX:** `point_in_window_bounds()` now refuses a click whose resolved point is outside its window and returns a clear error — no more taskbar misfire (frame-verified). To actually *exercise* right-click needs a screen tall enough to show the control (this RDP session won't take a programmatic resize). |
| drag slider | ✓ worked | slider_value 0→48 |
| scroll | ✗ no-op (clean) | Same off-screen root cause (scroll-tall pane is lower still); now guarded, not misfiring. |
| set_value | ✓ worked | mirror=set-by-cua |
| type | ✓ worked | **Fixed:** the recorder now clicks the text box (focus) before `type_text`; type targets the focused control, and `set_value` doesn't focus, so without this it was a no-op. |
| press-key (Tab) | n/a | no asserted effect |

**Driver fix shipped (this branch):** `platform-windows` — `point_in_window_bounds()` + guards in the
click / double_click / right_click element paths. Investigated and ruled out: UIA `GetClickablePoint`
for cursor-centering (returns the same rect center or fails), and SetFocus-to-scroll-into-view (would
steal foreground, breaking the very contract these bg runs measure).

### Linux / GTK3
- AT-SPI/element actions (single-click, set_value, type) **✓ work**.
- All **pixel-based** actions (double-click, right-click, drag, scroll, every coordinate click)
  **✗ no-op**: cua-driver's Linux input is **XSendEvent** (synthetic, no focus steal — required for
  the contract), but **GTK ignores synthetic XSendEvent** (`send_event` flag). Documented in
  `platform-linux/src/input/mod.rs`. Real limitation; not a recorder bug.

### Linux / Electron (Chromium, via AT-SPI)

The cross-platform Electron harness runs on Linux straight from `node_modules` (no packaging),
launched `electron . --no-sandbox --disable-gpu --force-renderer-accessibility` under
Xvfb + a dbus session. `--force-renderer-accessibility` makes Chromium publish its full
**web-AX tree on AT-SPI**, which `get_window_state` returns cleanly alongside the
screenshot (check box "I agree", slider, entry "type here", the click-target section, and every
`agreed=`/`slider_value=`/`last_action=`/`mirror=` status label — 72 elements). Same verifier as
Windows, but reading AT-SPI text instead of UIA. Recorder: `linux/lin-rec-electron.py`
(+ `lin-run-electron.sh`, `lin-dashboard-electron.html`, `lin-all-electron.sh`).

| mode | effects landed | landed actions | focus contract (stole) |
|------|----------------|----------------|------------------------|
| ax-fg          | 3/6 | click, drag, type | foreground (8 actions) |
| ax-bg          | 2/6 | click, type       | **3/8 stole** (set_value, type, press-key) |
| px-fg      | 1/5 | double-click      | foreground (7 actions) |
| px-bg      | 1/5 | double-click      | **6/7 stole** |
| px-desktop | 1/2 | click             | foreground (4 actions) |

**Headline — Linux Electron breaks the no-foreground contract, but far less than Windows
Electron.** Windows Electron stole **7/8** in ax-bg (Chromium self-foregrounds on nearly every
action). Linux Electron steals **3/8 in ax-bg** — only the keyboard/focus actions (`set_value`,
`type`, `press-key`) pull the window frontmost via the X focus path; the AX pointer actions
(click/double/right/drag/scroll) all **held**. In **px-bg** it steals **6/7** (pixel dispatch
is coordinate-injection that foregrounds Chromium). Effect coverage mirrors the documented Linux
synthetic-input limit: AX `click`/`type` land; AX `double_click`/`right_click`/`drag`/`scroll` and
`set_value` are no-ops on the Chromium controls via AT-SPI, while **pixel** `double_click` *does*
fire on the click-target (`last_action=double_click`, frame-verified in px-fg/bg). All 5 modes
frame-verified; saved as `linux-electron-{ax-fg,ax-bg,px-fg,px-bg,px-desktop}.mp4`.

## Agent-cursor centering (user-reported)
The agent cursor lands off-center on the checkbox. Root cause: the WPF checkbox's AX
BoundingRectangle is the **full-width row** (≈803px), so its center is mid-row. Verified that
UIA `GetClickablePoint` does **not** help — it either fails (slider) or returns the same rect
center (text box). So this is **not a driver defect**: the bounds are reported faithfully and the
element action works regardless of cursor position. The visual is a function of the control's
geometry (a stretched CheckBox). Most-honest fix would be the harness laying the checkbox out at
content width (how real apps render it); a driver left-bias heuristic would be unreliable.

## Cross-toolkit coverage matrix (Windows, ax-bg)

Same recorder (parameterized by `-Toolkit`), same 8-action plan, one ax-bg pass each. This is the
high-signal output: which cua-driver actions actually LAND per toolkit, and whether the no-foreground
contract holds. (right-click + scroll are ✗ across the board here: the harnesses either lack those
controls or they're off-screen at the 556px layout — see the off-screen guard above.)

| Toolkit  | effects landed | landed actions                          | focus contract (stole) | note |
|----------|---------------|------------------------------------------|------------------------|------|
| WPF      | **5/7** | click, double, drag, set_value, type     | 2/8 | fullest harness |
| WinUI3   | **3/7** | left-click(checkbox), scroll, set_value  | 0/7 | re-recorded with parity controls; double/right/drag don't land (see WinUI3 dispatch below) |
| WebView2 | **4/7** | double, right, drag, set_value           | 0/7 | re-recorded; left-click(checkbox) + scroll are honest no-ops (checkbox below the fold, web won't scroll in ax mode) |
| Electron | **5/7** | click, double, drag, set_value, type     | **7/8** | **Electron/Chromium self-foregrounds → no-foreground contract VIOLATED** |

**Headline:** the no-foreground contract holds on WPF / WinUI3 / WebView2, but **breaks on Electron**
(7/8 actions stole focus — Chromium foregrounds itself; consistent with the #1984 dispatch note in the
code). The verifier reads each toolkit's own status labels (WPF/WinUI3: UIAutomation; WebView2/Electron:
Chromium web-AX text via a substring window-title match because their titles carry a `[cdp=NNNN]` suffix).

WinUI3 + WebView2 were **re-recorded** after the parity controls landed (frame-verified). Recorder fixes
that made WebView2 work at all: the web DOM only surfaces when `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=
--force-renderer-accessibility` is set (otherwise `get_window_state` returns only the chrome frame →
every web action resolves to nothing → the prior SIZE=0/empty-MP4 run); and the daemon's ffmpeg probe
missed the binary because the recorder runs as a different user than the one WinGet installed it under, so
ffmpeg must be on PATH before `serve`. Recording on the VM also requires Session 2 attached/active
(`tscon … /dest:console`) — a disconnected session blanks GPU content and fails `gdigrab`.

**WinUI3 double/right-click — confirmed not driver-fixable via the WPF path.** AX `double_click`/
`right_click` on the WinUI3 click-target produce `last_action=none, clicks=0` (no Click/DoubleTapped/
RightTapped), while the *identical* actions land on WebView2 — so the harness wiring is correct; the gap
is WinUI3-specific dispatch. Matches the `platform-windows/src/input/dispatch.rs` note: routing WinUI3
through the WPF synthetic-pen/coordinate path neither lands double/right NOR holds the contract (measured:
regressed ax-bg 0/8→8/8 stolen). Single left-click works via UIA Invoke. A real fix needs a WinUI3-specific
input path targeting the composition input-site (DirectComposition/ContentIsland InputSite), not
PostMessage-to-top-HWND; the same gap explains WinUI3 drag-slider being a no-op.

Evidence: `matrix-{winui3,webview2,electron}-ax-bg.mp4` (+ the WPF set) on the Desktop.

## Driver fixes shipped this round (cross-platform)

- **Windows — cached UIA element use-after-free** (`platform-windows`): the element cache handed out a bare
  COM pointer; under concurrent sessions a `get_window_state` snapshot-replace could `Release` it mid-action
  (click/type/set_value) → daemon crash. Now a `RetainedElement` guard `AddRef`s under the cache lock and
  `Release`s on drop — the Windows port of the macOS #1796 retain-under-lock fix. Compile-verified.
- **Linux — GTK left-click + value-only widgets** (`platform-linux`): pixel left-clicks now land via AT-SPI
  hit-test + `doAction` (GTK drops synthetic X11 events; XTEST core events don't reach its XInput2 path) —
  without stealing focus. Sliders/scroll bars (Value interface, no Action) now surface in `get_window_state`
  (`is_indexable = actions || has_value`), so `set_value` drives them. Verified on the Linux VM.
- **macOS — numeric `set_value`** (`platform-macos`): CFNumber write for NSSlider, AXIncrement/AXDecrement
  stepping fallback for SwiftUI sliders. Verified live (AppKit 0→50, SwiftUI 0→50).

## Legacy modal popups (WPF) — driver CAN open + list them

Confirmed: a **background `click` via UIA Invoke** fires the harness's popup buttons *even when they
are off-screen* (y≈876–1026 on the 768px display), and `list_windows` then enumerates the dialogs:
`Open MessageBox` → **"Harness MessageBox"**, `Open Owned Window` → **"Harness Owned Popup"**,
`Open Layered Popup` → **"Harness Layered Popup"**.

This briefly regressed: the first cut of the off-screen guard (above) sat *before* the dispatch
branch and so blocked the UIA-Invoke path too (which needs no coordinates). **Fix:** the guard now
applies only to the coordinate-delivery paths (foreground SendInput tap + background coordinate
injection); UIA Invoke runs unguarded, so opening a modal from an off-screen button works again.

## macOS (AppKit) — 5 modes, contract HOLDS

Recorded on the macOS host (AppKit harness, ScreenCaptureKit). TCC already granted. Saved locally
only (`macos-appkit-*.mp4`) — these record a personal screen, not uploaded.

| mode | effects | focus contract |
|------|---------|----------------|
| ax-bg | 5/7 | **0/8 stole** |
| px-bg | 4/6 | **0/7 stole** |
| ax-fg / px-fg / px-desktop | 5/7 / 4/6 / 2/3 | foreground |

Headline: in **px-bg**, pixel left-click/double/drag landed on the harness while Chrome stayed
frontmost — **0 focus steals**; the no-foreground contract holds on macOS for both AX and pixel
dispatch. Honest macOS gaps surfaced: the NSView click-target isn't in the AX tree (needs pixels);
pixel right-click never fires `rightMouseDown`; pixel scroll doesn't move `NSScrollView` (element
scroll does); NSButton ignores synthetic pixel clicks (AX `element_index` targets it); and there's
no true window-less screen click on macOS (`click` requires `pid`; pixel dispatch is window-anchored).
Two driver gotchas to flag: **`end_session` poisons a reused session id** (subsequent actions
silently no-op), and AppKit **window height drifts between launches** (store targets as window-local
points, convert to live screenshot px).

## Control-parity pass (all surfaces → WPF baseline)

Brought every harness up to the WPF 6-control set (checkbox, click-target L/R/double,
slider, scroll-target, text-input, context-menu) so the same 8-action matrix is
exercisable everywhere; `shared/scenarios.json` + `shared/web/index.html` are the
single source of truth. Harness DOM/UI parity landed for GTK3, WinUI3, the shared web
(WebView2/Electron/WKWebView), and macOS AppKit/SwiftUI.

**scroll_target — Linux Electron (re-recorded, frame-verified).** The shared web
`scroll-tall` clipped region now resolves on Linux Electron in all 5 modes via the
Chromium web-AX tree; the recorder aims the scroll at that section and checks a real
`scroll_offset=` delta. **Result: scroll is a no-op in every mode** (`scroll_offset`
stays 0) — the same AT-SPI synthetic-input limit as the other Linux pointer actions
(double/right/drag), for both the element_index and the pixel/vision path. Headline
intact: Linux Electron ax-bg still **stole 3/8** (scroll is now a counted failure, not
`na`). 5 mp4s re-encoded + overwritten (`linux-electron-*.mp4`).

**WebView2 (Windows) — harness restored, full-parity recording deferred.** The shared
web DOM (with scroll-target) is deployed; a clean self-contained re-publish of the
harness was needed after an in-place publish corrupted the bundle (window stopped
resolving). With the clean bundle the window resolves again and `sld`/`txt` map, but the
new click-target/checkbox/scroll/context controls need the WebView2 recorder's resolver
extended (as was done for Linux Electron) before a full-parity recording is meaningful —
deferred as low marginal value: it would only re-confirm the captured headline (Windows
Chromium **steals 7/8**, the matrix-webview2-ax-bg.mp4 evidence).

## macOS — 4 surfaces + numeric-set_value driver fix

Surfaces recorded on the macOS host (local-only, personal screen — never uploaded):
**AppKit, SwiftUI, Electron, WKWebView**, 5 modes each. WKWebView (Apple WebKit, the
native analogue of WebView2) **holds the contract** — confirming the Windows Electron/
WebView2 steal is specific to Windows-Chromium, not WebKit-on-macOS.

**Driver fix shipped (this branch, `platform-macos`):** `set_value` now writes a
**CFNumber** for numeric `AXValue` controls (NSSlider/NSStepper reject a CFString);
falls back to CFString for text fields.

### Live host verification (AppKit + SwiftUI + WKWebView, against the running driver)

The CFNumber fix was verified live after a `install-local` reinstall (the new daemon is
the **Jun-27 post-fix** binary; the installer re-signed the `CuaDriver.app` bundle
ad-hoc and re-granted TCC — `permissions status` reports Accessibility + Screen
Recording both ✅). The daemon was driven directly via the `cua-driver call` CLI.

- **set_value CFNumber fix — VERIFIED on AppKit NSSlider.** `set_value(AXSlider, "50")`
  on the AppKit harness now **succeeds** (`✅ Set AXValue on [5] AXSlider`) and
  `slider_value` goes **0 → 50** (frame-verified — thumb at midpoint). This is the control
  the fix targets; the CFNumber write lands where a CFString write was rejected.
- **SwiftUI slider — NOT a value-type problem; `AXValue` is unsettable there.** The
  SwiftUI `AXSlider` exposes only `actions=[increment,decrement]` and rejects *any*
  `AXValue` write (CFNumber and the CFString fallback both return `-25200`). So the
  CFNumber fix cannot help SwiftUI sliders — driving them needs repeated AXIncrement/
  AXDecrement (future driver work), not a value write. (Comment in `set_number_attr`
  notes `-25200` as well as `-25201`.)
- **SwiftUI harness — full control parity confirmed.** `get_window_state` exposes all
  six WPF-parity controls as actionable AX elements (text field, click-target,
  `AXSlider sld-value`, `AXCheckBox "I agree"`, context-menu button, scroll region) and
  the status labels (`counter=`, `mirror=`, `last_action=`, `clicks=`, `slider_value=`,
  `agreed=`, `menu_action=`, `scroll_offset=`) render as text nodes in `tree_markdown`
  for the verifier. An AX checkbox press flipped `agreed=false → true` (frame-verified).
- **WKWebView web-surface scroll — page works, nested overflow div is a no-op.** The
  driver's keystroke `scroll` (PageDown on the `AXWebArea`) scrolls the whole page a full
  page down (frame-verified). But the nested `scroll-tall` overflow div stays at
  `scroll_offset=0`: keystroke-scroll only drives the focused/page scroller, and a CSS
  `overflow:auto` div without `tabindex` never takes keyboard focus. Same root cause as the
  Linux Electron inner-div no-op; a pixel-wheel scroll (not in the current `scroll` tool)
  would be required to drive an arbitrary overflow container.

The two macOS gotchas above (`end_session` reuse poisoning, AppKit window-height drift)
were also filed for the driver.

## Deliverables
10 verified recordings (5 WPF + 5 GTK3) + index, published to the team demo blob
(URL tracked internally — not linked here to keep it out of the public repo).
Additional local-only sets (not uploaded): macOS AppKit/SwiftUI/Electron/WKWebView,
Linux Electron (with scroll-target), and the Windows cross-toolkit matrix.
