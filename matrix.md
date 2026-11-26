# Hyprland plugin requalification, 2026-11-26T02:46:10Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| arch | 0.57.2-2 | `9afded099cfa1866` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-rc | 0.57.2-5 | `d200226cf375baa7` | main, cua-driver-rs-v0.62.1 (`d19599242d42`) | pass | pass (24/24) | pass | pass | pass |
| omarchy-rc | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-rc | 0.57.2-5 | `d200226cf375baa7` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
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
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `0c66fe34feefaf80231a3ad5044099e0dafb74145b14262f70fbf292fe23385e` | `d19599242d42` |
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `08626ce8ce18eeacad1d2b53ce79c1c996585821edbacea7a34a6df94807f08d` | `99899b60460b` |
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `08626ce8ce18eeacad1d2b53ce79c1c996585821edbacea7a34a6df94807f08d` | `99899b60460b` |
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `0c66fe34feefaf80231a3ad5044099e0dafb74145b14262f70fbf292fe23385e` | `d19599242d42` |
| 0.57.2-5 | `e77ed7567161a8aa789ebb0bbe3a407f5937e4ec28b900dc4d512afbf39d8f76` | `e35c07e1759d38cf47250ffd517797de5b330856f86e0c5c54f529f04fb9bced` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-2 | `d9117121713a84d5b0774f426e3f2f5a9baecb8c02e11726c813429de61ba427` | `546d1e2fd886d9d62f3abd8cf3e0df35b47c6bfa270499171bcc720a8869e345` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-4 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `d9113f0c00858527bddb5f2438f88eb9c8b47e477053b099a283bd02c4833e2e` | `d19599242d42` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `f094bbdbef92b7a14f1365fe4600f18d4612c54ba86c782dd93d71c1b6959e79` | `d19599242d42` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `dfe261eaaa0064125ef877cc01a6b5fd2514367523b72e1a96388d2230eb2a20` | `d19599242d42` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `06350ff253803c7ae2e57478e146566db9099c040d35dd323920f6d9636329a3` | `4fd991010963` |

Upstream Hyprland: [v0.57.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.2) (packaged).

Omarchy `cua-hyprland-plugin 0.47.8-9` on omarchy-edge: installable.

Omarchy `cua-hyprland-plugin 0.47.8-9` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
