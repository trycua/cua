# Hyprland plugin requalification, 2026-11-25T18:49:26Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.61.4 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`4fd991010963`) | pass | pass (23/23) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`04c55207f12f`) | pass | pass (22/22) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`22f368905f32`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`01eadb9fc065`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.47.5 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`480da934a4dd`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`440eab868c4c`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`aa330aebe172`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main (`b477f0617505`) | pass | pass (20/20) | pass | pass | pass |
| omarchy-rc | 0.56.2-4 | `89cc2f58bf5a70e9` | main, cua-driver-rs-v0.34.0 (`0dec07dc8358`) | pass | pass (20/20) | pass | pass | pass |
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
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `08626ce8ce18eeacad1d2b53ce79c1c996585821edbacea7a34a6df94807f08d` | `99899b60460b` |
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `0c66fe34feefaf80231a3ad5044099e0dafb74145b14262f70fbf292fe23385e` | `d19599242d42` |
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `08626ce8ce18eeacad1d2b53ce79c1c996585821edbacea7a34a6df94807f08d` | `99899b60460b` |
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `0c66fe34feefaf80231a3ad5044099e0dafb74145b14262f70fbf292fe23385e` | `d19599242d42` |

Upstream Hyprland: [v0.57.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.2) (packaged).

Omarchy `cua-hyprland-plugin 0.47.8-8` on omarchy-edge: NOT installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
