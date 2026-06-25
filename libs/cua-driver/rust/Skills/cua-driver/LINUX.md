---
name: cua-driver-rs-linux
description: Drive a native Linux app via the cua-driver CLI — snapshot the AT-SPI tree, click by element_index or pixel, type, screenshot, record, verify via re-snapshot. Background by default (no foreground steal, no real-pointer move) on X11. Wayland input/capture is opt-in and still preview.
---

# cua-driver-rs — Linux

The Linux backend drives X11 apps **in the background**: clicks and
keystrokes are injected to the target window without raising it,
activating it, or moving the real pointer — the same no-foreground
contract the macOS and Windows backends hold. The full tool surface is
supported: `click`, `type_text`, scroll, `press_key`, `screenshot`,
`launch_app`, `list_apps`, `list_windows`, `get_window_state`, and
session recording.

AT-SPI is talked to natively over D-Bus (the `atspi`/zbus crate) — no
`pyatspi` or GObject-introspection typelibs are required at runtime.

## How input is delivered (the no-foreground contract)

- **Pixel click** — `XSendEvent(ButtonPress/Release)` to the resolved
  target window. No raise, no activate, no real-pointer warp. (It does
  **not** use `XTestFakeButtonEvent`, which would route through the
  focused window.)
- **Element click** (`element_index`) — AT-SPI `do_action` on the
  accessible. Toolkit-native, focus-free.
- **type_text** — AT-SPI EditableText first (focus-free; lands in an
  *unfocused* window's editable for Qt6 / GTK4). When a **non-editable**
  widget holds focus — a spreadsheet cell, a terminal, a canvas — it
  synth-types into the focused widget via XTest, and terminals take a
  focus-free pty-injection path. So background typing into an editable
  needs no focus; the XTest path is the foreground "type where I
  clicked" case.
- The **agent cursor** is a synthetic overlay showing where the run is
  acting; it never moves the real pointer (same model as macOS/Windows).
  It glides on clicks and `move_cursor`; issue `move_cursor` to make it
  track a field while typing advances focus across cells.

## Wayland (opt-in — still preview)

Native Wayland support is behind an opt-in flag and not yet at the same
maturity as X11:

- `CUA_DRIVER_RS_ENABLE_WAYLAND=1` enables the native-Wayland backend.
  On wlroots compositors (labwc/sway) it uses foreign-toplevel +
  screencopy; on GNOME-Mutter / KDE-KWin, which expose no client
  protocols for cross-window enumeration, cua-driver brings its own
  nested compositor (`CUA_WAYLAND_NEST=1`).
- Without the flag, on a Wayland session the driver falls back to
  XWayland where available.
- Screenshots work via `grim` / wlr-screencopy. **Video recording is
  not yet available on Wayland** — the recorder is `x11grab`, which is
  X11-only.

## Quick triage

If a tool call surprises you on Linux:

1. `cua-driver doctor` — reports the display server (X11 / Wayland),
   AT-SPI bus reachability, and `ffmpeg` availability (for recording).
2. Check `XDG_SESSION_TYPE` — `x11` is fully supported; `wayland`
   needs `CUA_DRIVER_RS_ENABLE_WAYLAND=1` for native input (preview),
   else XWayland.
3. Empty / partial AT-SPI tree — make sure the a11y bus is enabled
   (`gsettings set org.gnome.desktop.interface toolkit-accessibility
   true`). GTK4 / Qt6 also populate lazily, so re-snapshot after an
   interaction.

## Forbidden vectors

Same idea as macOS / Windows — don't shell out to anything that
foregrounds a target:

- `wmctrl -a <window>` / `wmctrl -R <window>` — activates / raises.
- `xdotool windowactivate <wid>` — activates.
- `xdotool key --window <wid> alt+Tab` — focus churn.

Prefer cua-driver tools with an explicit `window_id`. When in doubt,
ask the user.

## What to expect

| Intent | Status |
|---|---|
| Snapshot AT-SPI tree | ✅ GTK3/4, Qt5/6, wxWidgets, Electron (GTK4/Qt6 can be partial — re-snapshot) |
| Pixel click | ✅ background `XSendEvent`, no focus steal, no pointer move — but GTK menus/popups, SDL and Allegro filter synthetic events, so prefer **element-indexed click** for those (trycua/cua#2022) |
| Element-indexed click | ✅ AT-SPI `do_action` |
| Type text | ✅ AT-SPI focus-free, with XTest / pty fallback for the focused widget |
| Hotkey / `press_key` | ✅ |
| Screenshot full-display | ✅ X11; ✅ Wayland via `grim` |
| Screenshot per-window | ✅ X11 |
| `launch_app` | ✅ env-scrubbed launch (no workspace steal) |
| Recording (video) | ✅ X11 (`x11grab`); ⚠️ Wayland not yet supported (preview) |

See `SKILL.md` for the cross-platform loop (snapshot-before-AND-after,
pixel-click contract, failure modes) and `RECORDING.md` for session
recording.
