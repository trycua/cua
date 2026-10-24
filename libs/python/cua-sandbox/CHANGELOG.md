# Changelog

## [0.10.0](https://github.com/trycua/cua/compare/sandbox-v0.9.1...sandbox-v0.10.0) (2026-10-24)


### Features

* compat gateways for E2B and Daytona, Hetzner as your own cloud, the elastic pool, cua share and the public relay ([#5778](https://github.com/trycua/cua/issues/5778)) ([d4f3f80](https://github.com/trycua/cua/commit/d4f3f8063d7b30a3b3bda2b4c05c565a9dfcb460))


### Bug Fixes

* **sandbox:** the [driver] extra installs the current cua-driver (0.37.0) (CUA-1902) ([#5533](https://github.com/trycua/cua/issues/5533)) ([34f3279](https://github.com/trycua/cua/commit/34f3279247e0e910e4896ee33c499beafdb7ab9a))
* **sdk:** delete the Fleet SDK, pools and claims from the Rust SDK, cua-sandbox, Cua Bench and the apps (CUA-1836) ([#5488](https://github.com/trycua/cua/issues/5488)) ([348c47c](https://github.com/trycua/cua/commit/348c47cc30b1d222844480838ccc4acf53f7a511))
* **security:** distrust the compromised eMudhra intermediates in every Cua client (CUA-1948) ([#5622](https://github.com/trycua/cua/issues/5622)) ([149ea51](https://github.com/trycua/cua/commit/149ea51c613cc840c3760dec822c57b7e65df05a))
* **spaces:** say computer instead of machine (CUA-1863) ([#5508](https://github.com/trycua/cua/issues/5508)) ([5cbe31c](https://github.com/trycua/cua/commit/5cbe31c7109fa70eb596b6bde3b5964449e8676e))

## [0.9.1](https://github.com/trycua/cua/compare/sandbox-v0.9.0...sandbox-v0.9.1) (2026-10-19)


### Bug Fixes

* post-launch CI follow-ups ([#4405](https://github.com/trycua/cua/issues/4405)) ([352507b](https://github.com/trycua/cua/commit/352507b6c03162ab286b21d5ed509125cc3daece))
* lock the cua SDK checkout at 0.3.1 ([#4593](https://github.com/trycua/cua/issues/4593)) ([fcb386f](https://github.com/trycua/cua/commit/fcb386f4f958d1357af31ccb0056376f06d9db37))
* **python:** cua-sandbox, cua-bench and the playground no longer depend on Fleet; remove cua-train (CUA-1834) ([#5445](https://github.com/trycua/cua/issues/5445)) ([7733289](https://github.com/trycua/cua/commit/773328941ead1b83a616e8b6c5a5ac4851af7114))
* **sandbox:** report directory types in file listings ([#5175](https://github.com/trycua/cua/issues/5175)) ([1e53b1c](https://github.com/trycua/cua/commit/1e53b1cef43a650ff35a7299c470963244b84ce0))
* **sandbox:** roll back partially opened tunnels ([#5174](https://github.com/trycua/cua/issues/5174)) ([ca6a0e2](https://github.com/trycua/cua/commit/ca6a0e21050966e8ed6a3404b7ba2843b1cf6878))
* stop Cua Cloud (on="cloud") with a clear "Cua Cloud has ended" error (CUA-1854) ([#5415](https://github.com/trycua/cua/issues/5415)) ([5d78ec8](https://github.com/trycua/cua/commit/5d78ec8b7705d942a3d260b276bcce76614a634c))

## [0.9.0](https://github.com/trycua/cua/compare/sandbox-v0.8.0...sandbox-v0.9.0) (2026-10-01)


### ⚠ BREAKING CHANGES

* **sandbox:** Sandbox.create runs locally unless local=False; the localhost module and the computer_server, http, local and websocket transports are removed.

### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))
* **sandbox:** run on the cua SDK, local by default ([#4413](https://github.com/trycua/cua/issues/4413)) ([203cd1e](https://github.com/trycua/cua/commit/203cd1eebd3cc9f8e4706b7af7ab33e9ec578afe))

### Behaviour changes

* **sandbox:** sandboxes are local by default. `Sandbox.create` / `Sandbox.ephemeral` without `on=`/`local=` follow the user default (`CUA_DEFAULT_ON`, else `cua config set default.on cloud`, else local; the built-in local default shows a one-time notice, hidden by `CUA_QUIET_DEFAULT=1`). Pass `on="cloud"`/`local=False` (or `cloud=CloudOptions(...)`, which implies the cloud) for the cloud. For compatibility, omitting both while passing a cloud-only argument (a Fleet `pool`, `warm`, `max_pool_size`, `claim_ttl`, `claim_spec`, `api_key`, ...) still runs in the cloud with a `DeprecationWarning`. `local=True` together with `cloud=`, or `on=` contradicting `local=`, raises `InvalidArgument`. Without cloud credentials, a cloud that came from the default says how to switch back (`cua config set default.on local`).
* **sandbox:** `Sandbox.create` / `ephemeral` / `sandbox()` take `on=`, `kind=` (`auto`, `container`, `vm`) and `runtime=` as an engine name (local `gvisor`, `runc`, `qemu`, `lume`; cloud `gvisor`, `kubevirt`), validated by the cua SDK: a combination that does not exist raises `InvalidPlacement` (an `InvalidArgument`) listing the valid values. `CUA_DEFAULT_KIND` / `CUA_DEFAULT_RUNTIME` and `default.kind` / `default.runtime` fill unset values when they fit. A `Runtime` object keeps working as before. `SandboxInfo` gains `kind` and `runtime`; `Sandbox.list(location="local"|"cloud")` is the same filter as `local=`.
* **sandbox:** `Sandbox.suspend` / `resume` / `restart` on a cloud sandbox raise `Unsupported` (a `NotImplementedError`): Fleet cannot suspend a single sandbox. They no longer scale a pool named after the sandbox. With `local` omitted, these and `connect` / `get_info` / `delete` pick the local sandbox of that name when one exists.
* **sandbox:** `Sandbox.list()` lists local and cloud sandboxes by default (`local=True` / `local=False` filter; `all=` is deprecated), each row with `location`. The cloud part never fails it (silent without credentials, a warning after 5 s or on error) and reads the OS keychain only when the SDK's session marker says a session is stored (for sessions from before this release, run `cua auth status` once). `Sandbox.resume(name)` of a running cloud sandbox reconnects.
* **sandbox:** `SandboxInfo.location` says where a sandbox runs.
* **sandbox:** `Image.from_registry(ref)` is literal: `ubuntu:24.04` is Docker Hub's image, never the canonical Linux alias.

## [0.8.0](https://github.com/trycua/cua/compare/sandbox-v0.7.0...sandbox-v0.8.0) (2026-09-15)


### Features

* **sandbox:** OSWorld disks on Fleet via agent_type="osworld" + "Run OSWorld on Fleet" guide ([#3686](https://github.com/trycua/cua/issues/3686)) ([db8ba21](https://github.com/trycua/cua/commit/db8ba214b6fb954d5d2044a54261552dda5ddaf5))


### Bug Fixes

* **sandbox:** keep Image file sizes JSON-safe ([#3839](https://github.com/trycua/cua/issues/3839)) ([1d6e81e](https://github.com/trycua/cua/commit/1d6e81ea513a06a29a8e756bbc8ff26d64e03a02))

## [0.7.0](https://github.com/trycua/cua/compare/sandbox-v0.6.0...sandbox-v0.7.0) (2026-09-11)


### Features

* **sandbox:** use shared Driver MCP client ([#3718](https://github.com/trycua/cua/issues/3718)) ([d1ac400](https://github.com/trycua/cua/commit/d1ac40014457556c9d66a4d791fa40a8db5b9d68))

## [0.6.0](https://github.com/trycua/cua/compare/sandbox-v0.5.0...sandbox-v0.6.0) (2026-09-10)


### ⚠ BREAKING CHANGES

* **cua-driver:** ClickInput now requires target, position, and delivery_mode; click returns ActionResult directly and raises typed tool errors for refusals.

### Features

* **cua-driver:** expose typed native-window SDK flow ([#3683](https://github.com/trycua/cua/issues/3683)) ([75b04aa](https://github.com/trycua/cua/commit/75b04aac03ed6cb1e08c41b2384595e5c5ab2d9f))
* **sandbox:** connect typed Driver through explicit MCP carrier ([#3693](https://github.com/trycua/cua/issues/3693)) ([dc3d35c](https://github.com/trycua/cua/commit/dc3d35cb40cacceb6d9cc08c61940204ca076a3f))


### Bug Fixes

* **sandbox:** align optional Driver dependency with 0.26.0 ([#3699](https://github.com/trycua/cua/issues/3699)) ([2b748f6](https://github.com/trycua/cua/commit/2b748f64335cb2bf204e9f96d90346460a4419ab))
* **sandbox:** drain MCP responses before cancellation teardown ([#3696](https://github.com/trycua/cua/issues/3696)) ([07e36a3](https://github.com/trycua/cua/commit/07e36a3f05d88ca8be3a04454b8738f81c9d92f6))

## [0.5.0](https://github.com/trycua/cua/compare/sandbox-v0.4.3...sandbox-v0.5.0) (2026-09-09)


### Features

* **cua-driver:** integrate typed Driver access with Fleet Sandbox ([#3654](https://github.com/trycua/cua/issues/3654)) ([c9c29dc](https://github.com/trycua/cua/commit/c9c29dcffea354e3ae0cf75927845e79c9d38028))
* **cua-sandbox:** add signed service URLs ([#3508](https://github.com/trycua/cua/issues/3508)) ([8ab33c7](https://github.com/trycua/cua/commit/8ab33c739f74cd6e7b0a321b7e75ac7b795b4216))
* **sandbox:** add an optional compatible Driver SDK extra ([#3678](https://github.com/trycua/cua/issues/3678)) ([80f2dee](https://github.com/trycua/cua/commit/80f2dee09bca5e44ae6ac6ff3dc0ae81cf91d820))
* **sandbox:** define the Image API contract ([#3327](https://github.com/trycua/cua/issues/3327)) ([a525001](https://github.com/trycua/cua/commit/a52500167b8518dba137abf0b33ea59a504e161d))


### Bug Fixes

* **cua-sandbox:** honor Fleet request timeouts ([#3381](https://github.com/trycua/cua/issues/3381)) ([a925ffa](https://github.com/trycua/cua/commit/a925ffa25e6ea13388e8bead02237785b46e1b4a))
* **sandbox:** manage package releases with Release Please ([#3675](https://github.com/trycua/cua/issues/3675)) ([f880ac4](https://github.com/trycua/cua/commit/f880ac42540d7be6231b89c75115749923fa21be))

## Changelog

Release Please maintains entries for releases after the migration from
`sandbox-v0.4.3`. Earlier releases remain available in the repository's GitHub
release history.
