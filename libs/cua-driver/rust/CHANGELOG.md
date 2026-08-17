# Changelog

## [0.20.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.19.3...cua-driver-rs-v0.20.0) (2026-08-15)


### ⚠ BREAKING CHANGES

* **cua-driver:** remove browser approval tokens ([#3185](https://github.com/trycua/cua/issues/3185))

### Features

* add immutable Driver and Lume nightly releases ([8cef90b](https://github.com/trycua/cua/commit/8cef90b7ce6f85793a0ca3a3ddc050eac01b6b83))
* add persistent Driver and Lume release channels ([0813659](https://github.com/trycua/cua/commit/0813659ac237cd46da21c2a51f9a95f27a3e7845))
* **cua-driver:** add implicit lifecycle sessions ([#3013](https://github.com/trycua/cua/issues/3013)) ([858ef13](https://github.com/trycua/cua/commit/858ef13a5b1f2630b074d8f588a25cb0b19cab25))
* **cua-driver:** apply capability manifests across permission profiles ([#3015](https://github.com/trycua/cua/issues/3015)) ([3a53e53](https://github.com/trycua/cua/commit/3a53e53f99dcf8d5ed761c5cd0b5dd039c916121))
* **cua-driver:** remove browser approval tokens ([#3185](https://github.com/trycua/cua/issues/3185)) ([0d7c011](https://github.com/trycua/cua/commit/0d7c011eb31526d9c2b500806f35523dc2bdfb0e))


### Bug Fixes

* **cua-driver:** align agent guidance with lifecycle sessions ([#3041](https://github.com/trycua/cua/issues/3041)) ([dbde09e](https://github.com/trycua/cua/commit/dbde09e0bfd39abbc374e2e9aade86086e773bb8))
* **cua-driver:** hide policy-disabled MCP tools ([#3132](https://github.com/trycua/cua/issues/3132)) ([1532830](https://github.com/trycua/cua/commit/1532830d3d5c6e5db22198130d82a4741a7b4a2f))
* **cua-driver:** preserve direct MCP session ownership ([#3079](https://github.com/trycua/cua/issues/3079)) ([d509687](https://github.com/trycua/cua/commit/d5096871d088c2e5b5942d678f2c127840061d7d))
* **cua-driver:** preserve named CLI sessions ([#3144](https://github.com/trycua/cua/issues/3144)) ([1803e52](https://github.com/trycua/cua/commit/1803e52d0c4a94d88787706cfe89eccb6dc723db))
* **cua-driver:** reject misplaced MCP permission flags ([#3085](https://github.com/trycua/cua/issues/3085)) ([a5bc265](https://github.com/trycua/cua/commit/a5bc2651c10dc64f4779f2c9fd99bfddb9905ffb))
* **cua-driver:** stage current skill pack on Windows local installs ([#3083](https://github.com/trycua/cua/issues/3083)) ([b4bc58a](https://github.com/trycua/cua/commit/b4bc58a3a262f30fcd9d6772851cde0f3429deb6))
* **cua-driver:** verify foreground focus before input ([#3068](https://github.com/trycua/cua/issues/3068)) ([f9192dd](https://github.com/trycua/cua/commit/f9192dde69dd85ff6c5ad0bf4731d33ebc7d20d3))
* **cua-driver:** write the update-check cache to the canonical home ([#3032](https://github.com/trycua/cua/issues/3032)) ([69e5774](https://github.com/trycua/cua/commit/69e57742751407258f2189ec2c4bb21fba78da2e))

## [0.19.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.19.2...cua-driver-rs-v0.19.3) (2026-08-10)


### Bug Fixes

* **cua-driver:** remove Windows npm VC runtime prerequisite ([#3038](https://github.com/trycua/cua/issues/3038)) ([c167125](https://github.com/trycua/cua/commit/c167125e1bb0c4fe515beddc0d480f573f3c6077))

## [0.19.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.19.1...cua-driver-rs-v0.19.2) (2026-08-07)


### Bug Fixes

* **cua-driver:** constrain incompatible zune-core resolution ([#2984](https://github.com/trycua/cua/issues/2984)) ([1979ab7](https://github.com/trycua/cua/commit/1979ab73fdcdba130a54467ca7072c447011f7ab))
* **cua-driver:** make MCP output schemas object-rooted ([96d87ad](https://github.com/trycua/cua/commit/96d87adb248d746aed2a5427b1a256dbd3f1ae1a))
* **cua-driver:** reap Linux launch_app children instead of leaking zombies ([#2974](https://github.com/trycua/cua/issues/2974)) ([f91d78e](https://github.com/trycua/cua/commit/f91d78e49a74fd7a690100957886e93e185655e2))

## [0.19.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.19.0...cua-driver-rs-v0.19.1) (2026-08-07)


### Bug Fixes

* **cua-driver:** advertise refusals in the MCP outputSchema ([#2968](https://github.com/trycua/cua/issues/2968)) ([dc6f32c](https://github.com/trycua/cua/commit/dc6f32cd4d32bb0a60e18674086f443f9d9d8288))
* **cua-driver:** record launch provenance for sessionless launch_app so kill_app can reprove it ([#2966](https://github.com/trycua/cua/issues/2966)) ([b065550](https://github.com/trycua/cua/commit/b06555075fc0ccc14df7e17d33b7eb27d97c9c53)), closes [#2965](https://github.com/trycua/cua/issues/2965)
* **cua-driver:** resolve Linux launch_app names via .desktop entries and surface xdg-open failures ([#2954](https://github.com/trycua/cua/issues/2954)) ([fb4b492](https://github.com/trycua/cua/commit/fb4b492cd997dd3e8659298adfd98825974ebb7e))
* **cua-driver:** survive z-order BadMatch under reparenting WMs so the Linux agent cursor can paint ([#2957](https://github.com/trycua/cua/issues/2957)) ([9522358](https://github.com/trycua/cua/commit/9522358294f34aba138221e87d3dbb79486b199a)), closes [#2955](https://github.com/trycua/cua/issues/2955)

## [0.19.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.18.0...cua-driver-rs-v0.19.0) (2026-08-06)


### Features

* **cua-driver:** support Prime Agent skill onboarding ([#2944](https://github.com/trycua/cua/issues/2944)) ([0f796d0](https://github.com/trycua/cua/commit/0f796d0659877bee3404d11eef566cfbcbf8edf1))


### Bug Fixes

* **cua-driver:** Linux cursor overlay compositing, xrdp-family servers, and resting float ([#2940](https://github.com/trycua/cua/issues/2940)) ([997d6d8](https://github.com/trycua/cua/commit/997d6d86433254ccf12e5dbf1edcb46bf05eb5ac))
* **cua-driver:** resolve SDK release from run artifacts ([#2926](https://github.com/trycua/cua/issues/2926)) ([3cd9de9](https://github.com/trycua/cua/commit/3cd9de9a32bf6b4a98a2186c1535054bd740bb21))

## [0.18.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.17.0...cua-driver-rs-v0.18.0) (2026-08-05)


### Features

* **cua-driver:** add macOS Spaces awareness to list_windows ([#2850](https://github.com/trycua/cua/issues/2850)) ([f7404b4](https://github.com/trycua/cua/commit/f7404b40287c128b15afa30b6c6f9024cbf081b8))
* **cua-driver:** drop the session orb from the cursor badge ([#2842](https://github.com/trycua/cua/issues/2842)) ([59f280e](https://github.com/trycua/cua/commit/59f280e485be4f58811bda6a9e2dbe7632b9b63c))
* **cua-driver:** exact-target macOS background input v1 ([#2837](https://github.com/trycua/cua/issues/2837)) ([1b2cb5a](https://github.com/trycua/cua/commit/1b2cb5a706c3e5d636b683ab15336dbf35e579e0))


### Bug Fixes

* **cua-driver:** avoid accessibility advertise on Cinnamon ([#2784](https://github.com/trycua/cua/issues/2784)) ([b3ba121](https://github.com/trycua/cua/commit/b3ba12126188e1540b93d7c51d55926299fd36ab))
* **cua-driver:** avoid retrying deferred text writes ([7e0d6bc](https://github.com/trycua/cua/commit/7e0d6bcb5d7ca55d9da1ff3e8acc1bdddf6fa422))
* **cua-driver:** bound large macOS text synthesis ([#2863](https://github.com/trycua/cua/issues/2863)) ([a6b87e0](https://github.com/trycua/cua/commit/a6b87e01af9c3f291a40a9d7f4e6301418b2a8ab))
* **cua-driver:** bound Linux AT-SPI listener startup ([#2827](https://github.com/trycua/cua/issues/2827)) ([ed3c365](https://github.com/trycua/cua/commit/ed3c365c954446c774c422e8018541e56155dd6a))
* **cua-driver:** bound Linux snapshots after app exit ([#2822](https://github.com/trycua/cua/issues/2822)) ([d1c5a7a](https://github.com/trycua/cua/commit/d1c5a7a90b23a37a18838d9c2122d003d00e7ecf))
* **cua-driver:** bound Windows app-name lookup ([#2858](https://github.com/trycua/cua/issues/2858)) ([2be7aab](https://github.com/trycua/cua/commit/2be7aab3de35bdffa9d8ad767471c0699d36d6e5))
* **cua-driver:** bound Windows installed-app discovery ([#2855](https://github.com/trycua/cua/issues/2855)) ([ea971ef](https://github.com/trycua/cua/commit/ea971ef06d4ebeda16c6947cb2bca2dd875d2ddc))
* **cua-driver:** clarify synthetic browser click outcomes ([#2866](https://github.com/trycua/cua/issues/2866)) ([197c518](https://github.com/trycua/cua/commit/197c518c9c364bd30da8bc076748ea82ed093e27))
* **cua-driver:** explain permanent scope recovery ([#2857](https://github.com/trycua/cua/issues/2857)) ([9110d9f](https://github.com/trycua/cua/commit/9110d9fd69c7fda41caab3aaac9df80c87bdab2f))
* **cua-driver:** fail closed on X11 overlay shape errors ([#1818](https://github.com/trycua/cua/issues/1818)) ([0462954](https://github.com/trycua/cua/commit/0462954cfb9843ca0cc43f49262acf0e69e3517d))
* **cua-driver:** hard-bound Windows UIA provider calls ([#2117](https://github.com/trycua/cua/issues/2117)) ([c9c6607](https://github.com/trycua/cua/commit/c9c660770a26c2f0024b9be1079fac0d4c9effff))
* **cua-driver:** harden Windows discovery and foreground input ([#2812](https://github.com/trycua/cua/issues/2812)) ([441cae0](https://github.com/trycua/cua/commit/441cae0d4d4048524ce2f7eeabfb63a6c9a422d3))
* **cua-driver:** ignore macOS compositor sibling surfaces ([#2908](https://github.com/trycua/cua/issues/2908)) ([df57e61](https://github.com/trycua/cua/commit/df57e610d3f2e9c07aac59d4896d48656490e1cc))
* **cua-driver:** keep Windows autostart on the junction path ([#2809](https://github.com/trycua/cua/issues/2809)) ([06f0c04](https://github.com/trycua/cua/commit/06f0c048fb11b1780e61252eb41a2b968865a8a7))
* **cua-driver:** keep Windows cursor visible without focus theft ([#2864](https://github.com/trycua/cua/issues/2864)) ([2e7a412](https://github.com/trycua/cua/commit/2e7a41215ec9e3594567029aad07450c3d731721))
* **cua-driver:** keep remote debugging enabled when a restart is needed ([#2911](https://github.com/trycua/cua/issues/2911)) ([f5b15cc](https://github.com/trycua/cua/commit/f5b15cccf6a9f0424dd2d83f730b523c4be21df0))
* **cua-driver:** launch desktop AppsFolder apps on Windows ([#2862](https://github.com/trycua/cua/issues/2862)) ([fb2a6f6](https://github.com/trycua/cua/commit/fb2a6f60522ebf132270a3377c9a0496d2c0ba98))
* **cua-driver:** prevent Linux uinput pointer panics ([#2736](https://github.com/trycua/cua/issues/2736)) ([9fde53e](https://github.com/trycua/cua/commit/9fde53e1fb190834054eb36e7574bc0b523831e2))
* **cua-driver:** quiesce idle Wayland overlay ([#2828](https://github.com/trycua/cua/issues/2828)) ([35b71a0](https://github.com/trycua/cua/commit/35b71a0ebd081c333bf53af6b386ef4798dd05b1))
* **cua-driver:** refuse minimized Windows element actions ([8a9b7ba](https://github.com/trycua/cua/commit/8a9b7baa32bb5b7ea48a1ee6ffc1d87a433405e4))
* **cua-driver:** repair X11 overlay after display changes ([#2354](https://github.com/trycua/cua/issues/2354)) ([81019b5](https://github.com/trycua/cua/commit/81019b54b89d577ccb5d7758eba0fd33861fc9d9))
* **cua-driver:** report honest macOS browser chrome coverage ([#2919](https://github.com/trycua/cua/issues/2919)) ([7c52b84](https://github.com/trycua/cua/commit/7c52b8449fab36cdda9fe4310e6afd3be202c955))
* **cua-driver:** report macOS key delivery truthfully ([#2830](https://github.com/trycua/cua/issues/2830)) ([3dc0fc0](https://github.com/trycua/cua/commit/3dc0fc0e345f6d8107899a18c74245615e294c03))
* **cua-driver:** report PostMessage pixel fallback truthfully ([4dc537e](https://github.com/trycua/cua/commit/4dc537ef34e715c099ab30c7d45618e791da2ad1))
* **cua-driver:** scope the Linux AT-SPI walk to the requested window ([#2895](https://github.com/trycua/cua/issues/2895)) ([8c515ca](https://github.com/trycua/cua/commit/8c515ca74d7f9dad2b3ae9d74b59294511824fa1))
* **cua-driver:** select the correct Linux AT-SPI application tree ([#2740](https://github.com/trycua/cua/issues/2740)) ([d86e49b](https://github.com/trycua/cua/commit/d86e49ba4bf1c456fd16eca220184a38600ed927))
* **cua-driver:** stage local installs over a running driver ([#2889](https://github.com/trycua/cua/issues/2889)) ([157816e](https://github.com/trycua/cua/commit/157816e6144a79162253762b79d0abdfffa8e486))
* **cua-driver:** stop Windows update --apply from killing itself ([#2805](https://github.com/trycua/cua/issues/2805)) ([aebd996](https://github.com/trycua/cua/commit/aebd9962d2686e75ff9e17e0a3735e303ff96981))
* **cua-driver:** verify exact macOS window activation ([#2829](https://github.com/trycua/cua/issues/2829)) ([cd5c6f3](https://github.com/trycua/cua/commit/cd5c6f3ec5c2f71a8fbc64563d42933d381008ba))


### Performance Improvements

* **cua-driver:** accelerate Linux X11 capture with MIT-SHM ([#2796](https://github.com/trycua/cua/issues/2796)) ([5448c44](https://github.com/trycua/cua/commit/5448c449c81c81a7e48a418f8276d257f0e4c563))
* **cua-driver:** accelerate macOS window capture with ScreenCaptureKit ([#2795](https://github.com/trycua/cua/issues/2795)) ([bc90373](https://github.com/trycua/cua/commit/bc90373362cb7c521b1ff03f94457d5de618095c))

## [0.17.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.16.0...cua-driver-rs-v0.17.0) (2026-08-02)


### ⚠ BREAKING CHANGES

* **cua-driver:** make native desktop actions snapshot-safe ([#2783](https://github.com/trycua/cua/issues/2783))

### Features

* **cua-driver:** make native desktop actions snapshot-safe ([#2783](https://github.com/trycua/cua/issues/2783)) ([d8ae6df](https://github.com/trycua/cua/commit/d8ae6df643df5049505a327b88abc2644a25b209))
* **cua-driver:** make native selection and editing foreground-safe ([#2789](https://github.com/trycua/cua/issues/2789)) ([815013a](https://github.com/trycua/cua/commit/815013a88c02d2c8c3e40f98d41fac84f037f91d))


### Bug Fixes

* **cua-driver:** focus exact target for native menus ([#2788](https://github.com/trycua/cua/issues/2788)) ([2269fa3](https://github.com/trycua/cua/commit/2269fa352a801bcf1505ac3af8b125db742a4b82))

## [0.16.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.15.0...cua-driver-rs-v0.16.0) (2026-08-01)


### Features

* **cua-driver:** add verified semantic window framing ([#2772](https://github.com/trycua/cua/issues/2772)) ([fb39144](https://github.com/trycua/cua/commit/fb3914475b9cba97376a6b9b739077cc0994d15f))


### Bug Fixes

* **cua-driver:** prefer semantic routes before GUI fallback ([6f2faf9](https://github.com/trycua/cua/commit/6f2faf9f9c0ea2db3d0b731d45c6d80a9de23db5))

## [0.15.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.14.2...cua-driver-rs-v0.15.0) (2026-08-01)


### ⚠ BREAKING CHANGES

* **cua-driver:** standardize action results ([#2713](https://github.com/trycua/cua/issues/2713))

### Features

* **cua-driver:** add checked state verification ([#2705](https://github.com/trycua/cua/issues/2705)) ([84440dd](https://github.com/trycua/cua/commit/84440ddc6d42accb629f2289012241ed512e1aea))
* **cua-driver:** add clipboard read and write primitives ([#2764](https://github.com/trycua/cua/issues/2764)) ([b8fb6d1](https://github.com/trycua/cua/commit/b8fb6d1218d84ea1806ebce2dec5f22edda29c21))
* **cua-driver:** standardize action results ([#2713](https://github.com/trycua/cua/issues/2713)) ([8e0a92e](https://github.com/trycua/cua/commit/8e0a92e3dbf20134be9922f9e0dc847addcc92fa))


### Bug Fixes

* **cua-driver:** bound macOS capture permission probe ([#2761](https://github.com/trycua/cua/issues/2761)) ([14b790f](https://github.com/trycua/cua/commit/14b790f766fe177aa047c88975e301a49a17cc4c))
* **cua-driver:** define list_windows z-index contract ([#2768](https://github.com/trycua/cua/issues/2768)) ([5fe669f](https://github.com/trycua/cua/commit/5fe669ff43897f305aacb2442d8c5084607daf66))
* **cua-driver:** index writable native value controls ([46fc5cb](https://github.com/trycua/cua/commit/46fc5cb7c2b477ba3194a792580dab4a595a5ec5))
* **cua-driver:** open Finder folder URLs reliably ([13057dc](https://github.com/trycua/cua/commit/13057dc6c8c08da71b8e06a63831ccb5b6870bc5))
* **cua-driver:** preserve verified selection readbacks ([dc0af9b](https://github.com/trycua/cua/commit/dc0af9b23ca8d866540b9ed2b4e4ad7b8b0c4ffb))
* **cua-driver:** reject ambiguous PID-only targets ([#2763](https://github.com/trycua/cua/issues/2763)) ([5b95f2a](https://github.com/trycua/cua/commit/5b95f2a888b84f1e892635b20484b69d27122630))
* **cua-driver:** reject concurrent text input per process ([#2762](https://github.com/trycua/cua/issues/2762)) ([07d1242](https://github.com/trycua/cua/commit/07d124258d83c2a53b0739463a925eaa3477066b))
* **cua-driver:** select macOS collection items reliably ([4be90ea](https://github.com/trycua/cua/commit/4be90ea11345535decb1baacde14c0313338f1c6))

## [0.14.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.14.1...cua-driver-rs-v0.14.2) (2026-07-31)


### Bug Fixes

* **cua-driver:** preserve Screen Sharing keyboard input ([#2698](https://github.com/trycua/cua/issues/2698)) ([61b1a84](https://github.com/trycua/cua/commit/61b1a84c9ede937fcc3f0c8a78aa78ac79d1421e))
* **cua-driver:** reconcile macOS launch readiness ([#2697](https://github.com/trycua/cua/issues/2697)) ([771e016](https://github.com/trycua/cua/commit/771e0165e7ecd2566cbb17b793a863755a8d651f))

## [0.14.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.14.0...cua-driver-rs-v0.14.1) (2026-07-29)


### Bug Fixes

* **cua-driver:** advance installers only after release publication ([dad42d1](https://github.com/trycua/cua/commit/dad42d1932d9683a78b344bad6f058944446a59a))
* **cua-driver:** fall back to the releases API when the baked version has no assets ([1be9c7e](https://github.com/trycua/cua/commit/1be9c7ed0d9aa7b6756727726350a39a6160c3d8))

## [0.14.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.13.1...cua-driver-rs-v0.14.0) (2026-07-29)


### ⚠ BREAKING CHANGES

* **cua-driver:** move cursor context into session badges ([#2677](https://github.com/trycua/cua/issues/2677))

### Features

* **cua-driver:** move cursor context into session badges ([#2677](https://github.com/trycua/cua/issues/2677)) ([11c4647](https://github.com/trycua/cua/commit/11c4647128a99b2879f31a3f4eedc6b08d52c079))


### Bug Fixes

* **cua-driver:** avoid starting Orca on COSMIC ([#2666](https://github.com/trycua/cua/issues/2666)) ([069272e](https://github.com/trycua/cua/commit/069272e0b97114559f2edc22e06b2f9cf5847847))
* **cua-driver:** declare browser chrome capture coverage ([#2599](https://github.com/trycua/cua/issues/2599)) ([a36865c](https://github.com/trycua/cua/commit/a36865c236e7b70f67d974fb80e2b0119929b636))
* **cua-driver:** keep cursor badges and drags in sync ([#2676](https://github.com/trycua/cua/issues/2676)) ([b249df8](https://github.com/trycua/cua/commit/b249df857994ebaebf9f02248e8312b3bd2a5dd2))
* **cua-driver:** refine session cursor feedback ([#2665](https://github.com/trycua/cua/issues/2665)) ([d366af0](https://github.com/trycua/cua/commit/d366af084e5357a570891fcf2c77c14811d9b617))

## [0.13.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.6...cua-driver-rs-v0.13.1) (2026-07-28)


### ⚠ BREAKING CHANGES

* **cua-driver:** simplify permissions and add session identity ([#2616](https://github.com/trycua/cua/issues/2616))
* **cua-driver:** ship semantic cursor themes
* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341))

### Features

* **cua-driver/macos:** implement page click_element + fix Chromium window targeting ([#2082](https://github.com/trycua/cua/issues/2082)) ([73fe822](https://github.com/trycua/cua/commit/73fe8227982c0b03795669e4cdf62384f0d54a34))
* **cua-driver:** add aggregate agent session telemetry ([aa41707](https://github.com/trycua/cua/commit/aa417076c7d31df9d78dc72e982b48b5506e1695))
* **cua-driver:** add aggregate agent session telemetry ([3a59246](https://github.com/trycua/cua/commit/3a5924640738e9881efc11a80b6e26ed6ef5ed0a))
* **cua-driver:** add bounded feature telemetry ([4972560](https://github.com/trycua/cua/commit/4972560226ea3ff81c5d07ca3e867fbaa5238f69))
* **cua-driver:** add bounded feature telemetry ([ebfb4eb](https://github.com/trycua/cua/commit/ebfb4eb7fa48e028d4a64ec94ddda2f466d8313d))
* **cua-driver:** add browser dialogs and uploads ([21ca689](https://github.com/trycua/cua/commit/21ca68928e546103a59ad851699a50e543ff601b))
* **cua-driver:** add browser telemetry contract ([fa13890](https://github.com/trycua/cua/commit/fa138903f779b33b4c8512c7d64bb960023161fe))
* **cua-driver:** add browser telemetry contract ([#2310](https://github.com/trycua/cua/issues/2310)) ([763a6ea](https://github.com/trycua/cua/commit/763a6ea21b86ad5f8a70ab2581edee1dd7370efa))
* **cua-driver:** add capability-aware browser tools ([#2257](https://github.com/trycua/cua/issues/2257)) ([0835daa](https://github.com/trycua/cua/commit/0835daa6d415c857c1d3ebefe2c29453a0bed923))
* **cua-driver:** add client and modality telemetry ([#2441](https://github.com/trycua/cua/issues/2441)) ([5cd0000](https://github.com/trycua/cua/commit/5cd0000e2e018a835303a8f8bdd82a88ea483a6e))
* **cua-driver:** add local consent UI and complete Wayland certification ([#2597](https://github.com/trycua/cua/issues/2597)) ([e9852cb](https://github.com/trycua/cua/commit/e9852cb68494e9f521c7734206f638773dbc3299))
* **cua-driver:** add persistent macOS interactive input sessions ([2dad3e5](https://github.com/trycua/cua/commit/2dad3e519e17b27eaa793151b8671957f578072c))
* **cua-driver:** add protected permission modes and consent grants ([#2383](https://github.com/trycua/cua/issues/2383)) ([c75e606](https://github.com/trycua/cua/commit/c75e60636c11e21ef44f1ebbe1c1350339bae295))
* **cua-driver:** add Rust-owned embedded host for SDK and MCP ([#2427](https://github.com/trycua/cua/issues/2427)) ([5016dc1](https://github.com/trycua/cua/commit/5016dc16bfe54c165e6678104ec521a6d85f76db))
* **cua-driver:** add semantic browser snapshots ([1a9fbb4](https://github.com/trycua/cua/commit/1a9fbb480605f72c449d2c741c19ae32ed5659d8))
* **cua-driver:** add semantic browser snapshots ([#2301](https://github.com/trycua/cua/issues/2301)) ([e83d2d3](https://github.com/trycua/cua/commit/e83d2d3d6142fee78edca6352790db4336abacbb))
* **cua-driver:** add stable health_report MCP tool for end-to-end driver diagnostics ([be761fa](https://github.com/trycua/cua/commit/be761fac796d3f266d56ed7ce89c5a5ff6a89eac))
* **cua-driver:** add true in-process SDK runtime ([#2461](https://github.com/trycua/cua/issues/2461)) ([617508a](https://github.com/trycua/cua/commit/617508a7ae123f277b203d31fca29933927a4636))
* **cua-driver:** add versioned native ABI ([b172afc](https://github.com/trycua/cua/commit/b172afc75f39832a4bdfaa9040d6d4f556449b49))
* **cua-driver:** browser_type can replace a field's content, not only append ([#2624](https://github.com/trycua/cua/issues/2624)) ([eb0b5a0](https://github.com/trycua/cua/commit/eb0b5a0c72cd1203e1dcaab768cdf37c33868302))
* **cua-driver:** capture inactive tabs and retain modal controls ([#2426](https://github.com/trycua/cua/issues/2426)) ([c4d7ddc](https://github.com/trycua/cua/commit/c4d7ddc5bc7c00faf3e9102bee664ea47b2f5fac))
* **cua-driver:** centralize protected resource grants per runtime ([#2577](https://github.com/trycua/cua/issues/2577)) ([dfb3781](https://github.com/trycua/cua/commit/dfb3781460f04f9b523871b2f478233a59e5d364))
* **cua-driver:** complete browser mutations ([b4007a8](https://github.com/trycua/cua/commit/b4007a8442d3f3238226b5d5d11feda8da13c0af))
* **cua-driver:** complete telemetry lifecycle coverage ([f06b7cf](https://github.com/trycua/cua/commit/f06b7cf26f44721edd968f1dd072eb395212dd10))
* **cua-driver:** complete telemetry lifecycle coverage ([7ac5836](https://github.com/trycua/cua/commit/7ac5836714e99f945e13e840d400f0a637edf56d))
* **cua-driver:** complete the browser action surface ([#2323](https://github.com/trycua/cua/issues/2323)) ([01a9505](https://github.com/trycua/cua/commit/01a9505aeac5d2a3afc57a7b1ce98523b58929ae))
* **cua-driver:** desktop-scope Phase 1 — capture_scope config, get_desktop_state, Windows screen-absolute actions ([#1968](https://github.com/trycua/cua/issues/1968)) ([#2019](https://github.com/trycua/cua/issues/2019)) ([fc27185](https://github.com/trycua/cua/commit/fc271854149771adbb9f2e71ad64e58de5087de7))
* **cua-driver:** embedded mode — inherit the host app's TCC grants, never prompt ([#2102](https://github.com/trycua/cua/issues/2102)) ([b654f27](https://github.com/trycua/cua/commit/b654f27d609ecbac22ea63a000c868c90c0ee44d))
* **cua-driver:** enforce consequential and privileged actions ([#2585](https://github.com/trycua/cua/issues/2585)) ([df368a7](https://github.com/trycua/cua/commit/df368a7698a07ec9ba7e10952d483457c22841ff))
* **cua-driver:** enforce desktop input grants ([#2580](https://github.com/trycua/cua/issues/2580)) ([2b5e938](https://github.com/trycua/cua/commit/2b5e938d7014ccdced5abe6d1c4d493063f67b56))
* **cua-driver:** enforce exact file transfer grants ([#2581](https://github.com/trycua/cua/issues/2581)) ([3f74894](https://github.com/trycua/cua/commit/3f748946ecc176d2d7a71add246c5647acca2be3))
* **cua-driver:** enforce private observation grants ([#2579](https://github.com/trycua/cua/issues/2579)) ([6052381](https://github.com/trycua/cua/commit/6052381e97446cca3338221eb888c49e57356890))
* **cua-driver:** expose macOS control state in window elements ([db281a3](https://github.com/trycua/cua/commit/db281a3b0d9bbabaf74f3c6847adf6abbb66995a))
* **cua-driver:** expose trusted protected-consent hosts ([#2578](https://github.com/trycua/cua/issues/2578)) ([fd4069e](https://github.com/trycua/cua/commit/fd4069ec6de49e9fa499e3057e874b9dae1d9665))
* **cua-driver:** fall back to key events when type-target is a terminal ([687d908](https://github.com/trycua/cua/commit/687d908f5efe19ddfd0915f6e654ffb9118bea01))
* **cua-driver:** implement SDK-owned runtime RFC ([#2561](https://github.com/trycua/cua/issues/2561)) ([1760f25](https://github.com/trycua/cua/commit/1760f253d3c4d76618a8c97a04f2c100ffc491ac))
* **cua-driver:** make permission adapter inventory mode-aware ([#2576](https://github.com/trycua/cua/issues/2576)) ([83a2f6f](https://github.com/trycua/cua/commit/83a2f6fad7b2f3349be40433e343cd8f934495eb))
* **cua-driver:** make SDK-owned runtime the default ([#2545](https://github.com/trycua/cua/issues/2545)) ([a62e821](https://github.com/trycua/cua/commit/a62e8219255f15f6ed747c03337198d6fe32912f))
* **cua-driver:** prepare ClawHub skill release ([#2265](https://github.com/trycua/cua/issues/2265)) ([393c984](https://github.com/trycua/cua/commit/393c984190b22543ed0d83ce457e7bfec0653492))
* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341)) ([b8a0f32](https://github.com/trycua/cua/commit/b8a0f32a06c75225ba24ebb5ab14f6507fa90d15))
* **cua-driver:** ship semantic cursor themes ([e06e922](https://github.com/trycua/cua/commit/e06e9229ba65529f1594b11a6834cc2738117045))
* **cua-driver:** ship vector semantic cursor themes ([#2603](https://github.com/trycua/cua/issues/2603)) ([e2c52d5](https://github.com/trycua/cua/commit/e2c52d50ba331798a3da4871fdad3bbcdd399633))
* **cua-driver:** simplify permissions and add session identity ([#2616](https://github.com/trycua/cua/issues/2616)) ([8dac16e](https://github.com/trycua/cua/commit/8dac16e2c22a24687983e0e169a4dc2329e6cadb))
* **cua-driver:** standardize computer action telemetry ([#2318](https://github.com/trycua/cua/issues/2318)) ([9a29c8d](https://github.com/trycua/cua/commit/9a29c8dde15591713ddf8657050201894da3c2d8))
* **cua-driver:** support approved existing browser profiles ([#2261](https://github.com/trycua/cua/issues/2261)) ([8582630](https://github.com/trycua/cua/commit/8582630ca8f4d2d8ca0315d19dc5073b81a0fed9))
* **cua-driver:** support multiple direct runtimes per process ([#2575](https://github.com/trycua/cua/issues/2575)) ([5f786b0](https://github.com/trycua/cua/commit/5f786b058162bc69caacaa00c05f6fcca85d9d3d))
* **cua-driver:** track update funnel telemetry ([#2392](https://github.com/trycua/cua/issues/2392)) ([994308a](https://github.com/trycua/cua/commit/994308a96649109c4e6334acba7179acc8542155))
* **cua-driver:** unify tool telemetry ownership ([1435e84](https://github.com/trycua/cua/commit/1435e84a24f3d8de442091a37c229176faa2aba4))
* **cua-driver:** unify tool telemetry ownership ([ccdace9](https://github.com/trycua/cua/commit/ccdace9e7f6c01e08432e91e6eb8c882981c4d05))
* **cua-driver:** window-less desktop-scope click — macOS + Linux (unified interface) ([#2056](https://github.com/trycua/cua/issues/2056)) ([af2255c](https://github.com/trycua/cua/commit/af2255c78ba86dfdbdd49e1ce09e42b5101080b1))
* **driver:** add per-session capture scope ([#2329](https://github.com/trycua/cua/issues/2329)) ([db0ee87](https://github.com/trycua/cua/commit/db0ee870c200b5acc9a6542eabc84dc674b729c7))
* **driver:** add YAML and Rego permission policies ([#2235](https://github.com/trycua/cua/issues/2235)) ([afb25d7](https://github.com/trycua/cua/commit/afb25d740ba7f5b7bfd9970aee7b02919b10eb6e))
* **platform-linux:** native-Wayland parity (wlroots + portal + libei) ([#1966](https://github.com/trycua/cua/issues/1966)) ([519dfdf](https://github.com/trycua/cua/commit/519dfdfbd6c7cf8e10fa860b15201f84da73eb57))
* **release:** automate Driver and Lume attribution ([#2267](https://github.com/trycua/cua/issues/2267)) ([5019927](https://github.com/trycua/cua/commit/50199272cff60b11df0374dd26db365338be5744))
* **skills:** autodetect Hermes (NousResearch/hermes-agent) at ~/.hermes/skills ([#1963](https://github.com/trycua/cua/issues/1963)) ([9d90c81](https://github.com/trycua/cua/commit/9d90c815d31827f64a09d94c1d32d22c327f2770))
* **test-harness:** Linux GTK3 harness app + test (parity with macOS/Windows) ([#2050](https://github.com/trycua/cua/issues/2050)) ([aaa3f4c](https://github.com/trycua/cua/commit/aaa3f4c4ad0b88fda5f51763a200bb32863a96cd))


### Bug Fixes

* **cua-driver-rs:** feature-gate portal+libei so bullseye CD publishes v0.6.1 ([#1967](https://github.com/trycua/cua/issues/1967)) ([5061c11](https://github.com/trycua/cua/commit/5061c11b3aa51f596f63838f87a2dca1991aa545))
* **cua-driver/install.ps1:** resolve a valid temp dir when $env:TEMP is a missing 8.3 short path ([#1911](https://github.com/trycua/cua/issues/1911)) ([#1990](https://github.com/trycua/cua/issues/1990)) ([8395169](https://github.com/trycua/cua/commit/8395169dbe1c19a17ab4320a8e91066b1c9eefbf))
* **cua-driver/linux:** auto-discover XAUTHORITY for SSH-driven Wayland+Xwayland ([#1926](https://github.com/trycua/cua/issues/1926)) ([#1999](https://github.com/trycua/cua/issues/1999)) ([9091a30](https://github.com/trycua/cua/commit/9091a30fb4663dcadb65cdf800de774325c2deff))
* **cua-driver/linux:** bound X11 overlay render work ([#2331](https://github.com/trycua/cua/issues/2331)) ([428d7aa](https://github.com/trycua/cua/commit/428d7aa5364401c842a5e1428ba3b9ad7009e051))
* **cua-driver/linux:** fail loudly when X11 input can't be delivered on pure Wayland ([#1921](https://github.com/trycua/cua/issues/1921)) ([#1994](https://github.com/trycua/cua/issues/1994)) ([7c1d06b](https://github.com/trycua/cua/commit/7c1d06bf19d224e4690b6a537e85361cce8c81bb))
* **cua-driver/linux:** get_desktop_state screen size on pure Wayland ([#2047](https://github.com/trycua/cua/issues/2047)) ([6b2b262](https://github.com/trycua/cua/commit/6b2b2629debb2b19c57d0c0b2abdaf216ea41d3e))
* **cua-driver/linux:** glide the agent cursor to the desktop-scope click point ([#2061](https://github.com/trycua/cua/issues/2061)) ([7468487](https://github.com/trycua/cua/commit/7468487d303469b4993b960c8a2cb66289034bed))
* **cua-driver/linux:** remap a spare keycode for keysyms absent from the X keymap ([#2048](https://github.com/trycua/cua/issues/2048)) ([a139668](https://github.com/trycua/cua/commit/a139668f1c87b12bc2ce9a2d2c600252331557d9))
* **cua-driver/linux:** report dead input backend on KDE/GNOME Wayland instead of silent no-op ([#1982](https://github.com/trycua/cua/issues/1982)) ([#1992](https://github.com/trycua/cua/issues/1992)) ([3aff226](https://github.com/trycua/cua/commit/3aff22663a6409ecca1a42353ebbac3455256237))
* **cua-driver/linux:** retry root-only AT-SPI tree on cold Qt6 launch ([#1927](https://github.com/trycua/cua/issues/1927)) ([#1998](https://github.com/trycua/cua/issues/1998)) ([b64af30](https://github.com/trycua/cua/commit/b64af303f2f844f20e5de1d10214d6123bf7f680))
* **cua-driver/linux:** return structured launch_app result ([#2091](https://github.com/trycua/cua/issues/2091)) ([c0c6790](https://github.com/trycua/cua/commit/c0c67908167f3d0532683b005ad26659dcab8a34))
* **cua-driver/macos:** set_config accepts {key,value} shape (parity with Win/Linux) ([#2059](https://github.com/trycua/cua/issues/2059)) ([3def0d7](https://github.com/trycua/cua/commit/3def0d79df66b8a9d0b4dfeb71201ddebe672a1d))
* **cua-driver/windows:** retry transient BuildUpdatedCache failures instead of returning elements=0 ([#1881](https://github.com/trycua/cua/issues/1881)) ([#1996](https://github.com/trycua/cua/issues/1996)) ([7518f37](https://github.com/trycua/cua/commit/7518f37a3222761e4a3e391f0516c9ae8fe7fa34))
* **cua-driver/windows:** route DoubleClick/RightClick on Chromium targets via SendInput ([#1984](https://github.com/trycua/cua/issues/1984)) ([#1995](https://github.com/trycua/cua/issues/1995)) ([26d9298](https://github.com/trycua/cua/commit/26d9298e03cc2d6df5d470e9a3707eb21adde398))
* **cua-driver:** accept letter keys on Windows ([#2551](https://github.com/trycua/cua/issues/2551)) ([506a4ed](https://github.com/trycua/cua/commit/506a4ede22dfe791b3630640bc0c24e0f03c6d96))
* **cua-driver:** activate Chromium checkboxes safely on Linux ([#2633](https://github.com/trycua/cua/issues/2633)) ([1f16242](https://github.com/trycua/cua/commit/1f16242750df58b6af4b7b44f4f3a317a66a4bb1))
* **cua-driver:** advertise delivery mode capability ([7f6657c](https://github.com/trycua/cua/commit/7f6657c440cedf1f20c6307c849a89487f867b41)), closes [#2425](https://github.com/trycua/cua/issues/2425)
* **cua-driver:** avoid leaking XInput masters without uinput ([36e8d5a](https://github.com/trycua/cua/commit/36e8d5ad480096d0e5b576912674bdc3b9a89267))
* **cua-driver:** avoid opaque X11 cursor bloom ([#2328](https://github.com/trycua/cua/issues/2328)) ([e57fed5](https://github.com/trycua/cua/commit/e57fed576afba793db756d0b54e6ff6dde3387d6))
* **cua-driver:** avoid SIGCHLD handler in doctor ([31bbc07](https://github.com/trycua/cua/commit/31bbc074d75f59fe2c417cddc66cf4093bd8330b)), closes [#2348](https://github.com/trycua/cua/issues/2348)
* **cua-driver:** bind macOS state and actions to requested windows ([#2645](https://github.com/trycua/cua/issues/2645)) ([729c4da](https://github.com/trycua/cua/commit/729c4da91fcb5d5aac85348d09e642a7929c59c8))
* **cua-driver:** bound tool telemetry volume ([#2382](https://github.com/trycua/cua/issues/2382)) ([99d4d61](https://github.com/trycua/cua/commit/99d4d613abc35a67753d206fcf3371673fb75edd))
* **cua-driver:** carry full 32-bit snapshot generation in element tokens ([#2608](https://github.com/trycua/cua/issues/2608)) ([fcac692](https://github.com/trycua/cua/commit/fcac692b6d6531a32f3ebc5d7a9ba675d90ec86a))
* **cua-driver:** clarify foreground escalation guidance ([b4f6ffc](https://github.com/trycua/cua/commit/b4f6ffca79b4f746e28b30eaa041c534704ee372))
* **cua-driver:** classify health_report as read-only diagnostics ([#2408](https://github.com/trycua/cua/issues/2408)) ([16e7573](https://github.com/trycua/cua/commit/16e7573363adaf3cb6abcfbe8ebf04b7172ff160)), closes [#2399](https://github.com/trycua/cua/issues/2399)
* **cua-driver:** declare WinRects support for GNOME Shell 49 and 50 ([#2565](https://github.com/trycua/cua/issues/2565)) ([776dadd](https://github.com/trycua/cua/commit/776daddcbc7dba345343f0e0f709bd9c486ec087))
* **cua-driver:** deliver background browser keystrokes ([a7b7413](https://github.com/trycua/cua/commit/a7b741317c86ccd3b1241421e411137f87115eb8))
* **cua-driver:** deliver foreground pixel hotkeys through HID ([#2464](https://github.com/trycua/cua/issues/2464)) ([bc1a58d](https://github.com/trycua/cua/commit/bc1a58d7926fadbc79b7d2925895878b9ab81cda))
* **cua-driver:** deliver trusted background browser clicks ([00f33c4](https://github.com/trycua/cua/commit/00f33c4cfaff223a0ade812bb16f4cdb16419d87))
* **cua-driver:** diagnose stale local MCP registrations ([#2498](https://github.com/trycua/cua/issues/2498)) ([df10b6d](https://github.com/trycua/cua/commit/df10b6d3d4b194a714656d3d8758db5017234de0))
* **cua-driver:** distinguish autostart query failures ([#2345](https://github.com/trycua/cua/issues/2345)) ([0b798a5](https://github.com/trycua/cua/commit/0b798a59cbe7f9e628ae488cb8023b9b6f990bd6))
* **cua-driver:** distrust web keystroke AX read-back ([c81d69b](https://github.com/trycua/cua/commit/c81d69b87e33c2677b42a8d957f534fba379fd58))
* **cua-driver:** don't treat socket read-timeout (EAGAIN) as fatal in daemon proxy ([#1864](https://github.com/trycua/cua/issues/1864)) ([#1997](https://github.com/trycua/cua/issues/1997)) ([b651461](https://github.com/trycua/cua/commit/b651461eae8f44083e6d7e1257a4aa1031a05581))
* **cua-driver:** enforce authorization in embedded SDK ([#2542](https://github.com/trycua/cua/issues/2542)) ([06bda89](https://github.com/trycua/cua/commit/06bda89f078e882a64ecccb78700ff26dc4d210f))
* **cua-driver:** enforce scope for mouse button actions ([caa241b](https://github.com/trycua/cua/commit/caa241be5f801e2060d4f5823fde64f9e6dc2be4)), closes [#2368](https://github.com/trycua/cua/issues/2368)
* **cua-driver:** expose skill install failure details ([#2510](https://github.com/trycua/cua/issues/2510)) ([d614d8d](https://github.com/trycua/cua/commit/d614d8db689cd663f9e8d784ff03ac1bd388f506))
* **cua-driver:** fail closed for unsupported browser and Wayland routes ([2b06a78](https://github.com/trycua/cua/commit/2b06a782544e9fc742bf2e90c80d65d9b5c6f721))
* **cua-driver:** gate Windows overlay compositing to real pixel changes ([#2558](https://github.com/trycua/cua/issues/2558)) ([d7c1609](https://github.com/trycua/cua/commit/d7c1609345ab6383e2447a97fbac05a3338e5bc7))
* **cua-driver:** harden browser preview boundaries ([48e2e7b](https://github.com/trycua/cua/commit/48e2e7ba8457ca9262cbe17b304f87c7d1b577ac))
* **cua-driver:** harden browser preview boundaries ([667646a](https://github.com/trycua/cua/commit/667646af2d6391d419ff2169e7b4ea3e8d7c9ffc))
* **cua-driver:** harden browser scroll and endpoint proof ([1041383](https://github.com/trycua/cua/commit/10413835ba9036f9435caa167915b255eecef2ec))
* **cua-driver:** harden browser scroll and endpoint proof ([7510ed9](https://github.com/trycua/cua/commit/7510ed9c1f82f66c758e6d02e755350d705cacd5))
* **cua-driver:** harden exact-profile browser attachment across Chrome and Edge ([2b06a78](https://github.com/trycua/cua/commit/2b06a782544e9fc742bf2e90c80d65d9b5c6f721))
* **cua-driver:** harden existing browser attachment ([2ccb87c](https://github.com/trycua/cua/commit/2ccb87c8a9cbcaabc428243c8e7fbe809ac63709))
* **cua-driver:** harden GNOME 50 Shell capture ([#2226](https://github.com/trycua/cua/issues/2226)) ([d250e53](https://github.com/trycua/cua/commit/d250e532dbf5fdc28df0e5f1ec6b3209f846a838))
* **cua-driver:** harden Linux AT-SPI and input delivery ([3c63d71](https://github.com/trycua/cua/commit/3c63d712a5eab52103112cfab86fc2ca842442a3))
* **cua-driver:** harden Linux AT-SPI and input delivery ([ea599fd](https://github.com/trycua/cua/commit/ea599fdfd76f49f94a3fc4ddb5486dca68a3fbc4))
* **cua-driver:** harden macOS browser profile setup ([e6643f2](https://github.com/trycua/cua/commit/e6643f21e56e7fcee90c503f8fd39c7659d83b43))
* **cua-driver:** harden macOS input and capture delivery ([4e1aab4](https://github.com/trycua/cua/commit/4e1aab42ca4dba143aed20da72878a0ec4290739))
* **cua-driver:** harden macOS input and capture delivery ([ec908f7](https://github.com/trycua/cua/commit/ec908f74cb9f89cb6dbb7af14c18ff71105bd75a))
* **cua-driver:** harden telemetry release paths ([#2214](https://github.com/trycua/cua/issues/2214)) ([06fcc0f](https://github.com/trycua/cua/commit/06fcc0fb3bdf5d111069525d146dcb5aa2050e1b))
* **cua-driver:** harden Windows delivery and GUI validation ([5007f85](https://github.com/trycua/cua/commit/5007f85a37b4e29fea1d34c3fcc4590dd21f1e99))
* **cua-driver:** harden Windows delivery and GUI validation ([3c5c4c1](https://github.com/trycua/cua/commit/3c5c4c13552e349653e4d62e1f03e172e5cd5faa))
* **cua-driver:** honor Cargo target dir in local installs ([#2486](https://github.com/trycua/cua/issues/2486)) ([7cb6810](https://github.com/trycua/cua/commit/7cb6810b429ec9723c9e2050d6670340eeda4265))
* **cua-driver:** isolate Wayland portal connections ([#2552](https://github.com/trycua/cua/issues/2552)) ([074ec16](https://github.com/trycua/cua/commit/074ec16647809619aed56454967225b67c945ee5))
* **cua-driver:** keep press_key modifiers on native Wayland ([#2573](https://github.com/trycua/cua/issues/2573)) ([669c917](https://github.com/trycua/cua/commit/669c9177895c4998aa2d393f01e262d506384eb0))
* **cua-driver:** Linux list_windows emits `bounds` for cross-platform parity ([#2017](https://github.com/trycua/cua/issues/2017)) ([#2018](https://github.com/trycua/cua/issues/2018)) ([9e84c1d](https://github.com/trycua/cua/commit/9e84c1d8826dc1bfba166456b34f86a9d6212e0d))
* **cua-driver:** Linux X11 and Wayland convergence ([#2182](https://github.com/trycua/cua/issues/2182)) ([51a8e25](https://github.com/trycua/cua/commit/51a8e2593fc88efd4f0678d9ab7bbba50c22c91b))
* **cua-driver:** macOS e2e convergence (conv2 3/7) ([#2183](https://github.com/trycua/cua/issues/2183)) ([1d0ff7c](https://github.com/trycua/cua/commit/1d0ff7cc3ef7cfcadbf86b8c4b96ab396e319317))
* **cua-driver:** make install-local TCC grants survive rebuilds and release↔local switches ([af75ca5](https://github.com/trycua/cua/commit/af75ca5e774e01efc0648a408726343e7133ccd1))
* **cua-driver:** make macOS permission consent explicit and durable ([#2407](https://github.com/trycua/cua/issues/2407)) ([7ac41be](https://github.com/trycua/cua/commit/7ac41be61e6b1c0034cd9d4de48ac2d5f7bf6b13))
* **cua-driver:** make macOS SDK load standalone ([#2477](https://github.com/trycua/cua/issues/2477)) ([457b17e](https://github.com/trycua/cua/commit/457b17eae8ea2d2ca1b45ad5b23fc342787769a7))
* **cua-driver:** model Linux dialog delivery ([31b4f85](https://github.com/trycua/cua/commit/31b4f85755da8d5f5e793745839e2b0639188b22))
* **cua-driver:** persist set_config to disk on Windows + Linux ([#2034](https://github.com/trycua/cua/issues/2034)) ([e08574c](https://github.com/trycua/cua/commit/e08574ca0b3c7ca5c0d9ae33e6ceeac9a45e0d20))
* **cua-driver:** pick the AT-SPI action by name, not by position ([#2574](https://github.com/trycua/cua/issues/2574)) ([ee77164](https://github.com/trycua/cua/commit/ee77164b285d9ba313bdd7da03b32449722d8ce4))
* **cua-driver:** prefer foreground delivery in refusal guidance ([#2623](https://github.com/trycua/cua/issues/2623)) ([6d97684](https://github.com/trycua/cua/commit/6d97684fc4beebbb4ea3d658fa49817f9b7cd0f8))
* **cua-driver:** preserve approved Windows CDP port proof ([2527e77](https://github.com/trycua/cua/commit/2527e771ca0ea5aaed0c3d62d52ff4b07322264a))
* **cua-driver:** preserve cross-platform browser posture ([6ea7806](https://github.com/trycua/cua/commit/6ea780690388c33498a8111bc618d7fd0bc8d196))
* **cua-driver:** preserve Windows autostart path quoting ([cf07c2c](https://github.com/trycua/cua/commit/cf07c2c9ef7b17808d4152d91bb6c5ee22b733ab))
* **cua-driver:** prevent silent ad-hoc local signing ([#2495](https://github.com/trycua/cua/issues/2495)) ([b680ff2](https://github.com/trycua/cua/commit/b680ff2db0c87407e188f6d8fdd28a6bc77228b0))
* **cua-driver:** recover invalid Windows bounds ([#2601](https://github.com/trycua/cua/issues/2601)) ([0026845](https://github.com/trycua/cua/commit/0026845a096fd5a960e6f97742fbaedb054998be))
* **cua-driver:** redial Chrome after native consent ([#2635](https://github.com/trycua/cua/issues/2635)) ([fdebc59](https://github.com/trycua/cua/commit/fdebc59791ae2a8c6bc25f29770dc21f3c5faea7))
* **cua-driver:** refuse background window-move drags on Windows ([#2559](https://github.com/trycua/cua/issues/2559)) ([68b8943](https://github.com/trycua/cua/commit/68b8943c5c703809f654aef1d2f81de4197657a5))
* **cua-driver:** refuse disabled macOS AX actions ([#2637](https://github.com/trycua/cua/issues/2637)) ([8f9c2df](https://github.com/trycua/cua/commit/8f9c2dfd422a14a29fa49870e26f790c0c0ef211))
* **cua-driver:** refuse dropped ConsoleHost typing on Windows ARM64 ([bd5a8d3](https://github.com/trycua/cua/commit/bd5a8d343cec5b9125b144e62d5348bc7814a425))
* **cua-driver:** register bundle in LaunchServices before tccutil reset ([#2376](https://github.com/trycua/cua/issues/2376)) ([767acf2](https://github.com/trycua/cua/commit/767acf25f25ea668ffab428e4f2e8985896de98e))
* **cua-driver:** reject browser-origin MCP HTTP requests ([#2378](https://github.com/trycua/cua/issues/2378)) ([fccac48](https://github.com/trycua/cua/commit/fccac4852e7d1141ab0b03202c9093d5e826d448))
* **cua-driver:** report unknown browser tab selection ([6611622](https://github.com/trycua/cua/commit/6611622d6c6ef355661c80b6ecae765694088153))
* **cua-driver:** request macOS permissions from running daemon ([#2414](https://github.com/trycua/cua/issues/2414)) ([231839d](https://github.com/trycua/cua/commit/231839de8161c85b63b86f8a54678a0cd9816335))
* **cua-driver:** reset TCC grant on release/local signing transition ([fa309eb](https://github.com/trycua/cua/commit/fa309eb5ed0aac1e63c9eaf80dcb2a7749b8b472))
* **cua-driver:** restore focus after bounded browser setup ([2b06a78](https://github.com/trycua/cua/commit/2b06a782544e9fc742bf2e90c80d65d9b5c6f721))
* **cua-driver:** restore macOS background pixel clicks ([1fbcacf](https://github.com/trycua/cua/commit/1fbcacf6f5ecdec31414a364c3c4450dfce82e70))
* **cua-driver:** restore v0.12.5 and v0.12.6 installs ([#2598](https://github.com/trycua/cua/issues/2598)) ([fc17069](https://github.com/trycua/cua/commit/fc17069b69786f81ae0ff50eefeabe328b06dc4b))
* **cua-driver:** retain country-only telemetry ([#2217](https://github.com/trycua/cua/issues/2217)) ([a705c2a](https://github.com/trycua/cua/commit/a705c2ac463392109063c171ca4586a29811103c))
* **cua-driver:** retry daemon socket writes on EAGAIN, write-side mirror of [#1997](https://github.com/trycua/cua/issues/1997) ([#2036](https://github.com/trycua/cua/issues/2036)) ([d524d97](https://github.com/trycua/cua/commit/d524d97b5a414a9df9d62767d627f24b17bb6353))
* **cua-driver:** retry transient Windows CDP discovery ([4f5abc2](https://github.com/trycua/cua/commit/4f5abc2a231e9a8565326d7f58e78477f09e2666))
* **cua-driver:** reuse self-signed local identity ([a27753e](https://github.com/trycua/cua/commit/a27753e457be1947728fcaa298f077bb306172e9))
* **cua-driver:** separate local and release identities ([#2404](https://github.com/trycua/cua/issues/2404)) ([a8d8142](https://github.com/trycua/cua/commit/a8d8142bc1dfef641cae80680e0aa9c9230fffb8))
* **cua-driver:** show active-tab browser cursors on macOS ([#2493](https://github.com/trycua/cua/issues/2493)) ([c7675a0](https://github.com/trycua/cua/commit/c7675a04d5d3b53a0809db8347b11a23ac81ce63))
* **cua-driver:** show Windows browser action cursors ([#2532](https://github.com/trycua/cua/issues/2532)) ([8271dd8](https://github.com/trycua/cua/commit/8271dd8abaafe49f774f5f6a9ba676b0596510d0))
* **cua-driver:** stabilize background browser tabs ([3348fc5](https://github.com/trycua/cua/commit/3348fc5778e5b0dfb9402d2c13048c301eda9f57))
* **cua-driver:** support Electron UniFFI buffers ([#2455](https://github.com/trycua/cua/issues/2455)) ([d3bf82a](https://github.com/trycua/cua/commit/d3bf82a2cbb27fd83cfa836c04f9f5678e3e3aad))
* **cua-driver:** support Windows ARM64 browser sessions ([9c5a78b](https://github.com/trycua/cua/commit/9c5a78b65b09bedceea1faafe09f1ac9488e588b))
* **cua-driver:** target page JavaScript exactly ([#2166](https://github.com/trycua/cua/issues/2166)) ([fb5bc19](https://github.com/trycua/cua/commit/fb5bc192a5311d0519447f1d301bdf0b0c93bbb0))
* **cua-driver:** trust native macOS sentinel focus ([#2481](https://github.com/trycua/cua/issues/2481)) ([6c2bbcc](https://github.com/trycua/cua/commit/6c2bbccfe5ad6690ef0961c492b34a3493354e84))
* **cua-driver:** verify desktop input effects and release modifiers ([#2492](https://github.com/trycua/cua/issues/2492)) ([5b68ef5](https://github.com/trycua/cua/commit/5b68ef596f9adf183121e864d19db5e0ee7ddd4d))
* **cua-driver:** verify set_value writes with an AXValue read-back ([#2621](https://github.com/trycua/cua/issues/2621)) ([e143b95](https://github.com/trycua/cua/commit/e143b9565069aa35c9ff21fec171858d8a264b06))
* **cua-driver:** wait for background tab input readiness ([a19e909](https://github.com/trycua/cua/commit/a19e909db8bfd236c8f71ab56edfd4c3fdc7c807))
* **cua-driver:** wait for generic type_text completion ([#2463](https://github.com/trycua/cua/issues/2463)) ([3858c7f](https://github.com/trycua/cua/commit/3858c7f2aafc3d03f5a6342d255a625d79b184fb))
* **cua-driver:** Windows e2e convergence ([#2180](https://github.com/trycua/cua/issues/2180)) ([88f88e1](https://github.com/trycua/cua/commit/88f88e1b5a31810699781e49f2dd9b9733fbaa82))
* **driver:** remove current autostart services on uninstall ([#2358](https://github.com/trycua/cua/issues/2358)) ([4a492f3](https://github.com/trycua/cua/commit/4a492f36704eaf063a82eadd0469d08684c7a1f5))
* **driver:** require daemon-backed calls and stabilize E2E ([#2338](https://github.com/trycua/cua/issues/2338)) ([9251bb0](https://github.com/trycua/cua/commit/9251bb006018d829b452e7109e322eab3f8c9633))
* **release:** synchronize driver skill versions ([fa5092b](https://github.com/trycua/cua/commit/fa5092bac1ed8e41703ac379814965f6773e8cab))
* **release:** synchronize driver skill versions ([512e45a](https://github.com/trycua/cua/commit/512e45a03fc0b4390f3d29b6443db5821157f7a0))

## [0.13.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.6...cua-driver-rs-v0.13.0) (2026-07-28)

> **Retracted:** 0.13.0 was published unintentionally and is not supported.
> Do not use this release. Distribution artifacts are being withdrawn; package
> registries permanently reserve published version numbers, so 0.13.0 will not
> be republished.


### ⚠ BREAKING CHANGES

* **cua-driver:** simplify permissions and add session identity ([#2616](https://github.com/trycua/cua/issues/2616))
* **cua-driver:** ship semantic cursor themes

### Features

* **cua-driver:** add local consent UI and complete Wayland certification ([#2597](https://github.com/trycua/cua/issues/2597)) ([e9852cb](https://github.com/trycua/cua/commit/e9852cb68494e9f521c7734206f638773dbc3299))
* **cua-driver:** browser_type can replace a field's content, not only append ([#2624](https://github.com/trycua/cua/issues/2624)) ([eb0b5a0](https://github.com/trycua/cua/commit/eb0b5a0c72cd1203e1dcaab768cdf37c33868302))
* **cua-driver:** centralize protected resource grants per runtime ([#2577](https://github.com/trycua/cua/issues/2577)) ([dfb3781](https://github.com/trycua/cua/commit/dfb3781460f04f9b523871b2f478233a59e5d364))
* **cua-driver:** enforce consequential and privileged actions ([#2585](https://github.com/trycua/cua/issues/2585)) ([df368a7](https://github.com/trycua/cua/commit/df368a7698a07ec9ba7e10952d483457c22841ff))
* **cua-driver:** enforce desktop input grants ([#2580](https://github.com/trycua/cua/issues/2580)) ([2b5e938](https://github.com/trycua/cua/commit/2b5e938d7014ccdced5abe6d1c4d493063f67b56))
* **cua-driver:** enforce exact file transfer grants ([#2581](https://github.com/trycua/cua/issues/2581)) ([3f74894](https://github.com/trycua/cua/commit/3f748946ecc176d2d7a71add246c5647acca2be3))
* **cua-driver:** enforce private observation grants ([#2579](https://github.com/trycua/cua/issues/2579)) ([6052381](https://github.com/trycua/cua/commit/6052381e97446cca3338221eb888c49e57356890))
* **cua-driver:** expose trusted protected-consent hosts ([#2578](https://github.com/trycua/cua/issues/2578)) ([fd4069e](https://github.com/trycua/cua/commit/fd4069ec6de49e9fa499e3057e874b9dae1d9665))
* **cua-driver:** implement SDK-owned runtime RFC ([#2561](https://github.com/trycua/cua/issues/2561)) ([1760f25](https://github.com/trycua/cua/commit/1760f253d3c4d76618a8c97a04f2c100ffc491ac))
* **cua-driver:** make permission adapter inventory mode-aware ([#2576](https://github.com/trycua/cua/issues/2576)) ([83a2f6f](https://github.com/trycua/cua/commit/83a2f6fad7b2f3349be40433e343cd8f934495eb))
* **cua-driver:** make SDK-owned runtime the default ([#2545](https://github.com/trycua/cua/issues/2545)) ([a62e821](https://github.com/trycua/cua/commit/a62e8219255f15f6ed747c03337198d6fe32912f))
* **cua-driver:** ship semantic cursor themes ([e06e922](https://github.com/trycua/cua/commit/e06e9229ba65529f1594b11a6834cc2738117045))
* **cua-driver:** ship vector semantic cursor themes ([#2603](https://github.com/trycua/cua/issues/2603)) ([e2c52d5](https://github.com/trycua/cua/commit/e2c52d50ba331798a3da4871fdad3bbcdd399633))
* **cua-driver:** simplify permissions and add session identity ([#2616](https://github.com/trycua/cua/issues/2616)) ([8dac16e](https://github.com/trycua/cua/commit/8dac16e2c22a24687983e0e169a4dc2329e6cadb))
* **cua-driver:** support multiple direct runtimes per process ([#2575](https://github.com/trycua/cua/issues/2575)) ([5f786b0](https://github.com/trycua/cua/commit/5f786b058162bc69caacaa00c05f6fcca85d9d3d))


### Bug Fixes

* **cua-driver:** accept letter keys on Windows ([#2551](https://github.com/trycua/cua/issues/2551)) ([506a4ed](https://github.com/trycua/cua/commit/506a4ede22dfe791b3630640bc0c24e0f03c6d96))
* **cua-driver:** activate Chromium checkboxes safely on Linux ([#2633](https://github.com/trycua/cua/issues/2633)) ([1f16242](https://github.com/trycua/cua/commit/1f16242750df58b6af4b7b44f4f3a317a66a4bb1))
* **cua-driver:** avoid leaking XInput masters without uinput ([36e8d5a](https://github.com/trycua/cua/commit/36e8d5ad480096d0e5b576912674bdc3b9a89267))
* **cua-driver:** carry full 32-bit snapshot generation in element tokens ([#2608](https://github.com/trycua/cua/issues/2608)) ([fcac692](https://github.com/trycua/cua/commit/fcac692b6d6531a32f3ebc5d7a9ba675d90ec86a))
* **cua-driver:** clarify foreground escalation guidance ([b4f6ffc](https://github.com/trycua/cua/commit/b4f6ffca79b4f746e28b30eaa041c534704ee372))
* **cua-driver:** declare WinRects support for GNOME Shell 49 and 50 ([#2565](https://github.com/trycua/cua/issues/2565)) ([776dadd](https://github.com/trycua/cua/commit/776daddcbc7dba345343f0e0f709bd9c486ec087))
* **cua-driver:** keep press_key modifiers on native Wayland ([#2573](https://github.com/trycua/cua/issues/2573)) ([669c917](https://github.com/trycua/cua/commit/669c9177895c4998aa2d393f01e262d506384eb0))
* **cua-driver:** pick the AT-SPI action by name, not by position ([#2574](https://github.com/trycua/cua/issues/2574)) ([ee77164](https://github.com/trycua/cua/commit/ee77164b285d9ba313bdd7da03b32449722d8ce4))
* **cua-driver:** prefer foreground delivery in refusal guidance ([#2623](https://github.com/trycua/cua/issues/2623)) ([6d97684](https://github.com/trycua/cua/commit/6d97684fc4beebbb4ea3d658fa49817f9b7cd0f8))
* **cua-driver:** preserve Windows autostart path quoting ([cf07c2c](https://github.com/trycua/cua/commit/cf07c2c9ef7b17808d4152d91bb6c5ee22b733ab))
* **cua-driver:** recover invalid Windows bounds ([#2601](https://github.com/trycua/cua/issues/2601)) ([0026845](https://github.com/trycua/cua/commit/0026845a096fd5a960e6f97742fbaedb054998be))
* **cua-driver:** redial Chrome after native consent ([#2635](https://github.com/trycua/cua/issues/2635)) ([fdebc59](https://github.com/trycua/cua/commit/fdebc59791ae2a8c6bc25f29770dc21f3c5faea7))
* **cua-driver:** refuse disabled macOS AX actions ([#2637](https://github.com/trycua/cua/issues/2637)) ([8f9c2df](https://github.com/trycua/cua/commit/8f9c2dfd422a14a29fa49870e26f790c0c0ef211))
* **cua-driver:** reject browser-origin MCP HTTP requests ([#2378](https://github.com/trycua/cua/issues/2378)) ([fccac48](https://github.com/trycua/cua/commit/fccac4852e7d1141ab0b03202c9093d5e826d448))
* **cua-driver:** restore macOS background pixel clicks ([1fbcacf](https://github.com/trycua/cua/commit/1fbcacf6f5ecdec31414a364c3c4450dfce82e70))
* **cua-driver:** restore v0.12.5 and v0.12.6 installs ([#2598](https://github.com/trycua/cua/issues/2598)) ([fc17069](https://github.com/trycua/cua/commit/fc17069b69786f81ae0ff50eefeabe328b06dc4b))
* **cua-driver:** verify set_value writes with an AXValue read-back ([#2621](https://github.com/trycua/cua/issues/2621)) ([e143b95](https://github.com/trycua/cua/commit/e143b9565069aa35c9ff21fec171858d8a264b06))

## [0.12.6](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.5...cua-driver-rs-v0.12.6) (2026-07-24)


### Bug Fixes

* **cua-driver:** isolate Wayland portal connections ([#2552](https://github.com/trycua/cua/issues/2552)) ([074ec16](https://github.com/trycua/cua/commit/074ec16647809619aed56454967225b67c945ee5))

## [0.12.5](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.4...cua-driver-rs-v0.12.5) (2026-07-24)


### Bug Fixes

* **cua-driver:** enforce authorization in embedded SDK ([#2542](https://github.com/trycua/cua/issues/2542)) ([06bda89](https://github.com/trycua/cua/commit/06bda89f078e882a64ecccb78700ff26dc4d210f))
* **cua-driver:** refuse dropped ConsoleHost typing on Windows ARM64 ([bd5a8d3](https://github.com/trycua/cua/commit/bd5a8d343cec5b9125b144e62d5348bc7814a425))

## [0.12.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.3...cua-driver-rs-v0.12.4) (2026-07-24)


### Bug Fixes

* **cua-driver:** deliver foreground pixel hotkeys through HID ([#2464](https://github.com/trycua/cua/issues/2464)) ([bc1a58d](https://github.com/trycua/cua/commit/bc1a58d7926fadbc79b7d2925895878b9ab81cda))
* **cua-driver:** expose skill install failure details ([#2510](https://github.com/trycua/cua/issues/2510)) ([d614d8d](https://github.com/trycua/cua/commit/d614d8db689cd663f9e8d784ff03ac1bd388f506))
* **cua-driver:** honor Cargo target dir in local installs ([#2486](https://github.com/trycua/cua/issues/2486)) ([7cb6810](https://github.com/trycua/cua/commit/7cb6810b429ec9723c9e2050d6670340eeda4265))
* **cua-driver:** show Windows browser action cursors ([#2532](https://github.com/trycua/cua/issues/2532)) ([8271dd8](https://github.com/trycua/cua/commit/8271dd8abaafe49f774f5f6a9ba676b0596510d0))
* **cua-driver:** support Windows ARM64 browser sessions ([9c5a78b](https://github.com/trycua/cua/commit/9c5a78b65b09bedceea1faafe09f1ac9488e588b))
* **cua-driver:** wait for generic type_text completion ([#2463](https://github.com/trycua/cua/issues/2463)) ([3858c7f](https://github.com/trycua/cua/commit/3858c7f2aafc3d03f5a6342d255a625d79b184fb))

## [0.12.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.2...cua-driver-rs-v0.12.3) (2026-07-23)


### Bug Fixes

* **cua-driver:** diagnose stale local MCP registrations ([#2498](https://github.com/trycua/cua/issues/2498)) ([df10b6d](https://github.com/trycua/cua/commit/df10b6d3d4b194a714656d3d8758db5017234de0))
* **cua-driver:** prevent silent ad-hoc local signing ([#2495](https://github.com/trycua/cua/issues/2495)) ([b680ff2](https://github.com/trycua/cua/commit/b680ff2db0c87407e188f6d8fdd28a6bc77228b0))
* **cua-driver:** show active-tab browser cursors on macOS ([#2493](https://github.com/trycua/cua/issues/2493)) ([c7675a0](https://github.com/trycua/cua/commit/c7675a04d5d3b53a0809db8347b11a23ac81ce63))
* **cua-driver:** verify desktop input effects and release modifiers ([#2492](https://github.com/trycua/cua/issues/2492)) ([5b68ef5](https://github.com/trycua/cua/commit/5b68ef596f9adf183121e864d19db5e0ee7ddd4d))

## [0.12.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.1...cua-driver-rs-v0.12.2) (2026-07-23)


### Bug Fixes

* **cua-driver:** tolerate fileno-less Python stdio ([#2482](https://github.com/trycua/cua/issues/2482)) ([d041f1a](https://github.com/trycua/cua/commit/d041f1a580d413bbe5da6645fd58b9088b2cdd4a))
* **cua-driver:** trust native macOS sentinel focus ([#2481](https://github.com/trycua/cua/issues/2481)) ([6c2bbcc](https://github.com/trycua/cua/commit/6c2bbccfe5ad6690ef0961c492b34a3493354e84))

## [0.12.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.0...cua-driver-rs-v0.12.1) (2026-07-23)


### Bug Fixes

* **cua-driver:** make macOS SDK load standalone ([#2477](https://github.com/trycua/cua/issues/2477)) ([457b17e](https://github.com/trycua/cua/commit/457b17eae8ea2d2ca1b45ad5b23fc342787769a7))

## [0.12.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.11.0...cua-driver-rs-v0.12.0) (2026-07-22)


### Features

* **cua-driver:** add client and modality telemetry ([#2441](https://github.com/trycua/cua/issues/2441)) ([5cd0000](https://github.com/trycua/cua/commit/5cd0000e2e018a835303a8f8bdd82a88ea483a6e))
* **cua-driver:** add true in-process SDK runtime ([#2461](https://github.com/trycua/cua/issues/2461)) ([617508a](https://github.com/trycua/cua/commit/617508a7ae123f277b203d31fca29933927a4636))
* **cua-driver:** add versioned native ABI ([b172afc](https://github.com/trycua/cua/commit/b172afc75f39832a4bdfaa9040d6d4f556449b49))
* **cua-driver:** expose macOS control state in window elements ([db281a3](https://github.com/trycua/cua/commit/db281a3b0d9bbabaf74f3c6847adf6abbb66995a))


### Bug Fixes

* **cua-driver:** advertise delivery mode capability ([7f6657c](https://github.com/trycua/cua/commit/7f6657c440cedf1f20c6307c849a89487f867b41)), closes [#2425](https://github.com/trycua/cua/issues/2425)
* **cua-driver:** avoid SIGCHLD handler in doctor ([31bbc07](https://github.com/trycua/cua/commit/31bbc074d75f59fe2c417cddc66cf4093bd8330b)), closes [#2348](https://github.com/trycua/cua/issues/2348)
* **cua-driver:** enforce scope for mouse button actions ([caa241b](https://github.com/trycua/cua/commit/caa241be5f801e2060d4f5823fde64f9e6dc2be4)), closes [#2368](https://github.com/trycua/cua/issues/2368)
* **cua-driver:** support Electron UniFFI buffers ([#2455](https://github.com/trycua/cua/issues/2455)) ([d3bf82a](https://github.com/trycua/cua/commit/d3bf82a2cbb27fd83cfa836c04f9f5678e3e3aad))

## [0.11.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.10.0...cua-driver-rs-v0.11.0) (2026-07-22)


### ⚠ BREAKING CHANGES

* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341))

### Features

* **cua-driver:** add persistent macOS interactive input sessions ([2dad3e5](https://github.com/trycua/cua/commit/2dad3e519e17b27eaa793151b8671957f578072c))
* **cua-driver:** add Rust-owned embedded host for SDK and MCP ([#2427](https://github.com/trycua/cua/issues/2427)) ([5016dc1](https://github.com/trycua/cua/commit/5016dc16bfe54c165e6678104ec521a6d85f76db))
* **cua-driver:** capture inactive tabs and retain modal controls ([#2426](https://github.com/trycua/cua/issues/2426)) ([c4d7ddc](https://github.com/trycua/cua/commit/c4d7ddc5bc7c00faf3e9102bee664ea47b2f5fac))
* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341)) ([b8a0f32](https://github.com/trycua/cua/commit/b8a0f32a06c75225ba24ebb5ab14f6507fa90d15))
* **cua-driver:** track update funnel telemetry ([#2392](https://github.com/trycua/cua/issues/2392)) ([994308a](https://github.com/trycua/cua/commit/994308a96649109c4e6334acba7179acc8542155))


### Bug Fixes

* **cua-driver:** classify health_report as read-only diagnostics ([#2408](https://github.com/trycua/cua/issues/2408)) ([16e7573](https://github.com/trycua/cua/commit/16e7573363adaf3cb6abcfbe8ebf04b7172ff160)), closes [#2399](https://github.com/trycua/cua/issues/2399)
* **cua-driver:** make macOS permission consent explicit and durable ([#2407](https://github.com/trycua/cua/issues/2407)) ([7ac41be](https://github.com/trycua/cua/commit/7ac41be61e6b1c0034cd9d4de48ac2d5f7bf6b13))
* **cua-driver:** request macOS permissions from running daemon ([#2414](https://github.com/trycua/cua/issues/2414)) ([231839d](https://github.com/trycua/cua/commit/231839de8161c85b63b86f8a54678a0cd9816335))
* **cua-driver:** separate local and release identities ([#2404](https://github.com/trycua/cua/issues/2404)) ([a8d8142](https://github.com/trycua/cua/commit/a8d8142bc1dfef641cae80680e0aa9c9230fffb8))

## [0.10.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.9.1...cua-driver-rs-v0.10.0) (2026-07-20)


### Features

* **cua-driver:** add protected permission modes and consent grants ([#2383](https://github.com/trycua/cua/issues/2383)) ([c75e606](https://github.com/trycua/cua/commit/c75e60636c11e21ef44f1ebbe1c1350339bae295))


### Bug Fixes

* **cua-driver:** bound tool telemetry volume ([#2382](https://github.com/trycua/cua/issues/2382)) ([99d4d61](https://github.com/trycua/cua/commit/99d4d613abc35a67753d206fcf3371673fb75edd))
* **cua-driver:** register bundle in LaunchServices before tccutil reset ([#2376](https://github.com/trycua/cua/issues/2376)) ([767acf2](https://github.com/trycua/cua/commit/767acf25f25ea668ffab428e4f2e8985896de98e))

## [0.9.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.9.0...cua-driver-rs-v0.9.1) (2026-07-20)


### Bug Fixes

* **cua-driver:** fail closed for unsupported browser and Wayland routes ([#2367](https://github.com/trycua/cua/pull/2367))
* **cua-driver:** harden exact-profile browser attachment across Chrome and Edge ([#2367](https://github.com/trycua/cua/pull/2367))
* **cua-driver:** restore focus after bounded browser setup ([#2367](https://github.com/trycua/cua/pull/2367))

## [0.9.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.3...cua-driver-rs-v0.9.0) (2026-07-19)


### Features

* **cua-driver-rs:** add Linux arm64 (aarch64) prebuilt support ([#1948](https://github.com/trycua/cua/issues/1948)) ([79884bd](https://github.com/trycua/cua/commit/79884bde1ea483affc907f2b6b264382023dbf17))
* **cua-driver-rs:** become own responsible process for true TCC status ([#1956](https://github.com/trycua/cua/issues/1956)) ([d61ce01](https://github.com/trycua/cua/commit/d61ce0197ee4f396220e4094da05745825a7b440))
* **cua-driver-rs:** caller-declared session identity + Streamable-HTTP transport for multi-agent parallelism ([#1798](https://github.com/trycua/cua/issues/1798)) ([e1e8c98](https://github.com/trycua/cua/commit/e1e8c98208f64c0e12d8683647091b1f264a7613))
* **cua-driver-rs:** check-update CLI verb + check_for_update MCP tool ([#1734](https://github.com/trycua/cua/issues/1734)) ([7d893ce](https://github.com/trycua/cua/commit/7d893ce827fe0d3730c696cef7eae70d98602bed))
* **cua-driver-rs:** daemon session identity — own + clean up session-scoped recording & config ([#1776](https://github.com/trycua/cua/issues/1776)) ([748e74a](https://github.com/trycua/cua/commit/748e74a84ef0581c98ff95e4a40b98ab28b07dae))
* **cua-driver-rs:** wire up --claude-code-computer-use-compat + make it default for claude ([#1678](https://github.com/trycua/cua/issues/1678)) ([ec07873](https://github.com/trycua/cua/commit/ec0787363da03595381b62c6a4781b1a0fcd955c))
* **cua-driver/macos:** implement page click_element + fix Chromium window targeting ([#2082](https://github.com/trycua/cua/issues/2082)) ([73fe822](https://github.com/trycua/cua/commit/73fe8227982c0b03795669e4cdf62384f0d54a34))
* **cua-driver:** add aggregate agent session telemetry ([aa41707](https://github.com/trycua/cua/commit/aa417076c7d31df9d78dc72e982b48b5506e1695))
* **cua-driver:** add aggregate agent session telemetry ([3a59246](https://github.com/trycua/cua/commit/3a5924640738e9881efc11a80b6e26ed6ef5ed0a))
* **cua-driver:** add bounded feature telemetry ([4972560](https://github.com/trycua/cua/commit/4972560226ea3ff81c5d07ca3e867fbaa5238f69))
* **cua-driver:** add bounded feature telemetry ([ebfb4eb](https://github.com/trycua/cua/commit/ebfb4eb7fa48e028d4a64ec94ddda2f466d8313d))
* **cua-driver:** add browser dialogs and uploads ([21ca689](https://github.com/trycua/cua/commit/21ca68928e546103a59ad851699a50e543ff601b))
* **cua-driver:** add browser telemetry contract ([fa13890](https://github.com/trycua/cua/commit/fa138903f779b33b4c8512c7d64bb960023161fe))
* **cua-driver:** add browser telemetry contract ([#2310](https://github.com/trycua/cua/issues/2310)) ([763a6ea](https://github.com/trycua/cua/commit/763a6ea21b86ad5f8a70ab2581edee1dd7370efa))
* **cua-driver:** add capability-aware browser tools ([#2257](https://github.com/trycua/cua/issues/2257)) ([0835daa](https://github.com/trycua/cua/commit/0835daa6d415c857c1d3ebefe2c29453a0bed923))
* **cua-driver:** add semantic browser snapshots ([1a9fbb4](https://github.com/trycua/cua/commit/1a9fbb480605f72c449d2c741c19ae32ed5659d8))
* **cua-driver:** add semantic browser snapshots ([#2301](https://github.com/trycua/cua/issues/2301)) ([e83d2d3](https://github.com/trycua/cua/commit/e83d2d3d6142fee78edca6352790db4336abacbb))
* **cua-driver:** add stable health_report MCP tool for end-to-end driver diagnostics ([be761fa](https://github.com/trycua/cua/commit/be761fac796d3f266d56ed7ce89c5a5ff6a89eac))
* **cua-driver:** complete browser mutations ([b4007a8](https://github.com/trycua/cua/commit/b4007a8442d3f3238226b5d5d11feda8da13c0af))
* **cua-driver:** complete telemetry lifecycle coverage ([f06b7cf](https://github.com/trycua/cua/commit/f06b7cf26f44721edd968f1dd072eb395212dd10))
* **cua-driver:** complete telemetry lifecycle coverage ([7ac5836](https://github.com/trycua/cua/commit/7ac5836714e99f945e13e840d400f0a637edf56d))
* **cua-driver:** complete the browser action surface ([#2323](https://github.com/trycua/cua/issues/2323)) ([01a9505](https://github.com/trycua/cua/commit/01a9505aeac5d2a3afc57a7b1ce98523b58929ae))
* **cua-driver:** desktop-scope Phase 1 — capture_scope config, get_desktop_state, Windows screen-absolute actions ([#1968](https://github.com/trycua/cua/issues/1968)) ([#2019](https://github.com/trycua/cua/issues/2019)) ([fc27185](https://github.com/trycua/cua/commit/fc271854149771adbb9f2e71ad64e58de5087de7))
* **cua-driver:** embedded mode — inherit the host app's TCC grants, never prompt ([#2102](https://github.com/trycua/cua/issues/2102)) ([b654f27](https://github.com/trycua/cua/commit/b654f27d609ecbac22ea63a000c868c90c0ee44d))
* **cua-driver:** fall back to key events when type-target is a terminal ([687d908](https://github.com/trycua/cua/commit/687d908f5efe19ddfd0915f6e654ffb9118bea01))
* **cua-driver:** Hermes-decoupling MCP surface + install UX ([e85cd87](https://github.com/trycua/cua/commit/e85cd878ae859e3c66f53ce83dd0060fdcb0528e))
* **cua-driver:** prepare ClawHub skill release ([#2265](https://github.com/trycua/cua/issues/2265)) ([393c984](https://github.com/trycua/cua/commit/393c984190b22543ed0d83ce457e7bfec0653492))
* **cua-driver:** standardize computer action telemetry ([#2318](https://github.com/trycua/cua/issues/2318)) ([9a29c8d](https://github.com/trycua/cua/commit/9a29c8dde15591713ddf8657050201894da3c2d8))
* **cua-driver:** support approved existing browser profiles ([#2261](https://github.com/trycua/cua/issues/2261)) ([8582630](https://github.com/trycua/cua/commit/8582630ca8f4d2d8ca0315d19dc5073b81a0fed9))
* **cua-driver:** unify tool telemetry ownership ([1435e84](https://github.com/trycua/cua/commit/1435e84a24f3d8de442091a37c229176faa2aba4))
* **cua-driver:** unify tool telemetry ownership ([ccdace9](https://github.com/trycua/cua/commit/ccdace9e7f6c01e08432e91e6eb8c882981c4d05))
* **cua-driver:** window-less desktop-scope click — macOS + Linux (unified interface) ([#2056](https://github.com/trycua/cua/issues/2056)) ([af2255c](https://github.com/trycua/cua/commit/af2255c78ba86dfdbdd49e1ce09e42b5101080b1))
* **cursor-overlay:** retina-aware rendering + arrow/teardrop selection ([c172e2a](https://github.com/trycua/cua/commit/c172e2acfa038ae437ca2d67795bf90c5c2c02e7))
* **driver:** add per-session capture scope ([#2329](https://github.com/trycua/cua/issues/2329)) ([db0ee87](https://github.com/trycua/cua/commit/db0ee870c200b5acc9a6542eabc84dc674b729c7))
* **driver:** add YAML and Rego permission policies ([#2235](https://github.com/trycua/cua/issues/2235)) ([afb25d7](https://github.com/trycua/cua/commit/afb25d740ba7f5b7bfd9970aee7b02919b10eb6e))
* **platform-linux:** native-Wayland parity (wlroots + portal + libei) ([#1966](https://github.com/trycua/cua/issues/1966)) ([519dfdf](https://github.com/trycua/cua/commit/519dfdfbd6c7cf8e10fa860b15201f84da73eb57))
* **release:** automate Driver and Lume attribution ([#2267](https://github.com/trycua/cua/issues/2267)) ([5019927](https://github.com/trycua/cua/commit/50199272cff60b11df0374dd26db365338be5744))
* **skills:** autodetect Hermes (NousResearch/hermes-agent) at ~/.hermes/skills ([#1963](https://github.com/trycua/cua/issues/1963)) ([9d90c81](https://github.com/trycua/cua/commit/9d90c815d31827f64a09d94c1d32d22c327f2770))
* **test-harness:** Linux GTK3 harness app + test (parity with macOS/Windows) ([#2050](https://github.com/trycua/cua/issues/2050)) ([aaa3f4c](https://github.com/trycua/cua/commit/aaa3f4c4ad0b88fda5f51763a200bb32863a96cd))
* **windows:** background input without z-raise + multi-cursor demos + overlay improvements ([#1809](https://github.com/trycua/cua/issues/1809)) ([53bb84c](https://github.com/trycua/cua/commit/53bb84cc71cba19f5a91c6cd99f80650a34c5c68))


### Bug Fixes

* **cua-driver-rs:** feature-gate portal+libei so bullseye CD publishes v0.6.1 ([#1967](https://github.com/trycua/cua/issues/1967)) ([5061c11](https://github.com/trycua/cua/commit/5061c11b3aa51f596f63838f87a2dca1991aa545))
* **cua-driver-rs:** make Cargo.toml the source of truth for release bumps ([#1907](https://github.com/trycua/cua/issues/1907)) ([dacde81](https://github.com/trycua/cua/commit/dacde8147e6c27ee15855e6060fe834c111ce616))
* **cua-driver-rs:** post-install + Skill hints route through mcp-config (not the broken `claude mcp add -- … --flag` line) ([#1681](https://github.com/trycua/cua/issues/1681)) ([ef7e2b6](https://github.com/trycua/cua/commit/ef7e2b605c4fa113bde25082812a2c10edb9de7a))
* **cua-driver-rs:** release installer unifies home on ~/.cua-driver + cleans up prior local install ([#1803](https://github.com/trycua/cua/issues/1803)) ([33a6893](https://github.com/trycua/cua/commit/33a6893634f47e809cf83eaed150f65304272948))
* **cua-driver-rs:** show the agent cursor by default ([#1955](https://github.com/trycua/cua/issues/1955)) ([1119035](https://github.com/trycua/cua/commit/11190356b3776f2003171872ef0f75c7f5d90d40))
* **cua-driver-rs:** wire/guide per-session cursors through the real mcp path + skills/docs ([#1787](https://github.com/trycua/cua/issues/1787)) ([18e2cbb](https://github.com/trycua/cua/commit/18e2cbb1c0c5ce221a2239abbdeb913a07777af9))
* **cua-driver/install.ps1:** resolve a valid temp dir when $env:TEMP is a missing 8.3 short path ([#1911](https://github.com/trycua/cua/issues/1911)) ([#1990](https://github.com/trycua/cua/issues/1990)) ([8395169](https://github.com/trycua/cua/commit/8395169dbe1c19a17ab4320a8e91066b1c9eefbf))
* **cua-driver/linux:** auto-discover XAUTHORITY for SSH-driven Wayland+Xwayland ([#1926](https://github.com/trycua/cua/issues/1926)) ([#1999](https://github.com/trycua/cua/issues/1999)) ([9091a30](https://github.com/trycua/cua/commit/9091a30fb4663dcadb65cdf800de774325c2deff))
* **cua-driver/linux:** bound X11 overlay render work ([#2331](https://github.com/trycua/cua/issues/2331)) ([428d7aa](https://github.com/trycua/cua/commit/428d7aa5364401c842a5e1428ba3b9ad7009e051))
* **cua-driver/linux:** fail loudly when X11 input can't be delivered on pure Wayland ([#1921](https://github.com/trycua/cua/issues/1921)) ([#1994](https://github.com/trycua/cua/issues/1994)) ([7c1d06b](https://github.com/trycua/cua/commit/7c1d06bf19d224e4690b6a537e85361cce8c81bb))
* **cua-driver/linux:** get_desktop_state screen size on pure Wayland ([#2047](https://github.com/trycua/cua/issues/2047)) ([6b2b262](https://github.com/trycua/cua/commit/6b2b2629debb2b19c57d0c0b2abdaf216ea41d3e))
* **cua-driver/linux:** glide the agent cursor to the desktop-scope click point ([#2061](https://github.com/trycua/cua/issues/2061)) ([7468487](https://github.com/trycua/cua/commit/7468487d303469b4993b960c8a2cb66289034bed))
* **cua-driver/linux:** remap a spare keycode for keysyms absent from the X keymap ([#2048](https://github.com/trycua/cua/issues/2048)) ([a139668](https://github.com/trycua/cua/commit/a139668f1c87b12bc2ce9a2d2c600252331557d9))
* **cua-driver/linux:** report dead input backend on KDE/GNOME Wayland instead of silent no-op ([#1982](https://github.com/trycua/cua/issues/1982)) ([#1992](https://github.com/trycua/cua/issues/1992)) ([3aff226](https://github.com/trycua/cua/commit/3aff22663a6409ecca1a42353ebbac3455256237))
* **cua-driver/linux:** retry root-only AT-SPI tree on cold Qt6 launch ([#1927](https://github.com/trycua/cua/issues/1927)) ([#1998](https://github.com/trycua/cua/issues/1998)) ([b64af30](https://github.com/trycua/cua/commit/b64af303f2f844f20e5de1d10214d6123bf7f680))
* **cua-driver/linux:** return structured launch_app result ([#2091](https://github.com/trycua/cua/issues/2091)) ([c0c6790](https://github.com/trycua/cua/commit/c0c67908167f3d0532683b005ad26659dcab8a34))
* **cua-driver/macos:** set_config accepts {key,value} shape (parity with Win/Linux) ([#2059](https://github.com/trycua/cua/issues/2059)) ([3def0d7](https://github.com/trycua/cua/commit/3def0d79df66b8a9d0b4dfeb71201ddebe672a1d))
* **cua-driver/windows:** retry transient BuildUpdatedCache failures instead of returning elements=0 ([#1881](https://github.com/trycua/cua/issues/1881)) ([#1996](https://github.com/trycua/cua/issues/1996)) ([7518f37](https://github.com/trycua/cua/commit/7518f37a3222761e4a3e391f0516c9ae8fe7fa34))
* **cua-driver/windows:** route DoubleClick/RightClick on Chromium targets via SendInput ([#1984](https://github.com/trycua/cua/issues/1984)) ([#1995](https://github.com/trycua/cua/issues/1995)) ([26d9298](https://github.com/trycua/cua/commit/26d9298e03cc2d6df5d470e9a3707eb21adde398))
* **cua-driver:** add DPI awareness manifest for Windows ([#1821](https://github.com/trycua/cua/issues/1821)) ([b286d9c](https://github.com/trycua/cua/commit/b286d9ccd8ea23d40da9e211dd5ad67fa69cd6a8))
* **cua-driver:** avoid opaque X11 cursor bloom ([#2328](https://github.com/trycua/cua/issues/2328)) ([e57fed5](https://github.com/trycua/cua/commit/e57fed576afba793db756d0b54e6ff6dde3387d6))
* **cua-driver:** default install.sh backend back to Swift ([#1762](https://github.com/trycua/cua/issues/1762)) ([af6bf41](https://github.com/trycua/cua/commit/af6bf41a491f39c28420aaf1abe70e257790678e))
* **cua-driver:** deliver background browser keystrokes ([a7b7413](https://github.com/trycua/cua/commit/a7b741317c86ccd3b1241421e411137f87115eb8))
* **cua-driver:** deliver trusted background browser clicks ([00f33c4](https://github.com/trycua/cua/commit/00f33c4cfaff223a0ade812bb16f4cdb16419d87))
* **cua-driver:** distinguish autostart query failures ([#2345](https://github.com/trycua/cua/issues/2345)) ([0b798a5](https://github.com/trycua/cua/commit/0b798a59cbe7f9e628ae488cb8023b9b6f990bd6))
* **cua-driver:** don't treat socket read-timeout (EAGAIN) as fatal in daemon proxy ([#1864](https://github.com/trycua/cua/issues/1864)) ([#1997](https://github.com/trycua/cua/issues/1997)) ([b651461](https://github.com/trycua/cua/commit/b651461eae8f44083e6d7e1257a4aa1031a05581))
* **cua-driver:** harden browser preview boundaries ([#2347](https://github.com/trycua/cua/pull/2347)) ([667646a](https://github.com/trycua/cua/commit/667646af2d6391d419ff2169e7b4ea3e8d7c9ffc))
* **cua-driver:** harden browser scroll and endpoint proof ([#2353](https://github.com/trycua/cua/pull/2353)) ([7510ed9](https://github.com/trycua/cua/commit/7510ed9c1f82f66c758e6d02e755350d705cacd5))
* **cua-driver:** harden existing browser attachment ([2ccb87c](https://github.com/trycua/cua/commit/2ccb87c8a9cbcaabc428243c8e7fbe809ac63709))
* **cua-driver:** harden Linux AT-SPI and input delivery ([3c63d71](https://github.com/trycua/cua/commit/3c63d712a5eab52103112cfab86fc2ca842442a3))
* **cua-driver:** harden Linux AT-SPI and input delivery ([ea599fd](https://github.com/trycua/cua/commit/ea599fdfd76f49f94a3fc4ddb5486dca68a3fbc4))
* **cua-driver:** harden macOS browser profile setup ([e6643f2](https://github.com/trycua/cua/commit/e6643f21e56e7fcee90c503f8fd39c7659d83b43))
* **cua-driver:** harden macOS input and capture delivery ([4e1aab4](https://github.com/trycua/cua/commit/4e1aab42ca4dba143aed20da72878a0ec4290739))
* **cua-driver:** harden macOS input and capture delivery ([ec908f7](https://github.com/trycua/cua/commit/ec908f74cb9f89cb6dbb7af14c18ff71105bd75a))
* **cua-driver:** harden telemetry release paths ([#2214](https://github.com/trycua/cua/issues/2214)) ([06fcc0f](https://github.com/trycua/cua/commit/06fcc0fb3bdf5d111069525d146dcb5aa2050e1b))
* **cua-driver:** harden Windows delivery and GUI validation ([5007f85](https://github.com/trycua/cua/commit/5007f85a37b4e29fea1d34c3fcc4590dd21f1e99))
* **cua-driver:** harden Windows delivery and GUI validation ([3c5c4c1](https://github.com/trycua/cua/commit/3c5c4c13552e349653e4d62e1f03e172e5cd5faa))
* **cua-driver:** Linux list_windows emits `bounds` for cross-platform parity ([#2017](https://github.com/trycua/cua/issues/2017)) ([#2018](https://github.com/trycua/cua/issues/2018)) ([9e84c1d](https://github.com/trycua/cua/commit/9e84c1d8826dc1bfba166456b34f86a9d6212e0d))
* **cua-driver:** Linux X11 and Wayland convergence ([#2182](https://github.com/trycua/cua/issues/2182)) ([51a8e25](https://github.com/trycua/cua/commit/51a8e2593fc88efd4f0678d9ab7bbba50c22c91b))
* **cua-driver:** macOS e2e convergence (conv2 3/7) ([#2183](https://github.com/trycua/cua/issues/2183)) ([1d0ff7c](https://github.com/trycua/cua/commit/1d0ff7cc3ef7cfcadbf86b8c4b96ab396e319317))
* **cua-driver:** make install-local TCC grants survive rebuilds and release↔local switches ([#2360](https://github.com/trycua/cua/pull/2360)) ([af75ca5](https://github.com/trycua/cua/commit/af75ca5e774e01efc0648a408726343e7133ccd1))
* **cua-driver:** model Linux dialog delivery ([31b4f85](https://github.com/trycua/cua/commit/31b4f85755da8d5f5e793745839e2b0639188b22))
* **cua-driver:** parse uninstall.sh under macOS /bin/bash (bash 3.2) ([#1723](https://github.com/trycua/cua/issues/1723)) ([d720891](https://github.com/trycua/cua/commit/d72089181db06d8530bf15023757fbc0a8a65c57))
* **cua-driver:** persist set_config to disk on Windows + Linux ([#2034](https://github.com/trycua/cua/issues/2034)) ([e08574c](https://github.com/trycua/cua/commit/e08574ca0b3c7ca5c0d9ae33e6ceeac9a45e0d20))
* **cua-driver:** preserve approved Windows CDP port proof ([2527e77](https://github.com/trycua/cua/commit/2527e771ca0ea5aaed0c3d62d52ff4b07322264a))
* **cua-driver:** preserve cross-platform browser posture ([6ea7806](https://github.com/trycua/cua/commit/6ea780690388c33498a8111bc618d7fd0bc8d196))
* **cua-driver:** report unknown browser tab selection ([6611622](https://github.com/trycua/cua/commit/6611622d6c6ef355661c80b6ecae765694088153))
* **cua-driver:** reset TCC grant on release/local signing transition ([fa309eb](https://github.com/trycua/cua/commit/fa309eb5ed0aac1e63c9eaf80dcb2a7749b8b472))
* **cua-driver:** retain country-only telemetry ([#2217](https://github.com/trycua/cua/issues/2217)) ([a705c2a](https://github.com/trycua/cua/commit/a705c2ac463392109063c171ca4586a29811103c))
* **cua-driver:** retry daemon socket writes on EAGAIN, write-side mirror of [#1997](https://github.com/trycua/cua/issues/1997) ([#2036](https://github.com/trycua/cua/issues/2036)) ([d524d97](https://github.com/trycua/cua/commit/d524d97b5a414a9df9d62767d627f24b17bb6353))
* **cua-driver:** retry transient Windows CDP discovery ([4f5abc2](https://github.com/trycua/cua/commit/4f5abc2a231e9a8565326d7f58e78477f09e2666))
* **cua-driver:** reuse self-signed local identity ([a27753e](https://github.com/trycua/cua/commit/a27753e457be1947728fcaa298f077bb306172e9))
* **cua-driver:** stabilize background browser tabs ([3348fc5](https://github.com/trycua/cua/commit/3348fc5778e5b0dfb9402d2c13048c301eda9f57))
* **cua-driver:** target page JavaScript exactly ([#2166](https://github.com/trycua/cua/issues/2166)) ([fb5bc19](https://github.com/trycua/cua/commit/fb5bc192a5311d0519447f1d301bdf0b0c93bbb0))
* **cua-driver:** uninstall.ps1 one-liner crashes iex with "Unexpected attribute 'CmdletBinding'" ([#1750](https://github.com/trycua/cua/issues/1750)) ([b8cbe85](https://github.com/trycua/cua/commit/b8cbe852b19e8b4de18f967636192e9175c91380))
* **cua-driver:** wait for background tab input readiness ([a19e909](https://github.com/trycua/cua/commit/a19e909db8bfd236c8f71ab56edfd4c3fdc7c807))
* **cua-driver:** Windows e2e convergence ([#2180](https://github.com/trycua/cua/issues/2180)) ([88f88e1](https://github.com/trycua/cua/commit/88f88e1b5a31810699781e49f2dd9b9733fbaa82))
* **driver:** remove current autostart services on uninstall ([#2358](https://github.com/trycua/cua/issues/2358)) ([4a492f3](https://github.com/trycua/cua/commit/4a492f36704eaf063a82eadd0469d08684c7a1f5))
* **driver:** require daemon-backed calls and stabilize E2E ([#2338](https://github.com/trycua/cua/issues/2338)) ([9251bb0](https://github.com/trycua/cua/commit/9251bb006018d829b452e7109e322eab3f8c9633))
* **get_window_state:** 30s timeout + 2000-node cap for heavy webview apps ([#1754](https://github.com/trycua/cua/issues/1754)) ([73a84f5](https://github.com/trycua/cua/commit/73a84f5d1f2092fbd58e3a53cd1d61f0f3c3807e))
* **release:** synchronize driver skill versions ([#2362](https://github.com/trycua/cua/pull/2362)) ([512e45a](https://github.com/trycua/cua/commit/512e45a03fc0b4390f3d29b6443db5821157f7a0))
