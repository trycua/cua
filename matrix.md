# Hyprland plugin requalification, 2026-11-08T06:45:48Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.1-1 | `3add16bbea41d56f` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| arch | 0.57.1-1 | `3add16bbea41d56f` | main, cua-driver-rs-v0.41.2 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.1-1 | `3add16bbea41d56f` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| arch | 0.57.1-1 | `3add16bbea41d56f` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| arch | 0.57.1-1 | `3add16bbea41d56f` | cua-driver-rs-v0.38.0 (`aa330aebe172`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-edge | 0.57.1-1 | `3add16bbea41d56f` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-edge | 0.57.1-1 | `3add16bbea41d56f` | main, cua-driver-rs-v0.41.2 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.1-1 | `3add16bbea41d56f` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-edge | 0.57.1-1 | `3add16bbea41d56f` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-edge | 0.57.1-1 | `3add16bbea41d56f` | cua-driver-rs-v0.38.0 (`aa330aebe172`) | **FAIL** | n/a | n/a | n/a | **FAIL** |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.41.2 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.41.2 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.32.0 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |

## Header and module hashes

| Hyprland | Binary SHA-256 | Header inventory SHA-256 | GCC | Module SHA-256 | Plugin tree |
| --- | --- | --- | --- | --- | --- |
| 0.57.1-1 | `38e5b948a7325ab2b0acff5a8d0fe3542e6c55cd608c4d944b2ea8a752b769b8` | `d83f1e2a75619f934b0a663237de8c269961f4758673be8f6883aec3bb51f2d5` | 16.2.1 20260810 | `` | `5b3d63841bb6` |

Upstream Hyprland: [v0.57.1](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.1) (packaged).

Omarchy `cua-hyprland-plugin 0.33.2-4` on omarchy-edge: NOT installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
