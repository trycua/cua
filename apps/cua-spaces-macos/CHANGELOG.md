# Changelog

## [0.9.1](https://github.com/trycua/cua/compare/cua-spaces-v0.9.0...cua-spaces-v0.9.1) (2026-10-15)


### Bug Fixes

* **cua-spaces:** show a create's real stages and download progress, also on another machine ([#5198](https://github.com/trycua/cua/issues/5198)) ([13facff](https://github.com/trycua/cua/commit/13facfffdd95927ebac4218f10b8275d69f40759))
* **cua-spaces:** typed resource values, the generated Space name, and picking saved passwords ([#5205](https://github.com/trycua/cua/issues/5205)) ([a14eadd](https://github.com/trycua/cua/commit/a14eadd3b8a563dc4b73d58ff07cd4a2df1a9dc1))
* **spaces:** follow window drags when the notch setting turns on live (CUA-1618) ([#5189](https://github.com/trycua/cua/issues/5189)) ([29598e2](https://github.com/trycua/cua/commit/29598e25ba607bfb65e80c4acd49b8c4696edd69))
* **spaces:** list only Spaces in the notch, never the machines that provide them ([#5190](https://github.com/trycua/cua/issues/5190)) ([1ed129f](https://github.com/trycua/cua/commit/1ed129f64eceaa97cb9f0a082d91512f58f523a7))
* **spaces:** one machine state in both UIs, from the core (CUA-1617) ([#5188](https://github.com/trycua/cua/issues/5188)) ([f820274](https://github.com/trycua/cua/commit/f8202745a66a844bbafc4811878f3a43da0c36fb))
* **spaces:** open a notification to read it in full (CUA-1636) ([#5196](https://github.com/trycua/cua/issues/5196)) ([4582261](https://github.com/trycua/cua/commit/45822615665bb09585b7d0aa1faaaca99f65ca11))
* **spaces:** Teleport's review says what happens for "Just the app" (CUA-1616) ([#5187](https://github.com/trycua/cua/issues/5187)) ([a8d9f38](https://github.com/trycua/cua/commit/a8d9f38ff49f5f0ed8c5fd9681945f3aa97491ae))
* **spaces:** turn Space sharing on by default ([#5218](https://github.com/trycua/cua/issues/5218)) ([8049050](https://github.com/trycua/cua/commit/8049050559f77647f08c78fc276a70f19a660a7f))
* **spaces:** Upgrade falls back to the account app's billing page (CUA-1515) ([#5128](https://github.com/trycua/cua/issues/5128)) ([4b37441](https://github.com/trycua/cua/commit/4b37441420ca4c5fe09ac4fee575e9db221b410b))

## [0.9.0](https://github.com/trycua/cua/compare/cua-spaces-v0.8.0...cua-spaces-v0.9.0) (2026-10-14)


### Features

* **spaces:** drag a window onto a Space tile in the New UI to teleport it (CUA-1511) ([#5099](https://github.com/trycua/cua/issues/5099)) ([f157d9c](https://github.com/trycua/cua/commit/f157d9c2a91657ba5afd9ba07b34501629951bbe))
* **spaces:** New UI on by default for Beta (0.8.1), saved only as an explicit choice (CUA-1514) ([#5108](https://github.com/trycua/cua/issues/5108)) ([3eb7c09](https://github.com/trycua/cua/commit/3eb7c09ce3cd343cf4e1d5b2dba7ce49c6e47989))
* **spaces:** real Teams screen, api.cua.ai and app.cua.ai (0.8.1) ([#5113](https://github.com/trycua/cua/issues/5113)) ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))


### Bug Fixes

* **spaces:** drop the last run.cua.ai billing link from the Swift fixture ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))
* **spaces:** same checklist count and every machine in every UI ([#5106](https://github.com/trycua/cua/issues/5106)) ([ff6079b](https://github.com/trycua/cua/commit/ff6079bef5b6089a5ef649c9447e441c72c9519b))
* **spaces:** Teams clients use Keycloak orgs via the Spaces API ([#5157](https://github.com/trycua/cua/issues/5157)) ([49a89ff](https://github.com/trycua/cua/commit/49a89ff38a06e5b17413d52d2cf6534a13cdba7f))
* **spaces:** the apps stop calling the retired run.cua.ai (CUA-1515) ([#5098](https://github.com/trycua/cua/issues/5098)) ([499130b](https://github.com/trycua/cua/commit/499130b2cb1394e973eafd286b4377beec847ab5))

## [0.8.0](https://github.com/trycua/cua/compare/cua-spaces-v0.7.2...cua-spaces-v0.8.0) (2026-10-13)


### Features

* **cua:** CUA_ENV=staging targets the Cua staging deployment (CUA-1395) ([#5013](https://github.com/trycua/cua/issues/5013)) ([72f329d](https://github.com/trycua/cua/commit/72f329d85246c42458b900d6b21b8cd34b830d05))
* **spaces-macos:** Machines page, Get started checklist, ⌘K palette, ⌘1-4 (CUA-1412) ([#5035](https://github.com/trycua/cua/issues/5035)) ([5b4a16a](https://github.com/trycua/cua/commit/5b4a16a522c71a3d154cea251b87225750a17fdc))
* **spaces-macos:** open the New UI from the notch and menu bar (CUA-1407) ([#5031](https://github.com/trycua/cua/issues/5031)) ([e088b19](https://github.com/trycua/cua/commit/e088b19c3c68b1a863a4af190f1d7ba20d6791b1))
* **spaces-macos:** passphrase setup and unlock in the New UI (CUA-1411) ([#5032](https://github.com/trycua/cua/issues/5032)) ([f982ea0](https://github.com/trycua/cua/commit/f982ea064bb1f6e29f6f2004e5021407f7006320))
* **spaces-macos:** run conversation timeline in the SwiftUI app (CUA-1413) ([#5028](https://github.com/trycua/cua/issues/5028)) ([75a81cf](https://github.com/trycua/cua/commit/75a81cf81629408f428139374b161d4c078eb708))
* **spaces-web:** persistent agent Memory, Routines and Access in the React UI (CUA-1408) ([#5033](https://github.com/trycua/cua/issues/5033)) ([541abc5](https://github.com/trycua/cua/commit/541abc55c64bdce0832e43d1002d6571407c098f))
* **spaces:** add a machine by link or QR code in the app (CUA-1358) ([#5006](https://github.com/trycua/cua/issues/5006)) ([a10b913](https://github.com/trycua/cua/commit/a10b91323d7082a2bb28fce3eb77c68dbcf9d45a))
* **spaces:** empty home with one-click Linux and macOS create ([#4870](https://github.com/trycua/cua/issues/4870)) ([2e4736b](https://github.com/trycua/cua/commit/2e4736b3ebff61ef99e8c0c74270b5cd75894643))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))
* **spaces:** presence cursors over the React video and window-drag teleport in Electron (CUA-1414) ([#5030](https://github.com/trycua/cua/issues/5030)) ([a77bd52](https://github.com/trycua/cua/commit/a77bd52c0e6132da280f8db61e2f4afa24105648))
* **spaces:** register the ⌘⇧Space global shortcut on macOS (CUA-1415) ([#5026](https://github.com/trycua/cua/issues/5026)) ([41047e2](https://github.com/trycua/cua/commit/41047e23d469b4e9b7dfd32331210692746cd8d7))
* **spaces:** relay usage row and allowance notice on the Mac app (CUA-1410) ([#5034](https://github.com/trycua/cua/issues/5034)) ([a329fcf](https://github.com/trycua/cua/commit/a329fcf435191655a8dc8271c0df9ea824c2b564))


### Bug Fixes

* **cua-spaces-macos:** ask before installing cua and adding it to PATH ([#5039](https://github.com/trycua/cua/issues/5039)) ([80ae9a4](https://github.com/trycua/cua/commit/80ae9a47ad9998a819bd5636b84e7eccb4a15841))
* **macos:** offer retry when stream provider creation fails ([#4807](https://github.com/trycua/cua/issues/4807)) ([8154096](https://github.com/trycua/cua/commit/81540962dbfbb0f9566cef8607288f3afdf65dd2))
* **macos:** refresh open detail after device approval ([#4794](https://github.com/trycua/cua/issues/4794)) ([59e372e](https://github.com/trycua/cua/commit/59e372ec6b29f05b3ae107e7c405fbf8f1fb81d1))
* **spaces-macos:** hit-test the notch tab as drawn, not at its widest ([#4611](https://github.com/trycua/cua/issues/4611)) ([a0394a6](https://github.com/trycua/cua/commit/a0394a648bc88af50c26d2a4c9e270186096e82a)), closes [#4610](https://github.com/trycua/cua/issues/4610)
* **spaces-macos:** keep the notch panel's search inside the glass panel ([#4847](https://github.com/trycua/cua/issues/4847)) ([384f0b3](https://github.com/trycua/cua/commit/384f0b3b28097a4aee90fb848f5348061e345fde)), closes [#4644](https://github.com/trycua/cua/issues/4644)
* **spaces-macos:** New UI (preview) replaces the main window instead of adding one ([#5054](https://github.com/trycua/cua/issues/5054)) ([d35d1cc](https://github.com/trycua/cua/commit/d35d1ccd08b266af28149f1afd200eeffbf2bf76))
* **spaces-macos:** open the viewer when ready, let the Space toolbar overflow, and unclip onboarding ([#5016](https://github.com/trycua/cua/issues/5016)) ([0f92100](https://github.com/trycua/cua/commit/0f921000ba7721ffc7be8b563dbb96ef8b3d543c))
* **spaces-macos:** send Command chords to the Space while the viewer has the keyboard ([#4848](https://github.com/trycua/cua/issues/4848)) ([f2b32eb](https://github.com/trycua/cua/commit/f2b32eb2d854cea6d0aa450e44fbac9983b063f6)), closes [#4609](https://github.com/trycua/cua/issues/4609)
* **spaces-macos:** stack empty-home tiles when the detail pane is narrow ([#4912](https://github.com/trycua/cua/issues/4912)) ([5274342](https://github.com/trycua/cua/commit/5274342fbf66ca2998325be5e900299406ca8650))
* **spaces:** leave the machines that provide Spaces out of the menu bar count ([#5050](https://github.com/trycua/cua/issues/5050)) ([97b2394](https://github.com/trycua/cua/commit/97b2394fc1173588e04af6901385a167d7f89029))
* **spaces:** preserve live OS metadata and show unknown honestly ([#4671](https://github.com/trycua/cua/issues/4671)) ([f25fe36](https://github.com/trycua/cua/commit/f25fe3691e651cc8ac6adf92931940ed25d9a6bb))
* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))
* **spaces:** show the runtime-inclusive Linux disk footprint ([#5038](https://github.com/trycua/cua/issues/5038)) ([a3f9a25](https://github.com/trycua/cua/commit/a3f9a2595f5358b78d3ba03a7657ac127ef43f2e))

## [0.7.2](https://github.com/trycua/cua/compare/cua-spaces-v0.7.1...cua-spaces-v0.7.2) (2026-10-05)


### Bug Fixes

* **spaces-macos:** ship the bundled cua SDK's sign-in refresh fixes ([#4645](https://github.com/trycua/cua/issues/4645)) ([b930198](https://github.com/trycua/cua/commit/b930198a23381f2f25205a61c64ece045aed7b44))

## [0.7.1](https://github.com/trycua/cua/compare/cua-spaces-v0.7.0...cua-spaces-v0.7.1) (2026-10-05)


### Bug Fixes

* **spaces-macos:** list relay machines after an update that misses the daemon handoff ([#4625](https://github.com/trycua/cua/issues/4625)) ([b05ce71](https://github.com/trycua/cua/commit/b05ce7166fb953fe69b157e66ae27e6d637d2e03))

## [0.7.0](https://github.com/trycua/cua/compare/cua-spaces-v0.6.1...cua-spaces-v0.7.0) (2026-10-03)


### Features

* **cua-spaces:** drag-the-key DMG installer and keycap app icon ([#4558](https://github.com/trycua/cua/issues/4558)) ([66ab76b](https://github.com/trycua/cua/commit/66ab76b656a8017715c487708184050b71d4618f))


### Bug Fixes

* **spaces-macos:** ask to relaunch when the app was replaced under it, instead of a failing update ([#4569](https://github.com/trycua/cua/issues/4569)) ([5d1b240](https://github.com/trycua/cua/commit/5d1b240f7172e0b570fd1466072e0bd876e3a5b7))
* **spaces-macos:** drop the New Space button beside a machine's not-sharing note ([#4565](https://github.com/trycua/cua/issues/4565)) ([2400bbe](https://github.com/trycua/cua/commit/2400bbefdb9adb5b5eee0aca2e978a0708567d41))
* **spaces-macos:** pause relay sharing while signed out; allow both settings off ([#4570](https://github.com/trycua/cua/issues/4570)) ([9206f15](https://github.com/trycua/cua/commit/9206f15d4b7c6887271e0219609b1a78bc48b04d))

## [0.6.1](https://github.com/trycua/cua/compare/cua-spaces-v0.6.0...cua-spaces-v0.6.1) (2026-10-03)


### Bug Fixes

* **spaces:** classify host access by route; collapse and page the This machine log ([#4540](https://github.com/trycua/cua/issues/4540)) ([ab8239a](https://github.com/trycua/cua/commit/ab8239a076c5681f8e8a51902d2ed2b38b531433))

## [0.6.0](https://github.com/trycua/cua/compare/cua-spaces-v0.5.0...cua-spaces-v0.6.0) (2026-10-03)


### Features

* **spaces-macos:** list your machines before this Mac is enrolled, and explain machines that keep their desktop private ([#4511](https://github.com/trycua/cua/issues/4511)) ([379085c](https://github.com/trycua/cua/commit/379085c5e267451db8d52989f47bd9fb85ab38ef))


### Bug Fixes

* **spaces-macos:** make a spare Mac a Spaces host you can use from another Mac ([#4512](https://github.com/trycua/cua/issues/4512)) ([410578a](https://github.com/trycua/cua/commit/410578a4a18e91fbee5d0a169342e3fb669a5bb9))

## [0.5.0](https://github.com/trycua/cua/compare/cua-spaces-v0.4.0...cua-spaces-v0.5.0) (2026-10-03)


### Features

* **spaces-macos:** a built-in gVisor Linux runtime, so Linux Spaces need no Docker on a Mac ([#4493](https://github.com/trycua/cua/issues/4493)) ([7d4fbac](https://github.com/trycua/cua/commit/7d4fbac7f7eaa2d2d9807bd1c320635f21aee2d7))


### Bug Fixes

* **telemetry:** record signed_in for every sign-in, not only the first run ([#4492](https://github.com/trycua/cua/issues/4492)) ([41c34cb](https://github.com/trycua/cua/commit/41c34cb0d704d816e612dd3f9d0c816cdfacf178))

## [0.4.0](https://github.com/trycua/cua/compare/cua-spaces-v0.3.1...cua-spaces-v0.4.0) (2026-10-03)


### Features

* **spaces-macos:** host setup that just works, your other Mac in Run on, and a built-in Lume ([#4489](https://github.com/trycua/cua/issues/4489)) ([5748bc5](https://github.com/trycua/cua/commit/5748bc5637b0fb4745f0f2160794b0b24f9c5caa))


### Bug Fixes

* **spaces-macos:** sign in inline and retry transient failures in host setup ([#4490](https://github.com/trycua/cua/issues/4490)) ([a129ab6](https://github.com/trycua/cua/commit/a129ab6a5a2e10e50271e27a157fd3afcdd424a4))

## [0.3.1](https://github.com/trycua/cua/compare/cua-spaces-v0.3.0...cua-spaces-v0.3.1) (2026-10-02)


### Bug Fixes

* never ship a CLI that pins an unpublished cua-spacesd ([#4473](https://github.com/trycua/cua/issues/4473)) ([da46c4b](https://github.com/trycua/cua/commit/da46c4bc85bc43f9641d3ce4b6f319e6d7b6c1a9))
* **spaces-macos:** stop the app's memory from growing without bound ([#4478](https://github.com/trycua/cua/issues/4478)) ([9313551](https://github.com/trycua/cua/commit/9313551ef3460ba9dbc035ce6528174138ab5000))

## [0.3.0](https://github.com/trycua/cua/compare/cua-spaces-v0.2.0...cua-spaces-v0.3.0) (2026-10-02)


### Features

* **keyvault:** per-app secret items with unattended locks, search and per-domain review ([#4444](https://github.com/trycua/cua/issues/4444)) ([7f1a03f](https://github.com/trycua/cua/commit/7f1a03fbc3f231b9cbd0ad99863c41bcf3c12bd0))
* **spaces:** Space thumbnails in the daemon and SDK; blurred connecting preview and an auto-connect setting ([#4442](https://github.com/trycua/cua/issues/4442)) ([7b98efd](https://github.com/trycua/cua/commit/7b98efd35b50e1ffaa796385c1d35534f8cb5bb2))


### Bug Fixes

* post-launch CI follow-ups ([#4405](https://github.com/trycua/cua/issues/4405)) ([352507b](https://github.com/trycua/cua/commit/352507b6c03162ab286b21d5ed509125cc3daece))
* **spaces-macos:** notch tiles show the OS logo and where the Space runs ([#4432](https://github.com/trycua/cua/issues/4432)) ([ec8fe1a](https://github.com/trycua/cua/commit/ec8fe1a324168a47c7916124cb663760cdf922c9))

## [0.2.0](https://github.com/trycua/cua/compare/cua-spaces-v0.1.0...cua-spaces-v0.2.0) (2026-10-02)


### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))


### Bug Fixes

* **release:** drive Cua Spaces releases from the macOS app ([#4440](https://github.com/trycua/cua/issues/4440)) ([a605a29](https://github.com/trycua/cua/commit/a605a29f888b526b1adbddd8ec385059dec672e6))
* **spaces-macos:** access indicators, teleport progress steps, layout nits ([#4424](https://github.com/trycua/cua/issues/4424)) ([ed0a57d](https://github.com/trycua/cua/commit/ed0a57d50e02e68b9563597896ce6600205aafb0))
* **teleport:** save to Keyvault stores the teleported session ([#4422](https://github.com/trycua/cua/issues/4422)) ([3c5e5c1](https://github.com/trycua/cua/commit/3c5e5c1e677c5dbf06018d6a6e0395adc40043fe))

## [0.1.0](https://github.com/trycua/cua/compare/cua-spaces-v0.1.0...cua-spaces-v0.1.0) (2026-10-01)


### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))

## Changelog

Release notes for the Cua Spaces desktop app (`cua-spaces-v*`: the macOS
installers and the Sparkle update feed, built by
`.github/workflows/cd-cua-spaces.yml`). Release Please drives the `cua-spaces`
component from this macOS app (`apps/cua-spaces-macos`) and owns the version
in `VERSION` and `Support/Info.plist`; the Tauri app in `apps/cua-spaces`
(`package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and
`src-tauri/Cargo.lock`) shares the version but does not trigger releases.
