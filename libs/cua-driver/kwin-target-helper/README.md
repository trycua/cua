# Cua KWin target helper

This is an **optional KDE Plasma/KWin Wayland integration**. It is not part of
the portable `cua-driver` binary and must be built against the user's installed
KWin/Qt/KF6 development files. Do not use a helper built for a different KWin
or Qt ABI.

The helper is intentionally **read-only**. It provides trusted KWin window
identity and state metadata: stable opaque tokens, PID, geometry,
active/minimized state, and stacking order. It does not expose window activation
or input mutation methods. KWin portal/libei input is focus-bound, so focus can
change between any focus check and compositor-side input processing. Cua
therefore refuses raw target-addressed KWin keyboard/pointer actions until a
target-bound KWin input path exists.

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
The exported D-Bus surface is deliberately limited to `GetVersion` and
`GetWindows`.

## Input safety boundary

Portal/libei events are delivered according to compositor focus when KWin
processes them. A target focus check immediately before dispatch cannot bind the
later event to that target, so adding more checks does not remove the race.
Until a future KWin integration can bind the mutation itself to an exact target,
Cua fails closed rather than sending raw focus-based input to a requested KWin
window.

## ABI and rollback

The module is compositor/Qt ABI-specific. Rebuild after Plasma/KWin or Qt
upgrades. To roll back a user-local test installation, unload the effect and
remove only the exact module installed by that test; do not remove KWin or
change the desktop session. The portable Cua Driver continues to build without
this optional helper and without KWin development packages.
