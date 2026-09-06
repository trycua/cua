# Install the pinned Hyprland plugin

This source distribution builds the Cua production input candidate without a
monorepo checkout. Use the exact Driver component release named by
`release_tag` in `SOURCE-PROVENANCE.json`. The Arch package version follows that
Driver release; the plugin's CMake version is recorded as `plugin_version`.

The source generator does not perform native certification. Its
`native_certified: false` field describes that limitation. Consult the release's
separate native evidence for the exact source revision and environment before
using the candidate. A checksum proves agreement with the reviewed recipe, not
native compatibility by itself.

## Build and install

The supported build contract is Linux x86_64 with Hyprland `0.56.2-1`, headers
`0.56.2`, GCC `16.1.1 20260728`, and shared `libstdc++.so.6.0.36`. The compiler,
compositor, and module must resolve the same runtime bytes. A newer package or
compiler does not satisfy this contract. Arrange the pinned dependencies in the
target environment before building; the recipe does not install a toolchain or
alter runtime search paths.

Download the source tarball, `PKGBUILD`, `SOURCE-PROVENANCE.json`, `README.md`, and
`SHA256SUMS` from the same exact component release into a dedicated directory.
Review the recipe and verify the downloaded assets:

```sh
sha256sum -c SHA256SUMS
```

Run `makepkg` as an ordinary user in that directory. It uses `/usr/bin/g++` by
default. If your matching compiler is staged elsewhere, set `CUA_RELEASE_CXX`
to its absolute executable path before invoking `makepkg`. The verifier checks
its identity, an emitted compiler probe, the compositor, and the shared runtime.
The recipe builds and tests the plugin with production input enabled,
experimental signed input disabled, and tracing disabled.

Before installing or replacing a module, save your work and exit the Hyprland
desktop session. From a text console, install the single package produced by
the recipe:

```sh
package_file="$(makepkg --packagelist)"
sudo pacman -U "$package_file"
```

Installation writes the module to
`/usr/lib/cua/hyprland/cua-hyprland-plugin.so`, its license under
`/usr/share/licenses/cua-hyprland-plugin/`, and source/build provenance under
`/usr/share/cua-hyprland-plugin/`. It has no install hooks, automatic loading, or
configuration changes. Keep the package file and its provenance for rollback.

## Activate in a fresh desktop session

Start a fresh Hyprland session with the pinned compositor. Load the module
explicitly and inspect its status:

```sh
hyprctl plugin load /usr/lib/cua/hyprland/cua-hyprland-plugin.so
hyprctl -j cua:status
```

The local input transport is disabled by default. Enabling it grants trusted
local desktop input capability; Driver continues to enforce its own permission
policy on individual calls. In Omarchy's Lua configuration, add:

```lua
hl.config({plugin = {cua = {enabled = true}}})
```

For a legacy Hyprland configuration, add:

```text
plugin:cua:enabled = true
```

After editing the configuration, run `hyprctl reload`, then inspect
`hyprctl -j cua:status` again. A runtime keyword or Lua evaluation without a
configuration reload does not reconcile the input sockets. Check the status's
protocol, capabilities, socket paths, and compositor identity. A loaded module
alone does not prove input availability. Start the matching Driver with
`CUA_DRIVER_RS_ENABLE_WAYLAND=1`; its production input protocol is v3 by default.

The input path checks exact native application packages: `libreoffice-fresh
26.2.5-3` for Calc and `inkscape 1.4.4-6`. It also requires the canonical
`evdev/pc105/us` keyboard map without variants, remaps, or additional groups.
There are two independent input lanes. A third concurrent owner receives a
lane-busy refusal. Other application versions, Chromium/Electron, and XWayland
are outside this input contract. Use normal Driver snapshots and background
actions, then verify the application result; a delivery acknowledgement is not
proof that the intended edit occurred. Do not automatically replay partial or
unknown actions. The release's native evidence identifies the qualified
operation cells within these limits.

## Upgrade, rollback, or remove

Keep a copy of the installed package and its exact compositor/compiler/runtime
provenance before upgrading. Build and test the replacement against its declared
pins. Exit the desktop session before replacing the package, install the
replacement from a text console, then start a fresh desktop session and repeat
activation and status checks. Do not hot-unload/reload the module to replace it:
production input retains resources until the compositor process exits.

If the replacement fails validation, exit the desktop session, reinstall the
saved package with `pacman -U` and its exact saved package filename, and restore
its matching compositor/runtime environment before starting a fresh session.
Do not force installation past the exact Hyprland dependency. If matching pins
are unavailable, leave the plugin inactive and use Driver without this plugin.

To disable input, remove the enabling setting or set it to false and run
`hyprctl reload`. To remove the package, remove any operator-added plugin load
or enable settings, exit the desktop session, and run
`sudo pacman -R cua-hyprland-plugin` from a text console. Start a fresh session
afterward. Unloading, upgrading, or removing a file in a running compositor does
not substitute for restarting that compositor.
