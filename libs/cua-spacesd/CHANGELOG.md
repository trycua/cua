# Changelog

## [0.3.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.2.2...cua-spacesd-v0.3.0) (2026-10-03)


### Features

* **relay:** log why each 401 is refused ([#4503](https://github.com/trycua/cua/issues/4503)) ([7da7429](https://github.com/trycua/cua/commit/7da7429f302825d3fecb021cd34af8d132c79728))
* **spaces-macos:** host setup that just works, your other Mac in Run on, and a built-in Lume ([#4489](https://github.com/trycua/cua/issues/4489)) ([5748bc5](https://github.com/trycua/cua/commit/5748bc5637b0fb4745f0f2160794b0b24f9c5caa))


### Bug Fixes

* never ship a CLI that pins an unpublished cua-spacesd ([#4473](https://github.com/trycua/cua/issues/4473)) ([da46c4b](https://github.com/trycua/cua/commit/da46c4bc85bc43f9641d3ce4b6f319e6d7b6c1a9))

## [0.2.2](https://github.com/trycua/cua/compare/cua-spacesd-v0.2.1...cua-spacesd-v0.2.2) (2026-10-02)


### Bug Fixes

* **teleport:** Chrome Safe Storage without keychain prompts ([#4470](https://github.com/trycua/cua/issues/4470)) ([5e4738b](https://github.com/trycua/cua/commit/5e4738bff8909e3e2c3f9f2422dd53a109a6b56a))

## [0.2.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.2.0...cua-spacesd-v0.2.1) (2026-10-02)


### Bug Fixes

* **cua-spacesd:** build on Windows again (geteuid is Unix-only) ([#4462](https://github.com/trycua/cua/issues/4462)) ([27e2261](https://github.com/trycua/cua/commit/27e226144cb204392ec8406bf449cf75d1f07ff8))

## [0.2.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.4...cua-spacesd-v0.2.0) (2026-10-02)


### Features

* **keyvault:** per-app secret items with unattended locks, search and per-domain review ([#4444](https://github.com/trycua/cua/issues/4444)) ([7f1a03f](https://github.com/trycua/cua/commit/7f1a03fbc3f231b9cbd0ad99863c41bcf3c12bd0))

## [0.1.4](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.3...cua-spacesd-v0.1.4) (2026-10-02)


### Bug Fixes

* **doctor:** scale the services probe waits with CUA_DOCTOR_TIMEOUT_SCALE ([#4456](https://github.com/trycua/cua/issues/4456)) ([01636cb](https://github.com/trycua/cua/commit/01636cb41d5a776abb7e3d78be70e29d9711f902))
* **spaces:** reattach Cua Volume after a daemon restart; keep the cua keychain unlocked ([#4458](https://github.com/trycua/cua/issues/4458)) ([9eb7edb](https://github.com/trycua/cua/commit/9eb7edbfc7c70632610491be518fa8580619d141))

## [0.1.3](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.2...cua-spacesd-v0.1.3) (2026-10-02)


### Bug Fixes

* post-launch CI follow-ups ([#4405](https://github.com/trycua/cua/issues/4405)) ([352507b](https://github.com/trycua/cua/commit/352507b6c03162ab286b21d5ed509125cc3daece))
* **teleport:** launch the app after import and trust Chrome on its Safe Storage key ([#4445](https://github.com/trycua/cua/issues/4445)) ([1522b05](https://github.com/trycua/cua/commit/1522b052d51a124ac75e933690a81bd0f2f7dfb1))

## [0.1.2](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.1...cua-spacesd-v0.1.2) (2026-10-01)


### Bug Fixes

* **teleport:** install cookies into a Chrome that was never launched ([#4436](https://github.com/trycua/cua/issues/4436)) ([9867f1d](https://github.com/trycua/cua/commit/9867f1d3d1e16f97bfad6b134785c4a3a0e3fa90))

## [0.1.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.0...cua-spacesd-v0.1.1) (2026-10-01)


### Bug Fixes

* **cua-spacesd:** refresh Cargo lockfiles after the 0.1.0 release ([#4427](https://github.com/trycua/cua/issues/4427)) ([aa9d4af](https://github.com/trycua/cua/commit/aa9d4af79d4c7099441813035ff042ec867f56a3))
* **spaces-macos:** access indicators, teleport progress steps, layout nits ([#4424](https://github.com/trycua/cua/issues/4424)) ([ed0a57d](https://github.com/trycua/cua/commit/ed0a57d50e02e68b9563597896ce6600205aafb0))

## [0.1.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.1.0...cua-spacesd-v0.1.0) (2026-10-01)


### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))

## Changelog

Release notes for cua-spacesd (`cua-spacesd-v*`: the in-sandbox daemon's
Linux, macOS and Windows release assets, built by
`.github/workflows/cd-cua-spacesd.yml`). It was called cua-env-driver and then
cua-guestd before; no `cua-env-driver-v*` or `cua-guestd-v*` release was
published.
