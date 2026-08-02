# cua compositor helpers — GNOME and KDE Wayland

These packages provide compositor-owned window identity and activation where
ordinary Wayland clients cannot. They are desktop-specific: WinRects targets
GNOME Shell/Mutter, and the KWin adapter targets KDE Plasma 6/KWin. Installing
either package does not claim support for other Wayland compositors.

## GNOME Shell/Mutter: WinRects

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
- `MoveCursor(x,y)` / `ClickPulse(x,y)` / `HideCursor()` — position and hide
  the agent cursor as a Clutter actor on the compositor stage.
- `SetCursorState(action,delivery,target,active)` — render the same 12 semantic
  action states as the cross-platform `cua.default` cursor theme. Delivery and
  target context appears as host-owned chips in the session badge rather than
  pointer-relative theme artwork. This contract requires helper v8.
- `SetCursorColor(fill_color)` — apply the stable per-session fill selected by
  cua-driver. The helper validates the `#RRGGBB` value, updates the matching
  glow, and keeps a white pointer outline.

It runs in the shell's privileged context, so **no xdg-desktop-portal grant** is
needed (unlike libei/RemoteDesktop).

### Install on GNOME

```
~/.cua-driver/packages/current/wayland-helper/install.sh
# From a source checkout, use ./install.sh in this directory.
# then log out/in once (GNOME loads extensions only at session startup)
gnome-extensions info winrects@cua   # -> State: ACTIVE
```

cua-driver auto-detects it at runtime (`wayland::shell_helper`). AX operations
still work when it is absent, but pixel geometry, the Shell cursor, and safe
foreground portal input are unavailable. cua-driver refuses focus-bound input
instead of injecting into an unverified target.

The semantic cursor requires helper v8. When an older helper is still loaded,
cua-driver does not draw its legacy cursor. Re-run the helper installer, then
reload the GNOME session so the new compositor-owned artwork becomes active.

Browser setup and consent are held to a stricter boundary: helper API v4 or
newer must be served by the verified GNOME Shell owner. The driver addresses
that owner's unique D-Bus name, so another same-session process cannot replace
the public name between verification and an activation request. One exact
target is activated only for the bounded operation, then the previously
focused Shell window is restored and verified.

wlroots compositors such as Sway and labwc do not need it: cua-driver uses
foreign-toplevel activation, virtual-pointer input, and layer-shell there.

## KDE Plasma 6/KWin: exact target adapter

The adapter source at `kwin/cua-kwin@cua` is a passive KWin 6 script with
protocol version 1. KDE's KPackage ID grammar does not permit `@`, so its
installed KWin package ID is the equivalent `cua-kwin.cua`; metadata retains
`cua-kwin@cua` as the adapter ID. It defines two primitives in KWin's own
scripting context:

- `cuaKWinSnapshot()` returns KWin's current stacking order, exact unmodified
  `KWin::Window.internalId` UUID, PID, frame geometry, caption,
  active/minimized/hidden/visible state, and the active-window UUID.
- `cuaKWinActivate(uuid)` resolves one complete internal UUID and asks KWin to
  unminimize, raise, and activate only that window. Its acknowledgement is not
  treated as proof of focus; cua-driver obtains another fresh snapshot and
  compares the complete UUID before any portal/libei input is sent.

Merely enabling the package performs no enumeration, activation, signal
registration, or other mutation. cua-driver invokes one primitive at a time
through the verified current user's genuine `kwin_wayland` process. The driver
retains each complete internal UUID behind its numeric API identifier, rejects
collisions, stale or unknown identifiers, and ambiguous PID-only targeting.

For a foreground operation, cua-driver records the exact active UUID, activates
and verifies the requested UUID, runs one bounded focus-sensitive operation,
then restores and verifies the prior UUID. Any missing, malformed, outdated, or
untrusted adapter state fails closed before global input.

### Install or update on KDE

```bash
~/.cua-driver/packages/current/wayland-helper/kwin/install.sh
# From a source checkout:
./libs/cua-driver/wayland-helper/kwin/install.sh
./libs/cua-driver/wayland-helper/kwin/diagnose.sh
cua-driver doctor
```

Installation uses Plasma's current-user KPackage directory and updates only
the adapter's `cua-kwin.cuaEnabled` KWin setting. It never needs root and does
not edit unrelated KWin settings. A running KWin session is reconfigured over
its session-bus API, so logout/login is not required. If KWin is not running,
the enabled package is loaded at the next ordinary Plasma session start.

Re-run `kwin/install.sh` to update the package. The diagnostic is read-only and
reports the session type, package/protocol state, KWin reachability, enabled
state, and whether KWin loaded the script. `cua-driver doctor` performs the
stronger owner/process, enumeration, exact-activation, and portal checks.

### Uninstall on KDE

```bash
~/.cua-driver/packages/current/wayland-helper/kwin/uninstall.sh
# From a source checkout:
./libs/cua-driver/wayland-helper/kwin/uninstall.sh
```

The uninstaller disables and unloads only the `cua-kwin@cua` adapter's
`cua-kwin.cua` KWin package, removes its
current-user KPackage, and reconfigures KWin. No logout/login is required.

Portal reachability by itself is never sufficient: RemoteDesktop/libei input
is global to KWin's current focus. If the adapter or its supported protocol is
absent, cua-driver continues to allow safe AT-SPI actions but refuses
focus-bound foreground input rather than guessing a target.
