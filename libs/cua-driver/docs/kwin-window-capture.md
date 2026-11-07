# KWin window capture

The native KWin capture adapter uses Plasma window-management UUIDs and
`org.kde.KWin.getWindowInfo` for exact window ownership and geometry. It calls
`org.kde.KWin.ScreenShot2.CaptureWindow` with that UUID and checks the returned
`windowId`. It does not crop the active desktop or activate the target.

`bring_to_front` and `invoke_menu` are the calls that change focus. They activate
the exact UUID through Plasma window management and confirm the window's state
from KWin, and they send no global input afterwards.

The optional Cua KWin effect is not required for this capture path. The adapter
does not enable KWin raw keyboard or pointer input.

## Input limitations

Native window IDs are driver-local UUID handles, not X11 XIDs. An AT-SPI
action or targeted editable-text operation can still work without raw input.
A control with no usable semantic action gets no pointer fallback, and a native
window ID never reaches X11 injection.

Background pointer fallback returns `background_unavailable`. Foreground raw
input returns `foreground_unavailable`, on any window: KWin's existing
portal/libei path is focus-bound, not window-bound. Activating a UUID and reading
back focus would leave a race between that check and input delivery. A
compositor-side target-bound input API is required before these requests can be
enabled. Installing the read-only Cua KWin helper or restarting AT-SPI does not
supply that capability. Only `bring_to_front` checks the window's current PID, and
it returns `foreground_unavailable` when another process owns the window.

## Permissions

This adapter gives an executable window capture and window management. What that
takes depends on the KWin release:

| KWin | `org.kde.KWin.ScreenShot2` | `org_kde_plasma_window_management` |
| --- | --- | --- |
| 6.7 and older | Needs `X-KDE-DBUS-Restricted-Interfaces` | Needs `X-KDE-Wayland-Interfaces` |
| 6.8 and newer | Any executable of the same user | Any client that isn't sandboxed |

KWin 6.8 removed both checks ([0ea31ae7](https://invent.kde.org/plasma/kwin/-/commit/0ea31ae72eb052cb9c9cc508318fc3f0f329ed53)).
On 6.8.2, plain `python3` with no desktop entry captured the desktop. So there,
this adapter works with no install step, and it adds no new reach: any program of
that user can already make these calls.

## Installation

Enable the existing native Wayland backend with
`CUA_DRIVER_RS_ENABLE_WAYLAND=1`. On KWin 6.7 and older, the application
installation must declare:

```ini
X-KDE-Wayland-Interfaces=org_kde_plasma_window_management
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
```

KWin matches the desktop entry's executable to the running process. A source
binary alone does not acquire these capabilities. The Nix package ships an entry
with its exact installed executable path. The release installer's entry is tracked
in [issue #6784](https://github.com/trycua/cua/issues/6784), so check there whether
your installer writes it. Install through the host's normal packaging and
permission mechanism; do not disable KWin permission checks. Restrictions vary by
KWin version and sandbox. The capture interface must return `windowId` metadata;
older implementations without that identity proof are refused. Native validation
used KWin 6.3.6, 6.6.6, 6.7.4 and 6.8.2.

A session whose `XDG_CURRENT_DESKTOP` names another desktop skips this adapter, so
other compositors pay nothing for it. An unset name still tries it.

Window enumeration accepts Plasma window-management v13 and newer, binding no
higher than v17. Stacking comes from the UUID-order event on v13–v16 (KWin 6.0
advertises v16) and the explicit stacking-order object on v17+. Higher `z_index`
values are nearer the front; UUID sorting only stabilizes ID allocation, not
stacking order.

On-screen metadata requires a non-minimized, nonzero-sized window on the current
virtual desktop and Activity. An absent or empty `activities` field means no
Activity restriction. A restricted window is not reported on-screen when the
current Activity cannot be established. Querying the current Activity does not
activate an absent Activity Manager service.

If Plasma window management is unavailable, discovery retains the existing
helper/AT-SPI fallbacks. Those fallback IDs are not KWin UUIDs and do not become
eligible for native KWin capture. A native capture error does not fall back to
an unrelated desktop crop.

Captures exclude the pointer and window shadow, include decorations, and use
logical resolution to match the reported window coordinate frame. KWin can return
physical pixels even when asked for logical ones, so the image is resized by the
scale KWin reports for it. A window resized around the capture keeps its own size
rather than being stretched to the new frame. Native pixel formats are decoded
with row-stride and premultiplied-alpha handling. Unsupported formats, invalid
layouts, mismatched owners/UUIDs, closed windows and timeouts are errors.

## Desktop capture

A desktop screenshot asks `CaptureWorkspace` first, in logical pixels and with
every output, before the Screenshot portal. Where KWin answers (see
[Permissions](#permissions)), there is no consent dialog, and a **Deny** saved in
the portal no longer applies, because the portal is never asked. Where KWin
refuses, the portal runs as before.

## Focused native regression

In a disposable KWin Wayland session, with `XDG_CURRENT_DESKTOP` set to `KDE` or
unset, install the exact candidate binary with its desktop entry and provide Python with GTK3/PyGObject and a working AT-SPI
bus. Isolate the session bus, runtime directory and accessibility bus together.
Start `at-spi-bus-launcher` inside that session rather than forwarding D-Bus
activation to the host's systemd user manager; verify that `org.a11y.Bus.GetAddress`
points inside the disposable runtime directory before starting the fixtures.
From `rust/` run:

```sh
CUA_TEST_DRIVER_BIN=/path/to/installed/cua-driver \
CUA_TEST_REQUIRE_DRIVER_BIN=1 \
CUA_E2E_UNRESTRICTED_GUI=1 \
cargo test -p cua-driver-e2e --test kwin_native_capture_test --locked -- \
  --ignored --nocapture --test-threads=1
```

This manual test creates two fullscreen windows with identical PID and title
but different solid colors. Both screenshots must retain their own color,
including the covered window. It checks repeated capture, logical dimensions,
owner mismatch, closure, resize, stable IDs and minimized visibility. Run it
at scale 1 and scale 2. It also checks exact refusal codes for raw foreground
input and semantic-click pointer fallback without dispatching to X11, that none
of them raises the covered window, and that `bring_to_front` refuses a wrong owner
and raises the window for the right one.
It verifies stacking changes between the same-PID windows and that PID-only
capture selects the topmost window. Hermetic protocol fixtures cover v12 refusal,
v13–v16 legacy stacking and v17+ explicit stacking; Activity membership and
disabled-service metadata are covered by pure tests. These fixtures do not
establish native capture compatibility with older KWin releases or certify a
live multi-Activity desktop.
It supplements the canonical desktop matrix; it does
not replace that matrix or establish coverage on other compositors.
