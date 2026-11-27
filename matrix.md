# Hyprland plugin requalification, 2026-11-27T14:49:35Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.2-2 | `9afded099cfa1866` | main, cua-driver-rs-v0.63.1 (`135325af5f0d`) | pass | pass (24/24) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-edge | 0.57.2-9 | `b076c425ecf5ae28` | main, cua-driver-rs-v0.64.1 (`135325af5f0d`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-edge | 0.57.2-9 | `b076c425ecf5ae28` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-9 | `b076c425ecf5ae28` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-rc | 0.57.2-7 | `abbcd3efd3b02da4` | main, cua-driver-rs-v0.63.1 (`135325af5f0d`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-rc | 0.57.2-7 | `abbcd3efd3b02da4` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-rc | 0.57.2-7 | `abbcd3efd3b02da4` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.57.2-7 | `abbcd3efd3b02da4` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.63.1 (`135325af5f0d`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.61.4 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`4fd991010963`) | pass | pass (23/23) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`04c55207f12f`) | pass | pass (22/22) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`22f368905f32`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`01eadb9fc065`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.47.5 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-stable | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |

## Header and module hashes

| Hyprland | Binary SHA-256 | Header inventory SHA-256 | GCC | Module SHA-256 | Plugin tree |
| --- | --- | --- | --- | --- | --- |
| 0.57.2-9 | `1ff4a430ad831b036b5222dce6331f6f8c02743085c2729ef7e34d661ed4b16c` | `51daf766bd83c034cd1bc59734548112511b69e1cff807794d6f42b4eeecbdf5` | 16.2.1 20260810 | `0c66fe34feefaf80231a3ad5044099e0dafb74145b14262f70fbf292fe23385e` | `135325af5f0d` |
| 0.57.2-9 | `1ff4a430ad831b036b5222dce6331f6f8c02743085c2729ef7e34d661ed4b16c` | `51daf766bd83c034cd1bc59734548112511b69e1cff807794d6f42b4eeecbdf5` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-9 | `1ff4a430ad831b036b5222dce6331f6f8c02743085c2729ef7e34d661ed4b16c` | `51daf766bd83c034cd1bc59734548112511b69e1cff807794d6f42b4eeecbdf5` | 16.2.1 20260810 | `08626ce8ce18eeacad1d2b53ce79c1c996585821edbacea7a34a6df94807f08d` | `99899b60460b` |

Upstream Hyprland: [v0.57.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.2) (packaged).

Omarchy `cua-hyprland-plugin 0.47.8-11` on omarchy-edge: installable.

Omarchy `cua-hyprland-plugin 0.47.8-10` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
