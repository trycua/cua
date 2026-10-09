# cua WinRects: GNOME Shell helper extension (Wayland)

A small GNOME Shell extension that lets cua-driver get **pixel coordinates**,
activate an exact target window, capture the compositor stage, and draw the
**agent cursor** on GNOME Mutter Wayland. A normal Wayland client cannot do
these things globally.

It exposes `org.cua.WinRects` on the session bus:

- `GetVersion() -> uint`: a browser-sensitive API version. cua-driver only
  accepts it after resolving the immutable D-Bus owner and proving the owner is
  the current user's system-installed `gnome-shell` process.
- `GetRects() -> json`: every window's frame geometry and surface-buffer
  origin. cua-driver combines the buffer origin with AT-SPI
  `CoordType::Window` per-widget coords: `screen = origin + window_xy`. This is
  the GNOME analogue of the X11 `_GTK_FRAME_EXTENTS` reconstruction (AT-SPI's
  `CoordType::Screen` is `(0,0)` for every widget on Mutter). Keeping the frame
  and buffer origins separate accounts for GTK client-side shadows.
- `Activate(id) -> bool`: activate one Shell stable-sequence window and report
  whether the request was accepted. cua-driver verifies focus through a second
  `GetRects` snapshot before sending focus-bound portal/libei input, preventing
  input from leaking into whichever application happened to be focused.
- `Capture() -> png_base64`: capture the compositor stage through Shell's
  screenshot API. cua-driver crops it with the same authoritative geometry.
- `MoveCursor(x,y)` / `ClickPulse(x,y)` / `HideCursor()`: position and hide
  the agent cursor as a Clutter actor on the compositor stage.
- `SetCursorState(action,delivery,target,active)`: render the same 12 semantic
  action states as the cross-platform `cua.default` cursor theme. Delivery and
  target context appears as host-owned chips in the session badge rather than
  pointer-relative theme artwork. This contract requires helper v8.
- `SetCursorColor(fill_color)`: apply the stable per-session fill selected by
  cua-driver. The helper validates the `#RRGGBB` value, updates the matching
  glow, and keeps a white pointer outline.
- `CaptureWindow(id,pid) -> png_base64` (API 9): snapshot one window's own
  compositor actor (`paint_to_content`), not the stage, so a covering window
  never shows and the target is not activated. `pid` 0 skips the owner check.
  A 1x1 transparent clone keeps Mutter painting a fully covered window and
  expires 4 s after the last capture. Refused while the screen is locked, for
  minimized windows, and above 32 MiB.
- `CaptureWindowPreview(id,pid,max_dimension) -> (png_base64, width, height)`
  (API 10): the same capture, downscaled on the GPU so its long edge is at
  most `max_dimension` (1 to 4096). `width`/`height` are the window's
  full-size pixels, which cua-driver reports as the original size. cua-driver
  removes GNOME's per-capture `Creation Time` PNG chunk, so identical frames
  are byte-identical.

It runs in the shell's privileged context, so **no xdg-desktop-portal grant** is
needed (unlike libei/RemoteDesktop).

## Install

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

With helper API 9 or newer, cua-driver captures GNOME windows only through
`CaptureWindow`; if the helper refuses a window, the screenshot fails rather
than falling back to a stage crop that could show another app. With an older
helper it still crops the stage to the window frame, which shows whatever
covers the window. Re-run the installer to get per-window capture.

The semantic cursor requires helper v8. When an older helper is still loaded,
cua-driver does not draw its legacy cursor. Re-run the helper installer, then
reload the GNOME session so the new compositor-owned artwork becomes active.

Browser setup and consent are held to a stricter boundary: helper API v4 or
newer must be served by the verified GNOME Shell owner. The driver addresses
that owner's unique D-Bus name, so another same-session process cannot replace
the public name between verification and an activation request. One exact
target is activated only for the bounded operation, then the previously
focused Shell window is restored and verified.

## Versions and other apps that bundle the helper

The extension is identified by its UUID, `winrects@cua`, and serves the
session-bus name `org.cua.WinRects`. Other apps ship the same helper: T3 Code
bundles it with its Linux build of Cua Driver. Keeping one UUID is deliberate.
Two copies under different UUIDs would both be enabled and race for the same
bus name and object path, and which one answered would be undefined. With one
UUID there is exactly one installed copy per user, and the rules below let
any copy serve every client:

- `GetVersion()` returns an integer API version, and `metadata.json`
  `"version"` carries the same number.
- API versions only add methods. A method never changes or disappears in a
  later version, so a newer helper serves every older client. Clients gate
  each feature on `GetVersion()`.
- A version number names one method set everywhere. Versions 9 and 10 were
  first defined in T3 Code's Linux patch to Cua Driver 0.34.0
  ([pingdotgg/t3code#16975](https://github.com/pingdotgg/t3code/pull/16975))
  and are adopted here unchanged, so T3 Code's v10 and this v10 are
  interchangeable. Coordinate any new number with apps that bundle the helper.
- Installers never downgrade. `install.sh` and the cua-driver installers keep
  an installed helper whose version is newer than the bundled one, and say so.
  `install.sh --force` replaces it anyway.

| Version | Adds                                                               |
| ------- | ------------------------------------------------------------------ |
| 8       | The semantic `SetCursorState` contract; shipped by Cua Driver 0.34 |
| 9       | `CaptureWindow(id,pid)`                                            |
| 10      | `CaptureWindowPreview(id,pid,max_dimension)`                       |

Cua Driver 0.34 and earlier bundle v8 or older, and their installers overwrite
an installed helper unconditionally. Running one of those installers after an app
installed v10 downgrades the helper until the newer app reinstalls it.

## Other compositors

wlroots compositors such as Sway and labwc do not need it: cua-driver uses
foreign-toplevel activation, virtual-pointer input, and layer-shell there.

KDE Plasma Wayland needs an equivalent target-addressable KWin activation
adapter; it is not yet provided. Portal reachability alone is insufficient
because RemoteDesktop/libei input is global to the compositor focus.
