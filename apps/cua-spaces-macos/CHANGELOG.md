# Changelog

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
