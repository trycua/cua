# Cua Driver action and icon catalog

**Status:** Semantic source catalog for cursor theme profile v2

**Source:** Cua Driver public contract and platform tool registries

**Scope:** Public MCP/CLI-callable tools and their meaningful runtime action
variants. Internal implementation helpers are excluded.

## What is being cataloged

A public tool name is not always a complete action identity.

For example, `click` may resolve to:

- an accessibility action or a pixel action;
- background or foreground delivery;
- a window target or the full desktop;
- left, right, or middle button;
- one, two, or three clicks.

The icon system should therefore resolve a call into:

```text
surface.intent.target.delivery.qualifiers
```

Examples:

```text
native.click.px.background.left.single
native.click.px.foreground.left.single
native.click.ax.background.press
native.scroll.desktop.down.page
browser.click.trusted.ref
browser.pointer.drag.dom_event
```

Foreground and background are separate resolved actions, but they should not
need unrelated illustrations. A maintainable asset system uses:

1. a base intent glyph or animation (`click`, `type`, `scroll`, `drag`);
2. a targeting treatment (`ax`, `px`, `desktop`, `page`, `browser`);
3. a host-rendered delivery chip (`background`, `foreground`);
4. a small qualifier where it changes the visible gesture (`right`, `double`,
   `down`, `page`, and so on).

## Canonical variant vocabulary

### Targeting

| Token             | Meaning                                               | How it is selected                        |
| ----------------- | ----------------------------------------------------- | ----------------------------------------- |
| `ax`              | Accessibility-addressed element action                | `element_token` or `element_index`        |
| `px`              | Window-local pixel action                             | Window target plus `x`/`y` coordinates    |
| `focused`         | Acts on the target's currently focused control        | No element or point supplied              |
| `desktop`         | Screen-absolute action                                | `scope:"desktop"` without a window target |
| `page`            | Legacy page/DOM action                                | A `page[action=…]` operation              |
| `browser_trusted` | Typed browser action through trusted CDP Input events | `input_route:"trusted"` or omitted        |
| `browser_dom`     | Explicit synthetic DOM event                          | `input_route:"dom_event"`                 |

### Delivery

| Token                   | Meaning                                                             |
| ----------------------- | ------------------------------------------------------------------- |
| `background`            | Targeted delivery without raising or fronting the target            |
| `foreground`            | Briefly front target, act, then restore the previous foreground app |
| `persistent_foreground` | `bring_to_front`; target stays foreground                           |
| `fixed_background`      | Browser or Linux low-level operation whose contract never fronts    |
| `none`                  | Read-only or lifecycle operation with no input-delivery posture     |

The desktop input family exposing `delivery_mode` is:

```text
click
double_click
right_click
drag
type_text
press_key
hotkey
scroll
```

`browser_dialog` also exposes background/foreground delivery for `accept` and
`dismiss`. Typed browser pointer/click operations never foreground the browser.

### Gesture qualifiers

| Dimension           | Values                                                    |
| ------------------- | --------------------------------------------------------- |
| Button              | `left`, `right`, `middle`                                 |
| Click count         | `single`, `double`, `triple`                              |
| AX click action     | `press`, `show_menu`, `pick`, `confirm`, `cancel`, `open` |
| Scroll direction    | `up`, `down`, `left`, `right`                             |
| Scroll unit         | `line`, `page`                                            |
| Browser input route | `trusted`, `dom_event`                                    |
| Browser typing      | `insert_text`, `keystrokes`                               |

Modifiers such as Command, Control, Alt/Option, and Shift belong in the
animation payload or label. They do not require separate base icons.

## Resolved native input actions

These are the action identities that should drive cursor animations.
Brace-delimited values are finite variants, not literal asset names.

| Public tool      | Resolved action keys                                                                       |
| ---------------- | ------------------------------------------------------------------------------------------ |
| `click`          | `native.click.ax.{background\|foreground}.{press\|show_menu\|pick\|confirm\|cancel\|open}` |
| `click`          | `native.click.px.{background\|foreground}.{left\|right\|middle}.{single\|double\|triple}`  |
| `click`          | `native.click.desktop.{left\|right\|middle}.{single\|double\|triple}`                      |
| `double_click`   | `native.double_click.{ax\|px}.{background\|foreground}`                                    |
| `right_click`    | `native.right_click.{ax\|px}.{background\|foreground}`                                     |
| `drag`           | `native.drag.px.{background\|foreground}.{left\|right\|middle}`                            |
| `drag`           | `native.drag.desktop.{left\|right\|middle}`                                                |
| `type_text`      | `native.type_text.{ax\|px\|focused}.{background\|foreground}`                              |
| `type_text`      | `native.type_text.desktop`                                                                 |
| `press_key`      | `native.press_key.{ax\|px\|focused}.{background\|foreground}`                              |
| `press_key`      | `native.press_key.desktop`                                                                 |
| `hotkey`         | `native.hotkey.{px\|focused}.{background\|foreground}`                                     |
| `hotkey`         | `native.hotkey.desktop`                                                                    |
| `set_value`      | `native.set_value.ax.background`                                                           |
| `scroll`         | `native.scroll.{ax\|px}.{background\|foreground}.{up\|down\|left\|right}.{line\|page}`     |
| `scroll`         | `native.scroll.desktop.{up\|down\|left\|right}.{line\|page}`                               |
| `move_cursor`    | `agent_cursor.move.window`, `native.pointer.move.desktop`                                  |
| `zoom`           | `native.zoom.window`                                                                       |
| `bring_to_front` | `native.window.persistent_foreground`                                                      |

Do not generate every Cartesian product as a fully independent illustration.
For example, all `native.scroll.*` actions share one scroll animation.
Delivery and targeting are composable host-rendered badge chips rather than
theme animations.

## Page compound actions

The public `page` tool contains seven operations:

| Resolved action key                   | Effect                                    | Platform note                                              |
| ------------------------------------- | ----------------------------------------- | ---------------------------------------------------------- |
| `page.get_text`                       | Read visible page text                    | Cross-platform                                             |
| `page.query_dom`                      | Query elements with a CSS selector        | Cross-platform                                             |
| `page.execute_javascript`             | Execute JavaScript                        | Cross-platform where a supported page backend is available |
| `page.click_element`                  | Click a CSS-selected element              | Cross-platform                                             |
| `page.insert_text`                    | Insert text at current DOM focus          | macOS implementation currently documented                  |
| `page.type_keystrokes`                | Dispatch durable per-character key events | macOS implementation currently documented                  |
| `page.enable_javascript_apple_events` | Enable Safari JavaScript automation       | macOS only                                                 |

## Typed browser actions

| Public tool               | Resolved action keys                                                                                                           |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `get_browser_state`       | `browser.bind`, `browser.snapshot.dom_refs_v1`, `browser.snapshot.semantic_v2`                                                 |
| `browser_prepare`         | `browser.prepare.detect`, `browser.prepare.isolated_new`, `browser.prepare.isolated_named`, `browser.prepare.existing_profile` |
| `browser_navigate`        | `browser.navigate`                                                                                                             |
| `browser_click`           | `browser.click.trusted.{ref\|viewport}`, `browser.click.dom_event.ref`                                                         |
| `browser_type`            | `browser.type.insert_text`, `browser.type.keystrokes`                                                                          |
| `browser_dialog`          | `browser.dialog.inspect`                                                                                                       |
| `browser_dialog`          | `browser.dialog.accept.{background\|foreground}`                                                                               |
| `browser_dialog`          | `browser.dialog.dismiss.{background\|foreground}`                                                                              |
| `browser_set_input_files` | `browser.files.set_input`                                                                                                      |
| `browser_download`        | `browser.download`                                                                                                             |
| `browser_pointer`         | `browser.pointer.{hover\|right_click\|double_click\|scroll\|drag}.{trusted\|dom_event}`                                        |

Typed browser clicks and pointer operations have fixed background posture. The
`trusted` and `dom_event` routes are materially different and should remain
distinct resolved action IDs.

## Session, recording, cursor, and maintenance variants

| Public tool                | Resolved action keys                                  |
| -------------------------- | ----------------------------------------------------- |
| `start_session`            | `session.start.{implicit\|named}`                     |
| `get_session`              | `session.inspect.one`                                 |
| `list_sessions`            | `session.inspect.list`                                |
| `end_session`              | `session.end`                                         |
| `escalate_session`         | `session.legacy.escalate.desktop`                     |
| `get_session_state`        | `session.legacy.inspect_capture`                      |
| `start_recording`          | `recording.start.trajectory`, `recording.start.video` |
| `stop_recording`           | `recording.stop`                                      |
| `get_recording_state`      | `recording.inspect`                                   |
| `replay_trajectory`        | `recording.replay`                                    |
| `set_agent_cursor_enabled` | `agent_cursor.show`, `agent_cursor.hide`              |
| `set_agent_cursor_motion`  | `agent_cursor.configure_motion`                       |
| `set_agent_cursor_theme`   | `agent_cursor.set_theme`                              |
| `get_agent_cursor_state`   | `agent_cursor.inspect`                                |
| `check_permissions`        | `permissions.inspect`, `permissions.request`          |
| `health_report`            | `system.health.inspect`                               |
| `get_config`               | `config.inspect`                                      |
| `set_config`               | `config.update`                                       |
| `check_for_update`         | `update.inspect`                                      |
| `install_ffmpeg`           | `dependency.ffmpeg.plan`, `dependency.ffmpeg.install` |

## Complete public tool-name union

The source-built macOS registry contains 49 tools. Windows adds
`debug_window_info`. Linux adds four low-level pointer tools. The union is 54
public tool names.

|   # | Tool                       | Category                    | Platform                              | Primary icon intent                                                   |
| --: | -------------------------- | --------------------------- | ------------------------------------- | --------------------------------------------------------------------- |
|   1 | `list_apps`                | Inspection                  | All                                   | Apps/list                                                             |
|   2 | `list_windows`             | Inspection                  | All                                   | Windows/list                                                          |
|   3 | `get_window_state`         | Inspection                  | All                                   | Window inspect; optional capture treatment                            |
|   4 | `launch_app`               | App lifecycle               | All                                   | Launch                                                                |
|   5 | `kill_app`                 | App lifecycle               | All                                   | Force stop                                                            |
|   6 | `bring_to_front`           | Window lifecycle            | All                                   | Persistent foreground                                                 |
|   7 | `click`                    | Pointer input               | All                                   | Click plus target/delivery/button/count                               |
|   8 | `double_click`             | Pointer input               | All                                   | Double click plus target/delivery                                     |
|   9 | `right_click`              | Pointer input               | All                                   | Context click plus target/delivery                                    |
|  10 | `drag`                     | Pointer input               | All                                   | Drag plus delivery                                                    |
|  11 | `type_text`                | Keyboard input              | All                                   | Type plus target/delivery                                             |
|  12 | `press_key`                | Keyboard input              | All                                   | Single key plus target/delivery                                       |
|  13 | `hotkey`                   | Keyboard input              | All                                   | Key chord plus delivery                                               |
|  14 | `set_value`                | Accessibility input         | All                                   | Set semantic value                                                    |
|  15 | `scroll`                   | Pointer/accessibility input | All                                   | Scroll plus target/delivery/direction                                 |
|  16 | `get_screen_size`          | Inspection                  | All                                   | Screen dimensions                                                     |
|  17 | `get_desktop_state`        | Inspection                  | All                                   | Desktop capture                                                       |
|  18 | `get_cursor_position`      | Inspection                  | All                                   | Cursor location                                                       |
|  19 | `move_cursor`              | Pointer/overlay input       | All                                   | Agent-cursor or real-pointer move selected by the per-call target     |
|  20 | `set_agent_cursor_enabled` | Agent cursor                | All                                   | Show/hide cursor                                                      |
|  21 | `set_agent_cursor_motion`  | Agent cursor                | All                                   | Motion configuration                                                  |
|  22 | `set_agent_cursor_theme`   | Agent cursor                | All                                   | Select an installed visual theme                                      |
|  23 | `get_agent_cursor_state`   | Agent cursor                | All                                   | Cursor inspect                                                        |
|  24 | `check_permissions`        | Permissions                 | All; prompting is macOS-specific      | Permission inspect/request                                            |
|  25 | `health_report`            | Maintenance                 | All                                   | Health check                                                          |
|  26 | `get_config`               | Configuration               | All                                   | Config inspect                                                        |
|  27 | `set_config`               | Configuration               | All                                   | Config update                                                         |
|  28 | `get_accessibility_tree`   | Inspection                  | All                                   | Desktop accessibility inspect                                         |
|  29 | `zoom`                     | Inspection                  | All                                   | Zoom/crop                                                             |
|  30 | `page`                     | Page/DOM                    | All, with operation-specific limits   | Use the seven page operation icons                                    |
|  31 | `get_browser_state`        | Typed browser               | All                                   | Bind or snapshot                                                      |
|  32 | `browser_prepare`          | Typed browser               | All                                   | Browser preparation/profile                                           |
|  33 | `browser_navigate`         | Typed browser               | All                                   | Navigate                                                              |
|  34 | `browser_click`            | Typed browser               | All                                   | Browser click plus route                                              |
|  35 | `browser_type`             | Typed browser               | All                                   | Browser type plus mode                                                |
|  36 | `browser_dialog`           | Typed browser               | All                                   | Inspect/accept/dismiss dialog                                         |
|  37 | `browser_set_input_files`  | Typed browser               | All                                   | Upload/attach files                                                   |
|  38 | `browser_download`         | Typed browser               | All                                   | Download                                                              |
|  39 | `browser_pointer`          | Typed browser               | All                                   | Use the ten browser-pointer operation icons                           |
|  40 | `start_recording`          | Recording                   | All                                   | Start trajectory/video recording                                      |
|  41 | `stop_recording`           | Recording                   | All                                   | Stop recording                                                        |
|  42 | `get_recording_state`      | Recording                   | All                                   | Recording inspect                                                     |
|  43 | `replay_trajectory`        | Recording                   | All                                   | Replay                                                                |
|  44 | `install_ffmpeg`           | Maintenance                 | Windows/Linux; no-op when unnecessary | Plan/install dependency                                               |
|  45 | `start_session`            | Session lifecycle           | All                                   | Optional implicit or named lifecycle start                            |
|  46 | `get_session`              | Session lifecycle           | All                                   | Inspect one visible lifecycle session                                 |
|  47 | `list_sessions`            | Session lifecycle           | All                                   | List transport-visible lifecycle sessions                             |
|  48 | `end_session`              | Session lifecycle           | All                                   | End session                                                           |
|  49 | `escalate_session`         | Legacy session compatibility | All                                  | Deprecated capture-scope escalation                                   |
|  50 | `get_session_state`        | Legacy session compatibility | All                                  | Deprecated capture-scope inspect                                      |
|  51 | `check_for_update`         | Maintenance                 | All                                   | Update check                                                          |
|  52 | `debug_window_info`        | Diagnostic                  | Windows only                          | Window diagnostic                                                     |
|  53 | `mouse_button_down`        | Low-level pointer           | Linux only                            | Hold button                                                           |
|  54 | `mouse_drag`               | Low-level pointer           | Linux only                            | Move held pointer                                                     |
|  55 | `mouse_button_up`          | Low-level pointer           | Linux only                            | Release button                                                        |
|  56 | `parallel_mouse_drag`      | Multi-pointer               | Linux only                            | Concurrent drags                                                      |

## Linux low-level pointer action keys

These tools are intentionally separate because a held button survives across
calls and multiple session cursors can drag concurrently:

```text
linux.mouse_button_down.{left|right|middle}.fixed_background
linux.mouse_drag.fixed_background
linux.mouse_button_up.fixed_background
linux.parallel_mouse_drag.fixed_background
```

## Aliases and removed surfaces

- `type_text_chars` is a deprecated invoke-time alias for `type_text`. It is
  deliberately hidden from `tools/list` and should reuse the `type_text` icon.
- The old `screenshot` tool is removed. Window capture is represented by
  `get_window_state`; full-display capture is `get_desktop_state`.
- Deprecated `get_window_state.capture_mode` values are accepted but ignored.
  They do not represent distinct actions and should not receive icons.
- Internal platform delivery paths such as `cgevent`, `x11_atspi`, `msaa`, and
  `key_events_fg` are result metadata, not public requested actions. They may
  be shown in diagnostics, but should not expand the primary icon catalog.

## Recommended first asset set

Generate these as reusable base glyphs/animations first:

```text
inspect
capture
launch
kill
foreground
click
double_click
right_click
drag
type_text
press_key
hotkey
set_value
scroll
move_cursor
zoom
navigate
dialog
upload
download
record
replay
session
permissions
settings
health
update
```

Then generate small composable treatments for:

```text
ax
px
desktop
page
browser_trusted
browser_dom
background
foreground
left
right
middle
double
triple
up
down
horizontal
vertical
```

This produces distinct foreground/background click states while keeping the
system small enough to remain coherent as new tools are added.
