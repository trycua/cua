# Changelog

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
