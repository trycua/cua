---
name: cua-driver-rs-linux
description: Drive a native Linux app (X11 / Wayland) via the cua-driver CLI — snapshot the AT-SPI tree, click by element_index or pixel, verify via re-snapshot. Linux backend is BETA in cua-driver-rs and the no-foreground contract has open issues (see this doc).
---

# cua-driver-rs — Linux

**Status: BETA.** The Linux backend in cua-driver-rs covers the core
tool surface (click, type_text, scroll, hotkey, screenshot,
launch_app, list_apps, list_windows, get_window_state) but several
behaviors that the macOS / Windows skills consider table-stakes are
**not yet implemented or only partially supported**:

- **No-foreground contract**: limited. X11's input model has no clean
  per-pid event routing equivalent of macOS `CGEventPostToPSN` or
  Windows `PostMessage(WM_LBUTTONDOWN)`. `XTestFakeKeyEvent` /
  `XTestFakeButtonEvent` synthesize input but route to the focused
  window — similar focus-stealing behavior to Windows `SendInput`,
  which the Windows backend avoids by using `PostMessage` instead.
  Linux has no equivalent per-window-message channel that bypasses
  focus, which is why XTest's focus-stealing is the binding
  limitation here. AT-SPI `accDoDefaultAction` works for accessible
  elements but requires the user's accessibility bus to be running,
  which is not the default on every distro.
- **Wayland support**: depends on compositor. Under GNOME-Mutter and
  KDE-KWin with `org.freedesktop.portal.RemoteDesktop` enabled, some
  click and key paths work. Under most other compositors, input
  synthesis is denied by the security model and the tool surface
  degrades to "passive" (snapshot, screenshot) only. Hyprland is the
  exception: it is fully supported for background element-index
  workflows — see the Hyprland section below.
- **UIA / AX-tree equivalent**: AT-SPI when available, otherwise
  empty. Many GTK4 / Qt6 apps populate AT-SPI lazily; agents should
  expect partial trees and re-snapshot.
- **launch_app**: backed by `xdg-open` / `gtk-launch` / `dbus-send`
  with display-environment scrubbing to avoid stealing the user's
  workspace. Not yet equivalent to macOS `FocusRestoreGuard`.
- **Recording**: supported. Per-turn screenshots + `app_state.json`
  (AT-SPI tree) + video. Video uses wlr-screencopy on Wayland and
  `x11grab` on X11; requires ffmpeg on PATH.

See `SKILL.md` (macOS) and `WINDOWS.md` (Windows) for the full
patterns. This file will grow as the Linux backend reaches GA. For
now, **prefer macOS / Windows hosts** for agent-driven GUI tasks; use
the Linux daemon for read-only inspection (screenshot,
list_windows, get_window_state) when running on a Linux host.

## Quick triage

If you're agent-driving on Linux and a tool call surprises you:

1. Run `cua-driver doctor` — reports display server (X11 / Wayland),
   AT-SPI bus reachability, XTest availability.
2. Check `XDG_SESSION_TYPE` — `wayland` means most input synthesis
   is gated by portals; `x11` means XTest works but routes via focus.
3. If Wayland: confirm `org.freedesktop.portal.RemoteDesktop` is
   present (`gdbus introspect --session --dest
   org.freedesktop.portal.Desktop --object-path
   /org/freedesktop/portal/desktop`). Without it, input synthesis
   is denied.

## Hyprland

Hyprland is the exception to the "passive-only under Wayland" rule —
background element-index workflows are fully supported. What's
specific to it:

- **Window ids**: `window_id` values come from hyprctl window
  addresses and exceed `u32::MAX`. That's expected — pass them
  through verbatim, don't truncate.
- **Per-window screenshots**: captured via the
  `hyprland-toplevel-export-v1` protocol — true surface capture, so
  the screenshot shows the correct content even for occluded /
  background windows and windows on other workspaces. This is what
  makes background computer use verifiable on Hyprland. grim
  region-crop is the fallback when the protocol is unavailable.
- **Input**: native-Wayland windows accept `element_index` actions
  (AT-SPI) but not pixel input.
- **launch_app**: if a newly launched window steals focus, the driver
  restores the previously active window. Best-effort, watches for
  ~2 s after launch.
- **Recording**: video captures the focused monitor via
  wlr-screencopy frames piped to ffmpeg. `cursor.jsonl` sampling
  works via the Hyprland IPC `cursorpos` query (global logical
  coords; empty on other Linux sessions).
- **Permission caveat**: if `ecosystem:enforce_permissions` is
  enabled in the Hyprland config and screencopy is denied, captures
  silently return black "permission denied" frames — no error is
  raised. Add an allow rule for the cua-driver binary to the
  Hyprland permission config.

## Forbidden vectors

Same idea as macOS / Windows — don't shell out to anything that
foregrounds a target:

- `wmctrl -a <window>` — activates the named window.
- `xdotool windowactivate <wid>` — activates.
- `wmctrl -R <window>` — raises and activates.
- `xdotool key --window <wid> alt+Tab` — same problem as Windows
  Alt+Tab.

Prefer cua-driver tools with explicit `window_id`. When in doubt,
ask the user.

## What to expect today

| Intent | Status |
|---|---|
| Snapshot UIA tree | ✅ AT-SPI when available, often partial for GTK4/Qt6 |
| Pixel click | ⚠️ X11 only, focus-stealing semantics |
| Element-indexed click | ⚠️ AT-SPI `accDoDefaultAction` when supported |
| Type text | ⚠️ XTest, focus-sensitive |
| Hotkey | ⚠️ XTest, focus-sensitive |
| Screenshot full-display | ✅ X11 (xshm); ✅ Wayland via grim (no portal) |
| Screenshot per-window | ✅ X11; ✅ Hyprland via toplevel-export (correct even when occluded); other Wayland TBD |
| launch_app | ✅ direct exec / xdg-open; focus-restore guard on Hyprland (see Hyprland section) |
| Recording | ✅ wlr-screencopy on Wayland / `x11grab` on X11; ffmpeg required |

Until Linux reaches GA, treat this doc as a planning placeholder
rather than a contract.
