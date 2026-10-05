# Changelog

## [0.4.0](https://github.com/trycua/cua/compare/cua-sdk-v0.3.1...cua-sdk-v0.4.0) (2026-10-05)


### Features

* **spaces:** expose a Space's authenticated spacesd client ([#4592](https://github.com/trycua/cua/issues/4592)) ([1f9a627](https://github.com/trycua/cua/commit/1f9a627557859a5a9ea417c54340120ce9e8eb36))


### Bug Fixes

* **cua-driver:** refuse set_value on Finder's Get Info Name field ([#4614](https://github.com/trycua/cua/issues/4614)) ([f8c292a](https://github.com/trycua/cua/commit/f8c292a95135d7c87d1f426bad3afa271a598f80)), closes [#4577](https://github.com/trycua/cua/issues/4577)
* **cua-sdk:** identify the serving daemon during overlays ([ecdd366](https://github.com/trycua/cua/commit/ecdd3669e38a5db0b491599609d41f8139a4d19c))
* lock the cua SDK checkout at 0.3.1 ([#4593](https://github.com/trycua/cua/issues/4593)) ([fcb386f](https://github.com/trycua/cua/commit/fcb386f4f958d1357af31ccb0056376f06d9db37))
* preserve borrowed sandboxes and remove speculative cleanup ([a597517](https://github.com/trycua/cua/commit/a597517d8077575050f40eaf31b575b868aed04f))
* **spaces-macos:** list relay machines after an update that misses the daemon handoff ([#4625](https://github.com/trycua/cua/issues/4625)) ([b05ce71](https://github.com/trycua/cua/commit/b05ce7166fb953fe69b157e66ae27e6d637d2e03))
* **spaces:** don't count the image pull against the macOS readiness timeout ([#4622](https://github.com/trycua/cua/issues/4622)) ([033b980](https://github.com/trycua/cua/commit/033b980cd2d1ed744309843cfa8fe3c18ca368e6))

## [0.3.1](https://github.com/trycua/cua/compare/cua-sdk-v0.3.0...cua-sdk-v0.3.1) (2026-10-03)


### Bug Fixes

* **cua-spacesd:** let an image mark features not applicable; bench images skip volume.mount ([#4568](https://github.com/trycua/cua/issues/4568)) ([fe6d89d](https://github.com/trycua/cua/commit/fe6d89d8049d61e4beab66dbeabc176c216e114f))
* **spaces-macos:** pause relay sharing while signed out; allow both settings off ([#4570](https://github.com/trycua/cua/issues/4570)) ([9206f15](https://github.com/trycua/cua/commit/9206f15d4b7c6887271e0219609b1a78bc48b04d))
* **spaces:** classify host access by route; collapse and page the This machine log ([#4540](https://github.com/trycua/cua/issues/4540)) ([ab8239a](https://github.com/trycua/cua/commit/ab8239a076c5681f8e8a51902d2ed2b38b531433))

## [0.3.0](https://github.com/trycua/cua/compare/cua-sdk-v0.2.0...cua-sdk-v0.3.0) (2026-10-03)


### Features

* **cua-driver:** let embedding hosts set the macOS key gap ([#3489](https://github.com/trycua/cua/issues/3489)) ([310cdfd](https://github.com/trycua/cua/commit/310cdfd589631bba98444d1e4e77ed03269e0d80))
* **keyvault:** per-app secret items with unattended locks, search and per-domain review ([#4444](https://github.com/trycua/cua/issues/4444)) ([7f1a03f](https://github.com/trycua/cua/commit/7f1a03fbc3f231b9cbd0ad99863c41bcf3c12bd0))
* **keyvault:** Windows DPAPI, Firefox and Electron secret items ([#4463](https://github.com/trycua/cua/issues/4463)) ([cb685fa](https://github.com/trycua/cua/commit/cb685fad7aef1df6a35ffec653295a0cea4daee6))
* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))
* **spaces-macos:** a built-in gVisor Linux runtime, so Linux Spaces need no Docker on a Mac ([#4493](https://github.com/trycua/cua/issues/4493)) ([7d4fbac](https://github.com/trycua/cua/commit/7d4fbac7f7eaa2d2d9807bd1c320635f21aee2d7))
* **spaces-macos:** host setup that just works, your other Mac in Run on, and a built-in Lume ([#4489](https://github.com/trycua/cua/issues/4489)) ([5748bc5](https://github.com/trycua/cua/commit/5748bc5637b0fb4745f0f2160794b0b24f9c5caa))
* **spaces-macos:** list your machines before this Mac is enrolled, and explain machines that keep their desktop private ([#4511](https://github.com/trycua/cua/issues/4511)) ([379085c](https://github.com/trycua/cua/commit/379085c5e267451db8d52989f47bd9fb85ab38ef))
* **spaces:** Space thumbnails in the daemon and SDK; blurred connecting preview and an auto-connect setting ([#4442](https://github.com/trycua/cua/issues/4442)) ([7b98efd](https://github.com/trycua/cua/commit/7b98efd35b50e1ffaa796385c1d35534f8cb5bb2))
* **telemetry:** record agent runs from every entry point ([#4497](https://github.com/trycua/cua/issues/4497)) ([66e0b66](https://github.com/trycua/cua/commit/66e0b6652fc4a89318e84955e71760646fc52350))


### Bug Fixes

* **cua-driver:** close the menu a failed macOS invoke_menu path opened ([#4484](https://github.com/trycua/cua/issues/4484)) ([154ca56](https://github.com/trycua/cua/commit/154ca5690350217f0824e004d04c031e5c411883))
* **cua-driver:** count an erroring macOS toggle press when its value moved ([#4485](https://github.com/trycua/cua/issues/4485)) ([ca59dc0](https://github.com/trycua/cua/commit/ca59dc0f15ace9c7c643042163bfd9086c88d9e8)), closes [#3835](https://github.com/trycua/cua/issues/3835)
* **cua-driver:** refuse set_value on Finder file name cells instead of confirming a rename that never happened ([#4483](https://github.com/trycua/cua/issues/4483)) ([323af7c](https://github.com/trycua/cua/commit/323af7c39876a4464ad6c4ce2acf1cf17b049921)), closes [#4469](https://github.com/trycua/cua/issues/4469)
* **cua-sdk:** name the cua-spacesd release to update to when a Space's is too old ([#4508](https://github.com/trycua/cua/issues/4508)) ([2bce444](https://github.com/trycua/cua/commit/2bce4442c107fc34c45b374b29a35d09f630fd83)), closes [#4480](https://github.com/trycua/cua/issues/4480)
* **cua-sdk:** remove a deleted Space's stale relay machine record ([#4510](https://github.com/trycua/cua/issues/4510)) ([5526f71](https://github.com/trycua/cua/commit/5526f71b7b70464a748157ef9d293181e2bfbb85))
* **cua-spacesd:** preserve Unicode host caller metadata ([#4501](https://github.com/trycua/cua/issues/4501)) ([a6912c9](https://github.com/trycua/cua/commit/a6912c941aae1324ec7703b4c4282afc43896231))
* never ship a CLI that pins an unpublished cua-spacesd ([#4473](https://github.com/trycua/cua/issues/4473)) ([da46c4b](https://github.com/trycua/cua/commit/da46c4bc85bc43f9641d3ce4b6f319e6d7b6c1a9))
* post-launch CI follow-ups ([#4405](https://github.com/trycua/cua/issues/4405)) ([352507b](https://github.com/trycua/cua/commit/352507b6c03162ab286b21d5ed509125cc3daece))
* **spaces-macos:** access indicators, teleport progress steps, layout nits ([#4424](https://github.com/trycua/cua/issues/4424)) ([ed0a57d](https://github.com/trycua/cua/commit/ed0a57d50e02e68b9563597896ce6600205aafb0))
* **spaces-macos:** make a spare Mac a Spaces host you can use from another Mac ([#4512](https://github.com/trycua/cua/issues/4512)) ([410578a](https://github.com/trycua/cua/commit/410578a4a18e91fbee5d0a169342e3fb669a5bb9))
* **spaces-macos:** notch tiles show the OS logo and where the Space runs ([#4432](https://github.com/trycua/cua/issues/4432)) ([ec8fe1a](https://github.com/trycua/cua/commit/ec8fe1a324168a47c7916124cb663760cdf922c9))
* **spaces-macos:** sign in inline and retry transient failures in host setup ([#4490](https://github.com/trycua/cua/issues/4490)) ([a129ab6](https://github.com/trycua/cua/commit/a129ab6a5a2e10e50271e27a157fd3afcdd424a4))
* **spaces:** reattach Cua Volume after a daemon restart; keep the cua keychain unlocked ([#4458](https://github.com/trycua/cua/issues/4458)) ([9eb7edb](https://github.com/trycua/cua/commit/9eb7edbfc7c70632610491be518fa8580619d141))
* **telemetry:** record signed_in for every sign-in, not only the first run ([#4492](https://github.com/trycua/cua/issues/4492)) ([41c34cb](https://github.com/trycua/cua/commit/41c34cb0d704d816e612dd3f9d0c816cdfacf178))
* **teleport:** install cookies into a Chrome that was never launched ([#4436](https://github.com/trycua/cua/issues/4436)) ([9867f1d](https://github.com/trycua/cua/commit/9867f1d3d1e16f97bfad6b134785c4a3a0e3fa90))
* **teleport:** launch the app after import and trust Chrome on its Safe Storage key ([#4445](https://github.com/trycua/cua/issues/4445)) ([1522b05](https://github.com/trycua/cua/commit/1522b052d51a124ac75e933690a81bd0f2f7dfb1))
* **teleport:** read Chrome cookies WAL-safely and explain v20 ([#4438](https://github.com/trycua/cua/issues/4438)) ([6fbcdf9](https://github.com/trycua/cua/commit/6fbcdf97b6ed953e70ea5efad5a61d3ffb490a2f))

## [0.2.0](https://github.com/trycua/cua/compare/cua-sdk-v0.2.0...cua-sdk-v0.2.0) (2026-10-01)


### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))

## Changelog

Release notes for the cua SDK (`cua-sdk-v*`: PyPI `cua`, npm `@trycua/cua`, Swift `Cua`, the `cua` CLI).

## Release notes: the unified cua SDK (first cua-sdk release)

Release Please adds the version heading and commit list above this section
when it opens the cua-sdk release PR. These are the curated notes for that
first release (the unified cua SDK, the `cua` CLI and daemon, and Spaces on
the SDK); copy them into the release description when the release is
published.

### Behaviour changes

* **refs:** sandboxes and Spaces share one ref scheme: `local:<name>`, `cloud:<name>`, `direct:<host:port>`, `relay:<machine-id>`. `sb.id` (and `SandboxInfo.id`, the daemon's `Sandbox.id`, Space ids) is the qualified ref; every call that takes a sandbox name accepts a ref, and a bare name must be unique across locations, else the new typed `AmbiguousSandbox` error (`CuaError.AmbiguousSandbox`, cua-sandbox `AmbiguousSandbox` with `.candidates`, daemon reason `AMBIGUOUS_SANDBOX`) lists the qualified candidates. `local=` (CLI `--local` / `--cloud` on every NAME command) narrows a bare name. `parse_sandbox_ref`, `qualify_sandbox_ref` and `ambiguous_sandbox_candidates` are exported in every binding. Legacy spellings still parse (`space://{fleet,local,direct,relay}/...`, `fleet:<ns>:<claim>`, `url:<addr>`, URLs) and stored Spaces registries migrate on read; output always uses the new form. One address is one ref: a new direct connection to an address replaces the earlier one (with its token).
* **refs:** the `location` vocabulary is `local | cloud | direct | relay` everywhere: the Spaces provider (`SpaceInfo.provider` is `cloud`, not `fleet`; `fleet` still parses), the Spaces contract (0.3.0: `providers` use these words, `relay` added) and CLI output (`--on cloud`, `--on direct:<addr>`, with `fleet` and `url:<addr>` as aliases; `cua sb ls` prints an `ID` column of refs).
* **refs:** cloud sandbox names are unique within the account: creating a named cloud sandbox fails with `InvalidArgument` when another pool already has a claim of that name (the same pool still reattaches).
* **image:** the local image prefix naming a cloud pool's template is `pool:<name>`; `fleet:<name>` is a deprecated alias (it logs a warning), so `fleet:` never names an image.
* **fleet:** `cua fleet pool export --terraform` writes every attribute the fleets provider has (`command`, `args`, `env`, `process_mode`, `claim_secrets`, `idle_ttl_seconds`, `ttl_policy`, `ttl_seconds_after_created`) and, for a pool with a private-registry pull secret, a `fleets_registry_secret` resource whose username and password are Terraform variables. `sidecar` blocks (now with `args`, `cpu`, `memory`) stay commented until the provider has them.
* **sdk:** local is the default provider on every surface. `SandboxCreateOptions.provider` is optional: unset means local, unless `url` is set (direct) or cloud options are (`cloud`, or the deprecated flat Fleet fields), which imply Fleet. `provider: Local` with `cloud` is `InvalidArgument`. The daemon applies the same rule, and the `cua mcp` / `cua daemon mcp` `sandbox_create` tool defaults to `local=true` (a `pool` implies the cloud).
* **image:** `Image.fromRegistry` / `from_registry` and `resolve_image` are literal (`ubuntu:24.04` is `docker.io/library/ubuntu:24.04`). Aliases apply only through `Image.linux()/windows()/macos()` and the CLI's bare words (`linux`, `windows`, `macos[:v]`, bare `ubuntu`).
* **sdk:** `sandbox.service(name).public_url(ttl, label)` (TS `publicUrl`).
* **cli:** `cua sb mcp NAME SVC config` masks credential headers; `--show-secrets` prints them.
* **sdk:** listing shows every sandbox by default: `sandboxes().list()` (no provider) returns local, direct and live cloud (Fleet) sandboxes, each with `location`; a provider filters. Without Fleet credentials the cloud part is left out silently; when Fleet fails or takes over 5 s it is left out with a warning (`list_with_warnings`, daemon `ListSandboxesResponse.warnings`; `include_cloud` defaults to true). `list_all()` is a deprecated alias. `cua sb ls` lists everything (`--local`, `--cloud` filter; a Fleet failure is a warning, exit 0); MCP `sandbox_list` takes `location`. `cua do ls` is unchanged. JSON rows carry `location` (`where` is deprecated).
* **auth:** a non-secret session marker, `~/.cua/session.json` (`store`, `account`, `expires_at`; mode 0600), is written when a session is stored and removed at logout, so implicit calls (the default sandbox listing) read the OS keychain only when a session is there. **One-time step for sessions stored before this release:** run `cua auth status` (or any cloud command, such as `cua sb ls --cloud`); its successful read writes the marker, and the default listing includes cloud sandboxes from then on. `may_have_fleet_session()` exposes the check in every binding.
* **sdk:** cloud `suspend` / `restart` are `Unsupported` (Fleet cannot suspend one claim) and no longer release the claim or scale a user pool to zero; `resume` of a running cloud sandbox reattaches.
