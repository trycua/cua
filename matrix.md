# Hyprland plugin requalification, 2026-11-11T22:46:01Z

CI requalification of the Cua Hyprland plugin per Hyprland build. Not native certification and not a package: the reviewed kit/profile process stays the release gate.

| Channel | Hyprland | ABI key | Plugin (refs) | Build | CTest | Load | Input smoke | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arch | 0.57.2-1 | `069d70ecae97ef36` | main (`22f368905f32`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-1 | `069d70ecae97ef36` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| arch | 0.57.2-1 | `069d70ecae97ef36` | main, cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-1 | `069d70ecae97ef36` | main (`01eadb9fc065`) | pass | pass (21/21) | pass | pass | pass |
| arch | 0.57.2-1 | `069d70ecae97ef36` | main, cua-driver-rs-v0.47.5 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-1 | `069d70ecae97ef36` | main (`22f368905f32`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-1 | `069d70ecae97ef36` | cua-driver-rs-v0.33.2 (`5b3d63841bb6`) | unsupported | n/a | n/a | n/a | unsupported |
| omarchy-edge | 0.57.2-1 | `069d70ecae97ef36` | main, cua-driver-rs-v0.47.8 (`99899b60460b`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-1 | `069d70ecae97ef36` | main (`01eadb9fc065`) | pass | pass (21/21) | pass | pass | pass |
| omarchy-edge | 0.57.2-1 | `069d70ecae97ef36` | main, cua-driver-rs-v0.47.5 (`1922be4c6d4a`) | pass | pass (21/21) | pass | pass | pass |
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
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `4539f137eda5b74b9a8339818ea34929f963aec0c3010e0d5304d68ce58d4eb8` | `22f368905f32` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `ae43e57e3495248d373a960ed413182c8e4504e7815f75970935a1b0ca1d3153` | `22f368905f32` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `` | `5b3d63841bb6` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `ae43e57e3495248d373a960ed413182c8e4504e7815f75970935a1b0ca1d3153` | `99899b60460b` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `4539f137eda5b74b9a8339818ea34929f963aec0c3010e0d5304d68ce58d4eb8` | `99899b60460b` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `6b8463c713c83470f8f4f0f8b7733f032f48d7ba1e9f1d6043fec6eeb888c488` | `01eadb9fc065` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `710724a42830a91a2966e2ee2029dd6931f3b120c4119a141e14e1da0774072e` | `01eadb9fc065` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `1b08260f4db02ff26698055ff2001512f2d0c11672bd6ef21723644f23ce508c` | `5b3d63841bb6` |
| 0.56.2-4 | `55da553be71222566ee73b973d2f56dbb9939044d8f22f81aa54b66c83f2f6d1` | `1fdefe6ac027a159d04a5dfee4928ec7ebd15544a9a25b5f66d2f5a46fcf364a` | 16.2.1 20260810 | `0095d0c1de9b13a2ae358120a32981234b5ca7f1be6e1c3ed751de2d94f1ac78` | `1922be4c6d4a` |
| 0.57.2-1 | `2d4d863fa3c1d4c1243324674a9e97fe3cad0203a3715b8054a02953d03988b4` | `816164635c4d3aca63ca58f58edc7a863360ba7b92fd649f9d302381a8244c40` | 16.2.1 20260810 | `89c46bde21d1497955af88a90777e785cdd6d09aa42eb6a0e022778b82253222` | `1922be4c6d4a` |

Upstream Hyprland: [v0.57.2](https://github.com/hyprwm/Hyprland/releases/tag/v0.57.2) (packaged).

Omarchy `cua-hyprland-plugin 0.47.8-2` on omarchy-edge: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-rc: installable.

Omarchy `cua-hyprland-plugin 0.33.2-3` on omarchy-stable: installable.
