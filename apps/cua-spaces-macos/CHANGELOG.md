# Changelog

## [0.8.0](https://github.com/trycua/cua/compare/cua-spaces-v0.7.2...cua-spaces-v0.8.0) (2026-10-10)


### Features

* **spaces:** empty home with one-click Linux and macOS create ([#4870](https://github.com/trycua/cua/issues/4870)) ([2e4736b](https://github.com/trycua/cua/commit/2e4736b3ebff61ef99e8c0c74270b5cd75894643))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))


### Bug Fixes

* **macos:** offer retry when stream provider creation fails ([#4807](https://github.com/trycua/cua/issues/4807)) ([8154096](https://github.com/trycua/cua/commit/81540962dbfbb0f9566cef8607288f3afdf65dd2))
* **macos:** refresh open detail after device approval ([#4794](https://github.com/trycua/cua/issues/4794)) ([59e372e](https://github.com/trycua/cua/commit/59e372ec6b29f05b3ae107e7c405fbf8f1fb81d1))
* **spaces-macos:** hit-test the notch tab as drawn, not at its widest ([#4611](https://github.com/trycua/cua/issues/4611)) ([a0394a6](https://github.com/trycua/cua/commit/a0394a648bc88af50c26d2a4c9e270186096e82a)), closes [#4610](https://github.com/trycua/cua/issues/4610)
* **spaces-macos:** keep the notch panel's search inside the glass panel ([#4847](https://github.com/trycua/cua/issues/4847)) ([384f0b3](https://github.com/trycua/cua/commit/384f0b3b28097a4aee90fb848f5348061e345fde)), closes [#4644](https://github.com/trycua/cua/issues/4644)
* **spaces-macos:** send Command chords to the Space while the viewer has the keyboard ([#4848](https://github.com/trycua/cua/issues/4848)) ([f2b32eb](https://github.com/trycua/cua/commit/f2b32eb2d854cea6d0aa450e44fbac9983b063f6)), closes [#4609](https://github.com/trycua/cua/issues/4609)
* **spaces-macos:** stack empty-home tiles when the detail pane is narrow ([#4912](https://github.com/trycua/cua/issues/4912)) ([5274342](https://github.com/trycua/cua/commit/5274342fbf66ca2998325be5e900299406ca8650))
* **spaces:** preserve live OS metadata and show unknown honestly ([#4671](https://github.com/trycua/cua/issues/4671)) ([f25fe36](https://github.com/trycua/cua/commit/f25fe3691e651cc8ac6adf92931940ed25d9a6bb))
* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))

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
