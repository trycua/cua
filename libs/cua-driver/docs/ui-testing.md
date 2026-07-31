# Background UI testing

`cua_driver.testing` is a macOS UI test layer for apps that must remain behind
the window a person is using. It supplies app lifecycle, stable element
queries, polling waits, assertions, and failure artifacts on top of Cua
Driver's accessibility and window-capture tools.

## Install

Install the Python test extra, then grant Accessibility and Screen Recording
to the process macOS identifies when pytest starts. For local runs this is
usually the terminal or Python host:

```bash
pip install "cua-driver[uitest]"
```

The test layer loads Cua Driver in the pytest process and binds one standard,
window-scoped Cua session. That binding keeps launch ownership, actions, and
cleanup on the same session.

## Write a test

```python
import pytest

from cua_driver.testing import CuaTestSession


@pytest.mark.asyncio
async def test_counter() -> None:
    async with CuaTestSession.create() as cua:
        app = await cua.launch(
            bundle_id="com.example.Counter",
            window_title="Counter",
        )

        await app.buttons.by_id("btn-increment").tap()
        await app.wait_for_text("counter=1")
```

`launch()` always requests a new application instance, giving the test its own
PID and window. The session kills only processes it launched. `attach(pid=...)`
never kills the attached process.

By default, the session reads every active display and centers the test window
on a non-main display before the first action. It prefers a built-in secondary
display, ignores mirrored displays, and verifies the resulting window bounds.
The requested secondary origin is passed into `launch_app`, which moves the
window as soon as WindowServer publishes it, before launch focus suppression
finishes. The runner centers and verifies the window again after launch. Both
moves use the accessibility API, so they do not activate or raise the app.
Machines with one display leave the window in place. Use
`CuaTestSession.create(placement="unchanged")` only when the test owns its
screen arrangement. Pass `window_title=` for apps with splash screens or
multiple top-level windows so early placement waits for the intended window.

## Select elements

Use accessibility identifiers for tests that must survive localization and UI
copy changes:

```python
button = app.buttons.by_id("btn-increment")
field = app.text_fields.by_id("txt-name")
```

Exact labels are useful for prototypes:

```python
button = app.buttons["Increment"]
button = app.buttons.labeled("Increment")
```

Available role collections include `buttons`, `checkboxes`, `groups`, `links`,
`menus`, `menu_items`, `radio_buttons`, `rows`, `sliders`, `tables`,
`text_areas`, and `text_fields`. `app.elements` searches every actionable
role. A label query that matches more than one element fails and prints the
candidate roles, labels, identifiers, and values.

## Act and wait

```python
await app.buttons.by_id("save").tap()
await app.text_fields.by_id("name").type_text("Ada")
await app.text_fields.by_id("name").set_value("Grace")
await app.text_fields.by_id("name").wait_for_value("Grace")
await app.wait_for_text("Saved")
await app.buttons.by_id("spinner").wait_for_disappearance()
```

Each action obtains a fresh window snapshot and acts through its element token.
Polling waits use fresh snapshots, so stale element handles do not leak between
UI updates.

## Background-only contract

The test layer has no foreground escape hatch. It does not expose
`bring_to_front`, screen-wide clicks, real cursor movement, or
`delivery_mode: "foreground"`. Actions always include the target PID, window
ID, and element token. Clicks and keyboard input explicitly request background
delivery; value changes use the target control's accessibility action. Before
and after each action, the runner reads the active app and WindowServer order.
It fails if the target is already active, becomes active, rises above a
visible window from the active app, or the active app cannot be identified.
The same order check applies when the test window is parked on another display.
Launch and attach also fail if placement changes the active app or the
requested position cannot be verified.

Some custom-rendered controls do not expose an accessibility action. Cua
Driver returns a structured refusal for those controls. The test layer records
the refusal and fails instead of escalating to a foreground pixel action.

## Failure artifacts

Timeouts, driver refusals, and foreground violations create a timestamped
directory under `test-results/cua-ui/` containing:

- `window.png`, the target window capture
- `tree.txt`, the accessibility tree
- `snapshot.json`, the structured elements and metadata

Pass `artifacts_dir=` to `CuaTestSession.create()` to place them in a CI
artifact directory.
