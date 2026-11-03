# Hyprland plugin requalification, 2026-11-03T06:45:44Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.0-3 | `b6c8e67d904313a1` | main, cua-driver-rs-v0.38.0 (`aa330aebe172`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| arch | 0.57.0-3 | `b6c8e67d904313a1` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-edge | 0.57.0-3 | `b6c8e67d904313a1` | main, cua-driver-rs-v0.38.0 (`aa330aebe172`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-edge | 0.57.0-3 | `b6c8e67d904313a1` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |

## Header and module hashes

| Hyprland | Binary SHA-256 | Header inventory SHA-256 | GCC | Module SHA-256 | Plugin tree |
| --- | --- | --- | --- | --- | --- |
| 0.57.0-3 | `7a7c6b4f3f4a6dba4389d6f12fb7e5c1d6df897e794653825015bc7b69f6339e` | `715a338ee2436f5bcbb0f55015c4d8b38c89f66a2776cdeecb52e0c7d560bb28` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.0-3 | `7a7c6b4f3f4a6dba4389d6f12fb7e5c1d6df897e794653825015bc7b69f6339e` | `715a338ee2436f5bcbb0f55015c4d8b38c89f66a2776cdeecb52e0c7d560bb28` | 16.2.1 20260810 | `` | `aa330aebe172` |

Upstream Hyprland: [v0.57.0](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.0) (packaged).

Omarchy `cua-hyprland-plugin 0.33.2-4` on omarchy-edge: NOT installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
