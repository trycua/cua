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

## Turning it on and off

The adapter is off by default on every KWin release. The person running the
driver turns it on or off with:

```sh
cua-driver kwin-capture on
cua-driver kwin-capture off
cua-driver kwin-capture status
```

The command saves `on`, `off` or `override-portal-deny` in
`~/.cua-driver/kwin-window-capture`. Any other content, or no file, is off. The driver reads it on every call, so no restart
of the driver or of KWin is needed. Off applies from the next call. On
returns once KWin grants a fresh connection, waiting up to about five seconds on
6.7 and older while KWin rereads its desktop-entry cache, so it applies from the
next call too. If KWin hasn't granted it by then, `on` still saves the setting,
prints the status line and exits 0; run `cua-driver kwin-capture status` to
check again. `set_config` cannot change it, so a model
can't turn it on for itself. While it is off, the driver behaves as it does
without this adapter: window capture on KWin returns `surface_identity_unproven`
and `bring_to_front` returns `foreground_unavailable`. Window ids listed while
it was on become unknown ids, so the next call refuses them and the caller lists
windows again.

A screenshot Deny saved in the desktop portal (`xdg-desktop-portal` permission
store, table `screenshot`) keeps the adapter off, because `ScreenShot2` doesn't
consult the portal itself. A permission store that doesn't answer counts as a
Deny; no store, or no saved answer, doesn't. The driver reuses one answer for up
to a second, so a Deny saved meanwhile applies within that second. `cua-driver kwin-capture on
--override-portal-deny` ignores the Deny.

What KWin itself checks depends on the release:

| KWin | `org.kde.KWin.ScreenShot2` | `org_kde_plasma_window_management` |
| --- | --- | --- |
| 6.7 and older | Needs `X-KDE-DBUS-Restricted-Interfaces` | Needs `X-KDE-Wayland-Interfaces` |
| 6.8 and newer | Any executable of the same user | Any client that isn't sandboxed |

KWin 6.8 removed both checks ([0ea31ae7](https://invent.kde.org/plasma/kwin/-/commit/0ea31ae72eb052cb9c9cc508318fc3f0f329ed53)).
For 6.7 and older, `kwin-capture on` writes
`~/.local/share/applications/ai.cua.driver.desktop` (under `XDG_DATA_HOME` when
set) naming the running executable, through `packages/current` for a release
install, and:

```ini
X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,org_kde_kwin_keystate
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
```

then runs `kbuildsycoca6` so KWin sees it. `kwin-capture off` removes it. KWin
matches the entry's `Exec` to the client's executable and looks it up for each
new Wayland connection; the adapter opens a new one on every call. KWin resolves
that link when it checks, so a release update keeps the grant. If the
executable moves some other way, `kwin-capture status` says the entry names
another executable; run `kwin-capture on` again. It reads only `Exec` and the
interface lists, so editing `Name` keeps the grant, and an entry without window
management or ScreenShot2 gets the same advice. Separate the Wayland
interfaces with commas: KWin 6.3.6 doesn't honor a semicolon list.
`org_kde_kwin_keystate` isn't for capture. It tells desktop typing whether Caps
Lock is on, and KWin 6.3 to 6.5 offers it only to an entry that lists it.

Enable the native Wayland backend with `CUA_DRIVER_RS_ENABLE_WAYLAND=1`. The
capture interface must return `windowId` metadata; older implementations without
that identity proof are refused. Native validation used KWin 6.3.6, 6.6.6, 6.7.4
and 6.8.2.

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
an unrelated desktop crop: it returns `surface_identity_unproven`.

Captures exclude the pointer and window shadow, include decorations, and use
logical resolution to match the reported window coordinate frame. KWin can return
physical pixels even when asked for logical ones, so the image is resized by the
scale KWin reports for it. A window resized around the capture keeps its own size
rather than being stretched to the new frame. Native pixel formats are decoded
with row-stride and premultiplied-alpha handling. Unsupported formats, invalid
layouts, mismatched owners/UUIDs, closed windows and timeouts return
`surface_identity_unproven`, the same refusal as any window the driver can't
capture exactly. The driver log at debug level names the cause.

## Desktop capture

While the adapter is on, a desktop screenshot asks `CaptureWorkspace` before
the Screenshot portal, in logical pixels and with every output. Its image is the
desktop frame `get_desktop_state` and desktop input use, so window positions and
clicks line up with it on any layout. Where KWin answers (see
[Turning it on and off](#turning-it-on-and-off)), there is no consent dialog.
`ScreenShot2` never asks the portal, so the switch is what keeps a portal
**Deny**: a saved Deny keeps this off, as it does window capture, unless
`--override-portal-deny` was given. While the adapter is off, or where KWin
refuses, the portal runs as before.

## Focused native regression

In a disposable KWin Wayland session, with `XDG_CURRENT_DESKTOP` set to `KDE` or
unset, install the exact candidate binary, run its `kwin-capture on`, and provide Python with GTK3/PyGObject and a working AT-SPI
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
