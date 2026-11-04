# Cua Hyprland plugin compatibility manifest

This branch is written by the
[`nightly-hyprland-plugin-requalify.yml`](https://github.com/trycua/cua/blob/main/.github/workflows/nightly-hyprland-plugin-requalify.yml)
workflow ([#4909](https://github.com/trycua/cua/issues/4909)). Do not edit it by hand.

- `compatibility.json` is the machine-readable manifest:
  <https://raw.githubusercontent.com/trycua/cua/hyprland-plugin-compat/compatibility.json>
- `matrix.md` is the same data as a table.

Each daily run reads the tracked packages in Arch `core`/`extra` and in Omarchy
edge, rc and stable. The tracked packages are `hyprland`, `aquamarine`,
`hyprutils`, `hyprlang`, `hyprcursor`, `hyprgraphics`, `glibc`, `gcc`,
`libgcc`, `libstdc++`, `libxkbcommon` and `wayland`. A **Hyprland build** is
identified by `abi_key`, a hash over each package's version *and* package
SHA-256. A same-version Arch rebuild therefore gets a new key. For every new
key, the run takes each plugin source (`main`, the latest Driver tag, and the
Driver source each Omarchy channel packages) and does the following:

- builds it in an Arch container against that channel's exact packages and headers;
- runs CTest;
- loads the module into a headless Hyprland session;
- runs a window-targeted background input smoke over `cua-input-v3`.

## Reading it

- `channels.<channel>.abi_key` is the channel's current Hyprland build.
- `current.<channel>.<plugin ref>.status` is the result for that build and plugin source. For example, `current["omarchy-edge"]["cua-driver-rs-v0.32.0"].status` is the result for the source behind Omarchy's `cua-hyprland-plugin 0.32.0-*`.
- `channels.<channel>.omarchy_plugin.installable` is `false` when the channel's packages no longer satisfy the exact pins of Omarchy's packaged plugin, so `pacman -Syu` would refuse the update.
- `entries[]` holds the full records:
  - Hyprland package and header versions;
  - the Hyprland binary SHA-256 and header-inventory SHA-256 (the same method as `profile_verify.py`);
  - the GCC version;
  - runtime packages;
  - the plugin tree, refs and module SHA-256;
  - per-check results.

Statuses:

| Status | Meaning |
| --- | --- |
| `pass` | Built against the exact headers, CTest passed, loaded with a matching ABI, and the input smoke passed. |
| `build-only` | Built and CTest passed, but no headless session was available on the runner. |
| `fail` | A build, test, load or smoke step failed. A tracking issue is opened. |

This is CI requalification, not native certification. It does not produce a
package or kit. A packaged plugin still needs a reviewed profile and kit
(docs/omarchy-edge-20261004-validation.md, "Rebuild and requalification").
