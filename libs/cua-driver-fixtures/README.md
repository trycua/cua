# cua-driver-fixtures

Canonical HTML test fixtures for cua-driver (Swift / macOS) and
cua-driver-rs (Rust / Windows + Linux + macOS). One source of truth; both
ports' integration tests reference these files via relative symlinks from
their existing `Tests/integration/fixtures/` and `assets/` trees.

## Files

| File | Purpose |
|------|---------|
| `interactive.html` | 31-line minimal click + type-mirror harness. Stable IDs: `#counter`, `#clicker`, `#textbox`, `#typed`. Used by `test_background_focus.py` (both ports) — opened via `file://`. |
| `form_all_inputs.html` | Every HTML5 input type (text, password, email, number, tel, textarea, select, checkbox, radio, range, date, color). Submit handler stores result in `window._submitted`; live read via `getFieldValues()`. Used by `test_drag_slider_delivery.py`, `test_hermes_form_fill*.py`. |
| `test_page.html` | Richer Safari/Chrome/Electron harness — button counter, text input, checkbox, dropdown, textarea, link, canvas. Served via local `html_server` HTTP fixture. Used by `test_chrome.py`, `test_safari.py`, `test_electron.py` (both ports). |
| `gesture_panels.html` | **New** — extends the same ID-convention style as `test_page.html` with panels for the four gestures not currently covered by the v2 harness: hotkey + modifier-state propagation, pixel-coord pinpoint accuracy, drag-and-drop event sequence, scroll position. See **Why** below. |

All four files are vanilla HTML — no external deps, no build step. Open
directly in any browser:

```bash
open libs/cua-driver-fixtures/test_page.html
```

## Where each port references the canonical copy

Existing port paths are preserved as symlinks. Tests that read these
paths (`os.path.join(_THIS_DIR, "fixtures", "interactive.html")`,
`f"{html_server}/test_page.html"`) work unchanged.

```
libs/cua-driver/Tests/integration/
├── fixtures/
│   ├── interactive.html      → ../../../../cua-driver-fixtures/interactive.html
│   └── form_all_inputs.html  → ../../../../cua-driver-fixtures/form_all_inputs.html
└── assets/
    └── test_page.html        → ../../../../cua-driver-fixtures/test_page.html

libs/cua-driver/rust/tests/integration/
├── fixtures/
│   └── interactive.html      → ../../../../cua-driver-fixtures/interactive.html
└── v2/assets/
    └── test_page.html        → ../../../../../cua-driver-fixtures/test_page.html
```

Edits to a fixture now propagate to **both ports** in a single commit.
Drift is impossible by construction.

## Why `gesture_panels.html` was added

`test_page.html` covers Safari/Chrome/Electron interaction surfaces well
but doesn't probe four gestures that the Windows cua-driver-rs stress
test (May 2026) showed need explicit harness coverage:

1. **Hotkey + modifier-state propagation** (`#hotkey-focus` + `#hotkey-status`):
   PostMessage-based hotkey paths inject keystrokes without updating the
   OS modifier state, so the page-level `KeyboardEvent.ctrlKey` reads
   `false` even after a routed `ctrl+s`. SendInput updates `GetKeyState`,
   so the same event reads `ctrl=true`. The status div prints
   `ctrl=true|false` directly — single assertion per routing path.

2. **Pixel-coord pinpoint** (`#coord-area` + `#coord-status`): a known
   target at viewport `(60, 60)` inside the box; status shows the
   distance from the target in pixels. Asserting `dist < 2` is a clean
   precision test for coordinate-routed clicks.

3. **Drag-and-drop sequence** (`#drag-source` + `#drag-target` +
   `#drag-status`): the page records each HTML5 drag event in order so
   tests can assert the full `dragstart → dragover → drop` chain reached
   the page (not just that the driver moved the cursor).

4. **Scroll position** (`#scroll-area` + `#scroll-status`): a scrollable
   container whose `scrollTop` is printed live — tests can assert scroll
   direction + amount after a routed scroll gesture.

Like `test_page.html`, each panel exposes a global accessor:
`window.getGesturePanelState()` returns a JSON snapshot of all four
panels' state. Tests can poll it via `page` with
`action: "execute_javascript"` (when running with Chromium's
`--remote-debugging-port`) or read the visible status divs via UIA / OCR.

### Known browser-coverage gaps (May 2026)

When driving Chromium-based browsers (Edge / Chrome / Electron) on
Windows, the integration tests should be aware of:

- **GPU-composited content blanks PrintWindow.** `cua-driver screenshot`
  captures the browser chrome + a white body. Workaround: launch the
  target browser with `--remote-debugging-port=<port>` and use
  `page` (`action: "execute_javascript"`) for both inputs and verification.
- **Chromium auto-disables full a11y** unless an AT-grade client is
  detected. UIA on the rendered DOM returns empty until the page is
  flagged accessibility-active. Workaround: enable Chromium's
  `--force-renderer-accessibility` flag (or rely on `--remote-debugging-port`).
- **`hotkey` SendInput injection requires the daemon at UIAccess
  integrity.** Otherwise `SetForegroundWindow` is rejected and the
  injected events land on the wrong window — `SendInput inserted only 0
  of 4 events`. Workaround: run `cua-driver` via the
  `cua-driver-uia.exe` worker (PR #1593) which carries the manifest.

These are environmental requirements for any DOM-level verification of
gesture behavior in a Chromium target on Windows; they don't affect the
canonical fixtures, only how they're driven.

## Adding new fixtures

1. Add the new HTML file directly to this dir
2. Document it in the table above (purpose + stable IDs + who uses it)
3. Add a relative symlink under whichever port(s) need it at the path
   their tests already use
4. Commit the new file + symlink(s) in the same PR

Do **not** create copies under each port's tree — symlink-only.

## License

Same as the parent project (MIT, see `LICENSE.md`).
