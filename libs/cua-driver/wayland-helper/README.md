# cua WinRects — GNOME Shell helper extension (Wayland)

A small GNOME Shell extension that lets cua-driver get **pixel coordinates**,
activate an exact target window, capture the compositor stage, and draw the
**agent cursor** on GNOME Mutter Wayland. A normal Wayland client cannot do
these things globally.

It exposes `org.cua.WinRects` on the session bus:

- `GetVersion() -> uint` — a browser-sensitive API version. cua-driver only
  accepts it after resolving the immutable D-Bus owner and proving the owner is
  the current user's system-installed `gnome-shell` process.
- `GetRects() -> json` — every window's frame geometry and surface-buffer
  origin. cua-driver combines the buffer origin with AT-SPI
  `CoordType::Window` per-widget coords: `screen = origin + window_xy`. This is
  the GNOME analogue of the X11 `_GTK_FRAME_EXTENTS` reconstruction (AT-SPI's
  `CoordType::Screen` is `(0,0)` for every widget on Mutter). Keeping the frame
  and buffer origins separate accounts for GTK client-side shadows.
- `Activate(id) -> bool` — activate one Shell stable-sequence window and report
  whether the request was accepted. cua-driver verifies focus through a second
  `GetRects` snapshot before sending focus-bound portal/libei input, preventing
  input from leaking into whichever application happened to be focused.
- `Capture() -> png_base64` — capture the compositor stage through Shell's
  screenshot API. cua-driver crops it with the same authoritative geometry.
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

cua-driver auto-detects it at runtime (`wayland::shell_helper`). AX operations
still work when it is absent, but pixel geometry, the Shell cursor, and safe
foreground portal input are unavailable. cua-driver refuses focus-bound input
instead of injecting into an unverified target.

Browser setup and consent are held to a stricter boundary: helper API v4 or
newer must be served by the verified GNOME Shell owner. The driver addresses
that owner's unique D-Bus name, so another same-session process cannot replace
the public name between verification and an activation request. One exact
target is activated only for the bounded operation, then the previously
focused Shell window is restored and verified.

wlroots compositors such as Sway and labwc do not need it: cua-driver uses
foreign-toplevel activation, virtual-pointer input, and layer-shell there.

KDE Plasma Wayland needs an equivalent target-addressable KWin activation
adapter; it is not yet provided. Portal reachability alone is insufficient
because RemoteDesktop/libei input is global to the compositor focus.
