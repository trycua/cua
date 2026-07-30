# CUA Driver native-Wayland NixOS suite

These checks run the packaged driver against native headless Wayland sessions:
XFCE/labwc, XFCE/Sway, KDE/KWin, and GNOME/Mutter. The session wrapper exports
the host compositor socket and unsets `DISPLAY`, so the driver and test apps
cannot silently fall back to X11. Each scenario uses the MCP service and copies
visual evidence from the NixOS test machine into the test result closure.

The full matrix is exposed from `flake.nix` as:

- integration/service startup;
- screenshot/capture;
- cursor/click;
- background terminal input;
- parallel drag; and
- representative foot, GTK, and Qt background-GUI rows.

KWin and Mutter entries intentionally exercise their real host socket rather
than the historical nested labwc workaround. This keeps native input and capture behavior observable in artifact evidence. `xfce-wayfire` remains excluded because the
pinned nixpkgs lineage cannot build its `wf-config` dependency; labwc and Sway
continue to cover the wlroots session family.

Run a single entry with:

```bash
nix build .#checks.x86_64-linux.cua-driver-wayland-gnome-cursor-click-gif --print-build-logs
```
