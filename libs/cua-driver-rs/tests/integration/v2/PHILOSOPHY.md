# cua-driver v2 Integration Test Philosophy

## Core Principle

**Tests exercise the public MCP tool interface only.**

The driver exposes a small, stable interface:

| Tool | Purpose |
|------|---------|
| `get_window_state` | Snapshot: AX/UIA tree + screenshot |
| `click` | Click at element or pixel |
| `type_text` | Type a string |
| `type_text_chars` | Type via keystroke synthesis |
| `press_key` | Send a named key |
| `hotkey` | Send a key combo |
| `scroll` | Scroll wheel |
| `drag` | Mouse drag |

Tests **never** reach for `browser_eval`, CDP, Apple Events, or any other
internal mechanism. The driver is a black box that delivers the action via
whatever technique works best for the target app. Tests verify the
*observable outcome*, not the delivery path.

---

## Canonical Test Pattern

Every test follows the same four-step loop:

```
before = get_window_state(pid, wid)          # 1. snapshot
element = find(before.tree, ...)             # 2. locate
act(pid, wid, element or coords)             # 3. act  (click / type / key)
after = get_window_state(pid, wid)           # 4. assert
assert <state changed in after>
```

- **Snapshot first** — always read the AX tree before acting so element
  indices are fresh.
- **Act once** — do not poll between action and assertion. One `get_window_state`
  after the action is enough (add a short `sleep` if the app needs to settle).
- **Assert observable state** — AX tree values, window titles, screenshot diffs.
  Never assert internal driver behaviour.

---

## UX Invariants (checked throughout every test)

A background thread polls at 5 ms and records any violation of:

| Signal | Violation |
|--------|-----------|
| Real cursor position (`NSEvent.mouseLocation`) | Moved > 4 px from baseline |
| Frontmost app PID (`NSWorkspace.frontmostApplication`) | Changed from sentinel |
| Agent overlay z-order (`CGWindowListCopyWindowInfo`) | cua-driver layer ≥ frontmost non-agent window layer |

The `ux_guard` pytest fixture starts the monitor before each test and calls
`monitor.assert_clean()` in teardown. If any violation was recorded the test
fails with a list of every incident.

---

## App-specific Oracle Strategy

| App | Primary oracle | Cross-check |
|-----|---------------|-------------|
| Safari | AX tree text values | — |
| Chrome | AX tree text values | — |
| Electron | AX tree text values | HTTP API on port 6769 |
| Tauri | AX tree text values | HTTP API on port 6769 |
| Blender | Screenshot + NCC template match | AX tree window title |
| Calculator | AX tree static text | — |

For browser apps the test page (`assets/test_page.html`) provides a
self-contained oracle: every interactive element reflects its state back into
the DOM (button click counter, text field value, checkbox label, select
label, textarea content). The AX tree exposes those DOM values directly.

---

## Canvas Test Pattern

Canvas pixels are not in the AX tree. Verify canvas interaction by screenshot
diff:

```python
before = get_window_state(pid, wid)
before_crop = cv.crop(cv.decode(before.screenshot), canvas_bbox)
click(pid, wid, x=target_x, y=target_y)
after = get_window_state(pid, wid)
after_crop = cv.crop(cv.decode(after.screenshot), canvas_bbox)
assert cv.diff_ratio(before_crop, after_crop) > 0.001
```

---

## Tauri / Electron HTTP API

The HTTP API is a **cross-check**, not the primary assertion:

```python
# Primary: AX tree
after = get_window_state(tauri_pid, wid)
assert ax_value(after.tree, field_idx) == "tauri test"

# Cross-check: HTTP event log
events = requests.get("http://localhost:6769/events").json()
assert any(e["type"] == "key_down" for e in events)
```

---

## Test File Structure

```
v2/
  PHILOSOPHY.md         ← this file
  conftest.py           ← pytest fixtures (driver, ux_guard, sentinel, html_server, ...)
  harness/
    __init__.py
    monitor.py          ← UXMonitor (5 ms PyObjC thread)
    driver.py           ← thin typed wrapper around DriverClient
    tree.py             ← tree_markdown parser + find / value / bbox helpers
    cv.py               ← PIL decode / crop / diff / template-match utilities
  assets/
    test_page.html      ← all-elements test page for browser tests
    blender/
      bboxes.json       ← pre-measured bounding boxes for template targets
  test_safari.py
  test_chrome.py
  test_electron.py
  test_tauri.py
  test_blender.py
```

---

## Running

```bash
cd tests/integration/v2
export CUA_DRIVER_BINARY=../../../target/debug/cua-driver

# Single file
python3 -m pytest test_safari.py -v

# Full suite
python3 -m pytest -v
```
