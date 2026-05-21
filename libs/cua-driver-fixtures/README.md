# cua-driver-fixtures

Shared test fixtures for cua-driver (Swift / macOS) and cua-driver-rs (Rust /
Windows + Linux + macOS). One source of truth; both ports' integration tests
reference these files.

## What's here

- **`gesture-playground.html`** — single-page interactive harness covering
  every cua-driver gesture: click (left/right/middle/double), type_text,
  press_key, hotkey (with modifier-state propagation check), pixel-coord
  clicks, drag-and-drop, scroll, custom canvas mousedown/move/up, and a form
  with every HTML input type. State is exposed as a JSON dump that tests can
  read back via `get_window_state` + DOM reads or via screenshot.

  Each panel has a `data-test="<id>"` attribute for stable test targeting.

  Open in any browser:
  ```bash
  open libs/cua-driver-fixtures/gesture-playground.html         # macOS
  start libs\cua-driver-fixtures\gesture-playground.html        # Windows
  xdg-open libs/cua-driver-fixtures/gesture-playground.html     # Linux
  ```

- **`interactive.html`** (planned) — keep as a thin redirect to
  `gesture-playground.html` for backward compatibility with existing
  fixtures that hard-code the filename. Both ports' `Tests/integration/`
  trees previously had their own copy; canonical source is now here.

## Migration plan

1. ✅ Add this dir with the new gesture-playground.html
2. Update `libs/cua-driver/Tests/integration/conftest.py` to resolve
   fixtures from this shared dir (with a fallback to the local copy for
   in-flight branches)
3. Same for `libs/cua-driver-rs/tests/integration/conftest.py` and the v2
   harness
4. After both ports compile + tests pass against the shared dir, remove the
   duplicated copies under each port

## State exposed by `gesture-playground.html`

The page maintains a single `state` object updated on every interaction.
A `<pre id="state-dump">` panel renders it as JSON; tests read back via
either:

- `cua-driver get_window_state` + filter on the JSON text inside `#state-dump`
- DOM read via the browser's accessibility tree (each panel has a stable
  `data-test` attribute and inner elements have stable `id`s)
- `cua-driver screenshot` + OCR (last resort for visual regression)

### State fields

| Panel | State field | Updated on |
|-------|-------------|------------|
| `1 · click counter` | `state.counter` (int) | `click` on `#counter-btn` |
| `2 · click types` | `state.multi` (`{type, at}`) | left/right/middle/dbl `click` |
| `3 · type_text mirror` | `state.text` (str) | `input` event on `#textbox` |
| `4 · keyboard` | `state.key` (`{key, code, ctrl, alt, shift, meta}`) | `keydown` on `#keyfocus` |
| `5 · pixel-coord click` | `state.coord` (`[x, y]`), `state.coord_dist` (px from (60,60)) | `click` on `#coord-area` |
| `6 · drag` | `state.drag` (`{dropped_from, dropped_at}`) | `drop` on `#drag-target` |
| `7 · scroll` | `state.scroll` (int, px) | `scroll` on `#scroll-area` |
| `8 · canvas` | `state.canvas` (`{type, x, y}`) | `mousedown/move/up` on `#canvas` |
| `9 · form` | `state.form` (form data) | `submit` on `#big-form` |

### Why this matters for cua-driver verification

- **Modifier-state propagation**: panel 4 reports `ctrl=true/false` from the
  keyboard event. After `hotkey ["ctrl", "s"]`, you can verify `state.key.ctrl
  === true` — that proves SendInput is correctly updating GetKeyState (the
  PostMessage-based path doesn't).
- **Pixel accuracy**: panel 5 reports distance from the ring center (60, 60).
  After `click(x=60, y=60)`, distance should be ~0. After `click(x=61, y=58)`
  it should be ~2. Lets you assert pixel-level click precision.
- **Custom canvas vs DOM events**: panel 8 captures `mousedown` events on a
  canvas. PostMessage WM_LBUTTONDOWN typically does NOT fire `mousedown` on
  HTML5 canvases; SendInput does. Same model as Audacity timeline / GIMP /
  Blender viewport behavior.
- **Drag**: panel 6 verifies the full HTML5 drag-and-drop sequence
  (`dragstart` → `dragover` → `drop`). Useful for asserting cua-driver's
  `drag` tool fires all three.
- **Scroll**: panel 7's scroll position is a simple int the test can read
  back to verify `scroll` direction + amount.

## Adding new fixtures

If you need a fixture for a specific bug or gesture not covered here:

1. Add it under `libs/cua-driver-fixtures/` (NOT under
   `libs/cua-driver/Tests/integration/fixtures/` or
   `libs/cua-driver-rs/tests/integration/fixtures/` — those dirs are
   deprecated, see Migration plan above)
2. Document the state shape it exposes in this README
3. Reference from both Swift + Rust integration test trees

## License

Same as the parent project (MIT, see `LICENSE.md`).
