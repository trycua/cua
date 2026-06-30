# cua WinRects — GNOME Shell helper extension (Wayland)

A ~40-line GNOME Shell extension that lets cua-driver get **pixel coordinates**
and draw the **agent cursor** on GNOME Mutter Wayland — two things a normal
Wayland client cannot do (no global coordinates; no `zwlr_layer_shell_v1`).

It exposes `org.cua.WinRects` on the session bus:

- `GetRects() -> json` — every window's `meta_window.get_frame_rect()` (screen
  geometry). cua-driver combines the window origin with AT-SPI
  `CoordType::Window` per-widget coords: `screen = origin + window_xy`. This is
  the GNOME analogue of the X11 `_GTK_FRAME_EXTENTS` reconstruction (AT-SPI's
  `CoordType::Screen` is `(0,0)` for every widget on Mutter).
- `MoveCursor(x,y)` / `ClickPulse(x,y)` / `HideCursor()` — render the agent
  cursor as a Clutter actor on the compositor stage.

It runs in the shell's privileged context, so **no xdg-desktop-portal grant** is
needed (unlike libei/RemoteDesktop).

## Install

```
./install.sh          # copies to ~/.local/share/gnome-shell/extensions + enables
# then log out/in once (GNOME loads extensions only at session startup)
gnome-extensions info winrects@cua   # -> State: ACTIVE
```

cua-driver auto-detects it at runtime (`wayland::shell_helper`); everything is
best-effort, so the driver still runs (without screen coords / Wayland cursor)
when the extension is absent. wlroots compositors (sway/labwc/KWin) don't need
it — cua-driver uses `zwlr_layer_shell` + foreign-toplevel there.

KDE Plasma Wayland (KWin) would need an equivalent KWin script; not yet provided.
