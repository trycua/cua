# NixOS Linux desktop certification

The NixOS checks exercise the packaged driver in disposable desktop sessions.
They keep session start-up, the MCP service, real input, capture, and visual
artifacts covered independently from the Ubuntu-hosted Rust E2E runners. Rust
owns the shared typed behavior contract; these checks prevent a NixOS or
compositor integration regression from silently removing that coverage.

| Coverage | Flake attributes | Automatic CI |
| --- | --- | --- |
| X11 service/integration and capture | `cua-driver-integration`, `cua-driver-screenshot` | PR smoke |
| X11 cursor/click, background terminal, parallel drag | `cua-driver-linux-*-gif`, `cua-driver-linux-parallel-drag-xserver` | Cursor/terminal in PR smoke; all in dispatch |
| X11 representative GTK, Qt, Electron apps | `cua-driver-linux-background-gui-{gtk4-characters,qt6-kcalc,electron-zettlr}` | Dispatch |
| Native Wayland integration, capture, cursor, terminal, parallel drag | `cua-driver-wayland-<session>-<scenario>` | Sway representative in PR smoke; all in dispatch |
| Native Wayland GTK and Qt app rows | `cua-driver-wayland-<session>-background-gui-{gtk3-gedit,qt6-kcalc}` | Dispatch |

`E2E: NixOS Linux desktop` runs two bounded PR cells: X11 integration/capture/
cursor and native Sway integration/cursor/background terminal. A maintainer can
run the full matrix for `xfce-labwc`, `xfce-sway`, KDE, and GNOME with
`workflow_dispatch`; each cell uploads a stable `nixos-desktop-*` artifact and
adds its artifact name to the Actions job summary. The NixOS tests copy GIF and
PNG evidence into their result closure. JSON logs and MP4 trajectories are also
uploaded whenever the current recorder emits them.

## Session scope

All four historical practical sessions remain matrix entries and now use their
host compositor sockets: labwc, Sway, KWin, and Mutter. `xfce-wayfire` remains
intentionally excluded: its `wf-config` dependency was
not buildable in the pinned nixpkgs lineage, while labwc and Sway cover the same
wlroots family. This is an upstream packaging limitation, not an equivalence
claim for Wayfire.

## Regression map

- `#2328`, `#2331`, and `#2631`: X11 cursor/click, desktop lock, and uinput/MPX rows.
- `#2565`, `#2573`, `#2574`, and `#2597`: GNOME/KDE native-Wayland input and GUI rows.
- `#2226` and likely `#2552`: native-Wayland screenshot/capture rows.

Run a single check locally with, for example:

```bash
nix build .#checks.x86_64-linux.cua-driver-wayland-xfce-sway-cursor-click-gif --print-build-logs
```
