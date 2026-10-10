# Changelog

## [0.6.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.5.3...cua-spacesd-v0.6.0) (2026-10-10)


### Features

* **cua-driver:** plan cursor motion with the new cua-cursor-motion crate ([#4758](https://github.com/trycua/cua/issues/4758)) ([558cb53](https://github.com/trycua/cua/commit/558cb534d26ad51680ffd0f85b249bdebe64989a))
* **cua-driver:** plan cursor motions from the SDKs through UniFFI ([#4767](https://github.com/trycua/cua/issues/4767)) ([365f5e3](https://github.com/trycua/cua/commit/365f5e3c5b92f9457dbd560ddea8ec0268565724))
* **cua-driver:** run_script tool: sandboxed JavaScript (QuickJS) that drives the driver's own tools in one call, with wall-time, driver-call, heap and stack limits (on by default since [#4936](https://github.com/trycua/cua/issues/4936)) (CUA-1214) ([#4822](https://github.com/trycua/cua/issues/4822)) ([45469a5](https://github.com/trycua/cua/commit/45469a59c931ddccd78f9c18f1bbba4cb4cdd7be))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))


### Bug Fixes

* **cua-spacesd:** connect hosted machines through HTTP proxies ([#4778](https://github.com/trycua/cua/issues/4778)) ([853e2c5](https://github.com/trycua/cua/commit/853e2c57aedb9f5573d802a8326f966985e28ef7))
* **cua-spacesd:** declare DPI awareness for Windows Host ([#4805](https://github.com/trycua/cua/issues/4805)) ([34d69bf](https://github.com/trycua/cua/commit/34d69bf2c7d9470e9f8e18d5e5f225123afcaafb))
* **cua-spacesd:** restore Windows viewer input through driver tools ([#4528](https://github.com/trycua/cua/issues/4528)) ([6b95048](https://github.com/trycua/cua/commit/6b95048b048d79ad3ce4745abe58d8e9e522144c))
* **cua-spacesd:** stream 6K displays and carry tool results over 4 MiB ([#4850](https://github.com/trycua/cua/issues/4850)) ([a88f100](https://github.com/trycua/cua/commit/a88f100b428a3eb9a73bd60757540d6ecd1fd00f)), closes [#4623](https://github.com/trycua/cua/issues/4623)
* **relay:** audit log in its own file, stale temp cleanup, sessions survive restarts ([#4939](https://github.com/trycua/cua/issues/4939)) ([cfc95fd](https://github.com/trycua/cua/commit/cfc95fd70369461339e1a7af97b15b1814211c06))
* **sdk:** honor HTTP proxies for controller RPC ([#4795](https://github.com/trycua/cua/issues/4795)) ([23e23d4](https://github.com/trycua/cua/commit/23e23d4cc321b1e64f5832d44a150667ff0d5c04))
* **spacesd:** ask get_window_state for structured elements in the tool backend ([#4908](https://github.com/trycua/cua/issues/4908)) ([ba4c636](https://github.com/trycua/cua/commit/ba4c6369660ab4a9c4d3d8af942bc53ad376615f))
* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))

## [0.5.3](https://github.com/trycua/cua/compare/cua-spacesd-v0.5.2...cua-spacesd-v0.5.3) (2026-10-03)


### Bug Fixes

* **cua-spacesd:** let an image mark features not applicable; bench images skip volume.mount ([#4568](https://github.com/trycua/cua/issues/4568)) ([fe6d89d](https://github.com/trycua/cua/commit/fe6d89d8049d61e4beab66dbeabc176c216e114f))

## [0.5.2](https://github.com/trycua/cua/compare/cua-spacesd-v0.5.1...cua-spacesd-v0.5.2) (2026-10-03)


### Bug Fixes

* **relay:** let the owner re-register a machine whose host is gone ([#4553](https://github.com/trycua/cua/issues/4553)) ([21b7bd6](https://github.com/trycua/cua/commit/21b7bd6e0c58b16b35494f6a0e4327f892c007da))

## [0.5.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.5.0...cua-spacesd-v0.5.1) (2026-10-03)


### Bug Fixes

* **spaces:** classify host access by route; collapse and page the This machine log ([#4540](https://github.com/trycua/cua/issues/4540)) ([ab8239a](https://github.com/trycua/cua/commit/ab8239a076c5681f8e8a51902d2ed2b38b531433))

## [0.5.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.4.1...cua-spacesd-v0.5.0) (2026-10-03)


### Features

* **spaces-macos:** list your machines before this Mac is enrolled, and explain machines that keep their desktop private ([#4511](https://github.com/trycua/cua/issues/4511)) ([379085c](https://github.com/trycua/cua/commit/379085c5e267451db8d52989f47bd9fb85ab38ef))


### Bug Fixes

* **cua-sdk:** remove a deleted Space's stale relay machine record ([#4510](https://github.com/trycua/cua/issues/4510)) ([5526f71](https://github.com/trycua/cua/commit/5526f71b7b70464a748157ef9d293181e2bfbb85))
* **spacesd:** honor desktop sharing setting for view-only shares ([#4532](https://github.com/trycua/cua/issues/4532)) ([32c5813](https://github.com/trycua/cua/commit/32c58137936b4d2ba052ab76fffbe1a0cfeaf509))

## [0.4.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.4.0...cua-spacesd-v0.4.1) (2026-10-03)


### Bug Fixes

* **cua-spacesd:** preserve Unicode host caller metadata ([#4501](https://github.com/trycua/cua/issues/4501)) ([a6912c9](https://github.com/trycua/cua/commit/a6912c941aae1324ec7703b4c4282afc43896231))

## [0.4.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.3.0...cua-spacesd-v0.4.0) (2026-10-03)


### Features

* **keyvault:** Windows DPAPI, Firefox and Electron secret items ([#4463](https://github.com/trycua/cua/issues/4463)) ([cb685fa](https://github.com/trycua/cua/commit/cb685fad7aef1df6a35ffec653295a0cea4daee6))


### Bug Fixes

* **spaces-macos:** make a spare Mac a Spaces host you can use from another Mac ([#4512](https://github.com/trycua/cua/issues/4512)) ([410578a](https://github.com/trycua/cua/commit/410578a4a18e91fbee5d0a169342e3fb669a5bb9))

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
