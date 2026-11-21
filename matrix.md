# Hyprland plugin requalification, 2026-11-21T02:45:54Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.2-1 | `070a26aacd10ee27` | main, cua-driver-rs-v0.55.0 (`04c55207f12f`) | pass | pass (22/22) | pass | pass | pass |
| arch | 0.57.2-1 | `070a26aacd10ee27` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-1 | `070a26aacd10ee27` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-edge | 0.57.2-4 | `bb1a98523f8a3e18` | cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-4 | `bb1a98523f8a3e18` | main, cua-driver-rs-v0.56.0 (`04c55207f12f`) | pass | pass (22/22) | pass | pass | pass |
| omarchy-edge | 0.57.2-4 | `bb1a98523f8a3e18` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
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
| 0.57.2-4 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `55b1cbdb0d67c9c505ab6fb4f01ea58df3bed04e414925aaf2d91dadc6eb2efa` | `99899b60460b` |
| 0.57.2-4 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `39e3a94661dbac784d41302eb052d0bc20a33a9c649cc5bf953580a1ecd9416b` | `04c55207f12f` |
| 0.57.2-4 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-3 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `39e3a94661dbac784d41302eb052d0bc20a33a9c649cc5bf953580a1ecd9416b` | `04c55207f12f` |
| 0.57.2-3 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `55b1cbdb0d67c9c505ab6fb4f01ea58df3bed04e414925aaf2d91dadc6eb2efa` | `99899b60460b` |
| 0.57.2-3 | `06d5ce647de97dda3fb14441a720d838f40340cf22c94cc4512a88b7f021cd0b` | `d8506508b80a87013faf6ee9703f1b825b326e74551822f2ed24b3491708924c` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.57.2-2 | `d05468e0704760b89321de5f2db695b302ea11324f35ffb3fad8de9fc003900f` | `15da45b4452fc553c9f769f0f4718c2131603ef4e6f2e50dc08f215afeecf196` | 16.2.1 20260810 | `20a8358c32fcafc010d0cee8383f169cec93e9308741245235b90fbb9b6d016c` | `04c55207f12f` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `20a8358c32fcafc010d0cee8383f169cec93e9308741245235b90fbb9b6d016c` | `04c55207f12f` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `4539f137eda5b74b9a8339818ea34929f963aec0c3010e0d5304d68ce58d4eb8` | `99899b60460b` |
| 0.57.2-2 | `d05468e0704760b89321de5f2db695b302ea11324f35ffb3fad8de9fc003900f` | `15da45b4452fc553c9f769f0f4718c2131603ef4e6f2e50dc08f215afeecf196` | 16.2.1 20260810 | `4539f137eda5b74b9a8339818ea34929f963aec0c3010e0d5304d68ce58d4eb8` | `99899b60460b` |

Upstream Hyprland: [v0.57.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.2) (packaged).

Omarchy `cua-hyprland-plugin 0.47.8-8` on omarchy-edge: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
