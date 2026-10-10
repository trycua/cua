# Hyprland plugin requalification, 2026-10-10T07:02:22Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| arch | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| arch | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-edge | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-edge | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-edge | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-2 | `c80eed71005fef40` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-2 | `c80eed71005fef40` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-2 | `c80eed71005fef40` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-2 | `c80eed71005fef40` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-2 | `c80eed71005fef40` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-2 | `c80eed71005fef40` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |

## Header and module hashes

| Hyprland | Binary SHA-256 | Header inventory SHA-256 | GCC | Module SHA-256 | Plugin tree |
| --- | --- | --- | --- | --- | --- |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `1b08260f4db02ff26698055ff2001512f2d0c11672bd6ef21723644f23ce508c` | `b477f0617505` |
| 0.56.2-2 | `da8fcacf347bcbed83edc40108c6e2298da095e22246bd764e9bb382786cebb2` | `88a6875af00203627b264a5c1f9908781be4ad8d9cee4e577fef174e72dd0e28` | 16.2.1 20260810 | `17ce4c243b81665770e0429c7301224a20b4f14de28d050d9fa43ede1958f9d2` | `b477f0617505` |

Upstream Hyprland: [v0.56.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.56.2) (packaged).

Omarchy `cua-hyprland-plugin 0.32.0-3` on omarchy-edge: installable.
