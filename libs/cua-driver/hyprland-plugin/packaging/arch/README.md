# Arch local-development package

This recipe builds the optional plugin directly from a Cua monorepo checkout.
It is intentionally unsuitable for an AUR or binary repository: there is no
remote source archive, checksum substitution, or promise of ABI compatibility
with another machine.

Install the exact Hyprland package and development files used by the running
Omarchy/Arch session. Point `CUA_SOURCE_ROOT` at the monorepo root and pass the
full package version reported by pacman; the package release suffix is part of
the pin and must not be guessed:

```bash
export CUA_SOURCE_ROOT=/path/to/cua
export CUA_HYPRLAND_PACKAGE_VERSION="$(pacman -Q hyprland | cut -d' ' -f2-)"
export CUA_HYPRLAND_HEADER_VERSION=0.56.2
makepkg -si
```

The recipe requires that exact installed package version and the exact
pkg-config header version, records the package dependency, builds from
`$CUA_SOURCE_ROOT/libs/cua-driver/hyprland-plugin`, runs the configured CTest
suite during `check()`, and installs only:

```text
/usr/lib/cua/hyprland/cua-hyprland-plugin.so
/usr/share/licenses/cua-hyprland-plugin-local/LICENSE
```

It does not edit Hyprland configuration or load the plugin. Load, check, and
unload it explicitly:

```bash
hyprctl plugin load /usr/lib/cua/hyprland/cua-hyprland-plugin.so
hyprctl -j cua:status
hyprctl plugin unload /usr/lib/cua/hyprland/cua-hyprland-plugin.so
```

After every Hyprland upgrade, remove or rebuild this local package before
loading it again. An exact dependency conflict is intentional protection
against carrying a stale compositor ABI module forward.

This is deliberately a monorepo-local development recipe, not an AUR or clean
chroot source package. Fleet must make the recorded candidate source available
at `CUA_SOURCE_ROOT` and record its Git SHA. Build with the same compiler family
and version used for Hyprland; matching the package, headers, full Hyprland ABI
fingerprint, and C++26 language mode does not make different C++ toolchains
interchangeable.

Fleet images should consume this same monorepo recipe or a derived reviewed
recipe, pin the image's exact Hyprland package, and keep the plugin unloaded by
default. The initial acceptance image must match Omarchy `4.0.2-1`, Hyprland
`0.56.2`, xdg-desktop-portal-hyprland `1.4.1`, Cua Driver `0.23.2`, and a
UWSM/SDDM session. This packages a discovery-only plugin; isolated background
input is not available in stable `0.23.2` or nightly. See
`../../tests/README.md` for the required Fleet and subsequent bare-metal
promotion evidence.
