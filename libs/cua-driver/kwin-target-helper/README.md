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

On Arch-derived systems, install the distribution's matching packages (package
names vary by distribution), then build from this directory:

```bash
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --parallel
```

The module is installed in KWin's effect plugin namespace:

```text
kwin/effects/plugins/cua_kwin_target_helper.so
```

For a user-local installation, use a prefix supported by the local Qt/KWin
plugin search path and verify discovery before loading:

```bash
cmake --install build --prefix "$HOME/.local"
```

If the local KWin installation does not search that prefix, use the
distribution's native packaging mechanism or an explicitly approved
system-local installation. Never copy the module into an arbitrary plugin
directory and never overwrite an existing file.

Load/reload through KWin's supported Effects interface where available:

```bash
qdbus6 org.kde.KWin /Effects loadEffect cua_kwin_target_helper
```

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

## Testing the input gate

`tests/run.sh` is an opt-in test that never touches your desktop session. It
builds a container image with KWin 6.6 (Ubuntu 26.04), builds the helper against
it, and starts a headless KWin on the container's own D-Bus with two windows
that log every key and click they receive.

```bash
tests/run.sh                        # gate suite: the helper with a raw libei client
tests/run.sh path/to/cua-driver     # also the driver suite
```

The driver suite needs a `--features portal-input` build linked for Ubuntu
26.04. It checks that typing, a pixel click and a key press reach the named
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
