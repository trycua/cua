# Cua KWin target helper

This is an **optional KDE Plasma/KWin Wayland integration**. It is not part of
the portable `cua-driver` binary and must be built against the user's installed
KWin/Qt/KF6 development files. Do not use a helper built for a different KWin
or Qt ABI.

The helper provides trusted KWin window identity and state metadata: stable
opaque tokens, PID, geometry, active/minimized state, and stacking order.
Protocol 2 adds an **input gate** that binds Cua Driver's input to one exact
window. The helper never creates, injects, or changes input, and it exposes no
window activation.

## Requirements

- KDE Plasma/KWin 6 with a matching KWin development package
- Qt 6 development files
- ECM/KF6CoreAddons development files
- CMake 3.21 or newer
- a running Wayland KWin session for loading/testing

Install the distribution's matching development packages (their names vary;
`tests/Containerfile*` list them for Ubuntu, Debian and Fedora), then run:

```bash
./install.sh
```

It builds the helper against this machine's KWin, installs it system-wide (it
asks for root), and runs `cua-driver kwin-capture on`. That command loads the
helper into the running KWin and has KWin start it with every session, so
nothing needs repeating after a login. `cua-driver kwin-capture status` shows
whether the helper is installed, loaded, and set to start. `./install.sh
--uninstall` turns it off and removes it.

Run `./install.sh` again after a KWin upgrade: KWin loads only a helper built
for its own version, and the status then reads "installed but not loaded".

The script installs one file in KWin's effect plugin namespace:

```text
<qt6 plugin dir>/kwin/effects/plugins/cua_kwin_target_helper.so
```

To build without installing:

```bash
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --parallel
```

Never copy the module into an arbitrary plugin directory and never overwrite
a file that is not this helper.

The helper exposes the Cua-owned service `org.cua.KWinTarget` only while the
effect is loaded by the current `kwin_wayland` process. Verify the service
owner, protocol version, and target snapshots before trusting identity data.
The exported D-Bus surface:

| Method | Since | Returns |
|---|---|---|
| `GetVersion()` | 1 | the protocol version |
| `GetWindows()` | 1 | window snapshots (protocol 2 adds each window's KWin `uuid`) |
| `GetFocus(uuid)` | 2 | whether keyboard and pointer focus are on that window or its transients |
| `ArmInput(uuid, client)` | 2 | an input gate id, or an error |
| `GetInputGate(id)` | 2 | how many events the gate let through and held back |
| `DisarmInput(id)` | 2 | the same counts, and removes the gate |

## Input safety boundary

Portal/libei events are delivered according to compositor focus when KWin
processes them. A focus check immediately before dispatch cannot bind the later
event to that target, so adding more checks does not remove the race.

The input gate binds the event itself instead. It is a KWin input filter: KWin
runs it on its main thread for each event, after resolving where the event goes
and before any window receives it. While a gate is armed for a window, an event
from one of the driver's own libei devices passes only when the keyboard or
pointer focus it would go to is that window or one of its transients (menus,
popups, dialogs). Any other event is held back and counted, and Cua Driver fails
the action with `input_interrupted` instead of reporting success. A release
follows its press, so no key or button is left held down.

Limits of the gate:

- It gates only devices whose libei client name starts with `cua-driver `
  followed by the arming client's name, so it can never hold back another
  program's input or the user's own keyboard and mouse.
- `ArmInput` refuses when that client has no connected libei device, when
  another gate is armed, or when the window is gone.
- A gate lasts until `DisarmInput` or until the D-Bus connection that armed it
  closes, so a driver that dies mid-action leaves nothing armed.
- KWin moves the pointer and updates keyboard modifier state before filters
  run. A held-back event can still move the cursor or briefly change the
  modifier state another window sees, but never delivers a key, button, motion,
  or scroll to it.
- Text a libei client sends as a string (`ei_text`, KWin 6.8 and newer) goes
  to KWin's input method without passing any input filter, so the gate never
  sees it. While a gate is armed, Cua Driver sends text only as key presses on
  the active layout, and refuses text the layout has no key for.

## Testing the input gate

`tests/run.sh` is an opt-in test that never touches your desktop session. It
builds a container image with KWin, builds the helper against it, and starts a headless KWin on the container's own D-Bus with two windows
that log every key and click they receive.

```bash
tests/run.sh                        # gate suite: the helper with a raw libei client
tests/run.sh path/to/cua-driver     # also the driver suite
DISTRO=debian13 tests/run.sh ...    # KWin 6.3 instead of Ubuntu 26.04's 6.6
DISTRO=fedora45 tests/run.sh ...    # KWin 6.8, which offers ei_text
```

The driver suite needs a `--features portal-input` build whose glibc the image
has; one built on Debian 13 runs on all three. It checks that typing, a pixel click and a key press reach the named
window while another window is active, that stealing focus mid-typing fails the
call with `input_interrupted` while the other window receives nothing, that
background input stays refused, and that input is refused once the helper is
unloaded. It needs podman or docker, and `/dev/dri` for the window capture the
pixel click reads first.

The headless KWin runs with its Wayland and screenshot permission checks off,
because those read KDE's application cache, which the throwaway session never
builds. They gate which programs may list and capture windows, not input.

## ABI and rollback

The module is compositor/Qt ABI-specific. Rebuild after Plasma/KWin or Qt
upgrades. To roll back a user-local test installation, unload the effect and
remove only the exact module installed by that test; do not remove KWin or
change the desktop session. The portable Cua Driver continues to build without
this optional helper and without KWin development packages.
