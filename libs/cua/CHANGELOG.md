# Changelog

## [0.24.1](https://github.com/trycua/cua/compare/cua-sdk-v0.24.0...cua-sdk-v0.24.1) (2026-11-11)


### Bug Fixes

* **runtime:** setup runsc no longer says already available on a runc-only Docker ([#7314](https://github.com/trycua/cua/issues/7314)) ([bc640ca](https://github.com/trycua/cua/commit/bc640ca8186aed9999f98596decf4b5a9cd9a607))

## [0.24.0](https://github.com/trycua/cua/compare/cua-sdk-v0.23.0...cua-sdk-v0.24.0) (2026-11-11)


### Features

* **spaces:** closing the window keeps Cua Spaces running, Quit stops the background service ([#7136](https://github.com/trycua/cua/issues/7136)) ([4af5875](https://github.com/trycua/cua/commit/4af587598509724ca1c716cbda5092d13794b8ea))

## [0.23.0](https://github.com/trycua/cua/compare/cua-sdk-v0.22.0...cua-sdk-v0.23.0) (2026-11-10)


### Features

* **spaces:** Home shows your computers as one supercomputer (CUA-3976) ([#7166](https://github.com/trycua/cua/issues/7166)) ([49f716e](https://github.com/trycua/cua/commit/49f716e9c7e224db0f1bafbb2957be385f308e01))


### Performance Improvements

* **cua-sdk:** build libcua_sdk in its own crate so cua-sdk is rlib-only (CUA-3971) ([#7250](https://github.com/trycua/cua/issues/7250)) ([73b54ae](https://github.com/trycua/cua/commit/73b54aeb62c5c5a253be761d30f4026dea2f1fd5))

## [0.22.0](https://github.com/trycua/cua/compare/cua-sdk-v0.21.0...cua-sdk-v0.22.0) (2026-11-10)


### Features

* **spaces:** Agents tab starts agents from the Home box, one entry per Space, easy to clear ([#7275](https://github.com/trycua/cua/issues/7275)) ([5a0b65f](https://github.com/trycua/cua/commit/5a0b65f7d6af5a1c09e7f3aced03d809cbd4d521))
* **spaces:** show a child Space's window in the parent's Stream section and stream it directly from the child ([#6819](https://github.com/trycua/cua/issues/6819)) ([5f09d89](https://github.com/trycua/cua/commit/5f09d890ad10a68c02de0cfd4e3fe24096537059))


### Bug Fixes

* **cua-spaces:** read spaces.json once per connect ([#7235](https://github.com/trycua/cua/issues/7235)) ([1344c86](https://github.com/trycua/cua/commit/1344c86d1cf892002c454b5c73139b9bca62f47a))
* **spaces:** a computer's row opens its shared desktop's Space page ([#7263](https://github.com/trycua/cua/issues/7263)) ([7b8d889](https://github.com/trycua/cua/commit/7b8d889130252a72972319d4f6791b59aeb442cb))
* **spaces:** first agent on This Mac after Set up no longer fails to reach it ([#7161](https://github.com/trycua/cua/issues/7161)) ([4611678](https://github.com/trycua/cua/commit/46116784b637483e750f624ef0ef6fb74c745bde))
* **spaces:** the relay row follows the Provide Spaces setting, and a Space on the account is listed once ([#7132](https://github.com/trycua/cua/issues/7132)) ([75fff6c](https://github.com/trycua/cua/commit/75fff6c8cc827d1dd85b81a07cc849778d1f2dee))
* **volume:** a stalled sync never hangs an agent start or the mount (CUA-4002) ([#7237](https://github.com/trycua/cua/issues/7237)) ([7f4a358](https://github.com/trycua/cua/commit/7f4a35873a00a101563394f54f8497d1288a3c8b))


### Performance Improvements

* **cua-ci:** build cua-ci beside cua-sdk, not after it (CUA-3971) ([#7240](https://github.com/trycua/cua/issues/7240)) ([1d23545](https://github.com/trycua/cua/commit/1d23545a03689800046f5fd7ecf78c3d128dec61))
* **cua-cli:** build cua-compat beside cua-cli, not before it (CUA-3971) ([#7222](https://github.com/trycua/cua/issues/7222)) ([5bf6559](https://github.com/trycua/cua/commit/5bf6559ab9942885cb5972928dd764bca5b6f7e2))
* **cua:** split cua-daemon into cua-daemon-core and the server (CUA-3971) ([#7214](https://github.com/trycua/cua/issues/7214)) ([9058fbe](https://github.com/trycua/cua/commit/9058fbe9f873c5ccc038a8350472045869275895))

## [0.21.0](https://github.com/trycua/cua/compare/cua-sdk-v0.20.0...cua-sdk-v0.21.0) (2026-11-10)


### Features

* **cua:** show each Space's progress when spaces create makes several ([#7185](https://github.com/trycua/cua/issues/7185)) ([c4403a6](https://github.com/trycua/cua/commit/c4403a69674986ac086666b3108903cd60e1ab46))
* **spaces:** agent chip shows model and reasoning level, with an inline Reasoning row ([#6861](https://github.com/trycua/cua/issues/6861)) ([bec0f80](https://github.com/trycua/cua/commit/bec0f80852f79ff8c4f6201436a10dcc29ad58d5))
* **spaces:** Child Spaces and Warm start are always on, their Labs switches are gone (CUA-3751) ([#6873](https://github.com/trycua/cua/issues/6873)) ([57942a0](https://github.com/trycua/cua/commit/57942a0e67625dffe0f79f9ea146395ff1cdac53))
* **spaces:** fold the side panel's sections and dock the desktop beside the thread ([#6797](https://github.com/trycua/cua/issues/6797)) ([6ae00f1](https://github.com/trycua/cua/commit/6ae00f1cf9cac8f2ecad5d37e56c10d4d95018fb))


### Bug Fixes

* **cua-daemon:** gate client-only code so the build without the client feature is warning-free ([#7209](https://github.com/trycua/cua/issues/7209)) ([ac6e7bf](https://github.com/trycua/cua/commit/ac6e7bff58ed7884ceb59bf6dedd74681e45e340))
* **spaces:** an offline computer reads offline within seconds, not minutes (CUA-3981) ([#7175](https://github.com/trycua/cua/issues/7175)) ([ac7d344](https://github.com/trycua/cua/commit/ac7d344d5d7e5fedb83cf8d61f50252f49f02c52))
* **spaces:** new-user friction from the 10 Nov device test (CUA-3980) ([#7176](https://github.com/trycua/cua/issues/7176)) ([55730e3](https://github.com/trycua/cua/commit/55730e3f48d43038276e22c6a8cf1fbf1934d9a5))

## [0.20.0](https://github.com/trycua/cua/compare/cua-sdk-v0.19.0...cua-sdk-v0.20.0) (2026-11-10)


### Features

* **cua-driver:** connect --replace switches an agent from another Cua Driver ([#7190](https://github.com/trycua/cua/issues/7190)) ([e15e9dd](https://github.com/trycua/cua/commit/e15e9dd47cdc970e7cbed5fec87e377c28ddae0f))


### Bug Fixes

* **cua-spaces:** hold Child Spaces on in the relay_publish opt-out test ([#7181](https://github.com/trycua/cua/issues/7181)) ([5b89d98](https://github.com/trycua/cua/commit/5b89d98fb55496c104ecac4221229bb2080ebdb9))

## [0.19.0](https://github.com/trycua/cua/compare/cua-sdk-v0.18.1...cua-sdk-v0.19.0) (2026-11-10)


### Features

* **cua:** report agent run and Space create status to the terminal (OSC 7501) ([#5155](https://github.com/trycua/cua/issues/5155)) ([f54ca3d](https://github.com/trycua/cua/commit/f54ca3de1cd7468a5ec2e74be571a80d9b2b532b))

## [0.18.1](https://github.com/trycua/cua/compare/cua-sdk-v0.18.0...cua-sdk-v0.18.1) (2026-11-10)


### Bug Fixes

* **cua-vmm:** one constant for the oldest Lume cua runs on ([#6946](https://github.com/trycua/cua/issues/6946)) ([311d525](https://github.com/trycua/cua/commit/311d525ab705dcebdc82dceefa175c32d2da1e18))

## [0.18.0](https://github.com/trycua/cua/compare/cua-sdk-v0.17.1...cua-sdk-v0.18.0) (2026-11-09)


### Features

* **agents:** official sign-in on each computer, stop copying sign-ins by default (CUA-3907) ([#7045](https://github.com/trycua/cua/issues/7045)) ([5a14a01](https://github.com/trycua/cua/commit/5a14a01e43fc350dead0f5098d08958b68cff3ce))
* **spaces-macos:** install-local.sh --development for fast dev-loop builds ([#7019](https://github.com/trycua/cua/issues/7019)) ([7f9f5c2](https://github.com/trycua/cua/commit/7f9f5c20440510a999851010c00034d2afd433d1))


### Bug Fixes

* **auth:** redact tokens in Debug output ([#7071](https://github.com/trycua/cua/issues/7071)) ([#7097](https://github.com/trycua/cua/issues/7097)) ([646d5eb](https://github.com/trycua/cua/commit/646d5eb76db3ffaef409aef77986ac7d368be1a6))
* **crash:** scrub tokens before sending to Sentry ([#7098](https://github.com/trycua/cua/issues/7098)) ([0d1eb93](https://github.com/trycua/cua/commit/0d1eb93c301e6ab1a9b1492ddb1db93f11f5fe7d))
* **spaces:** dogfood fixes from 8 Nov (setup, sign-in, Cua Driver, Local Network, Home) ([#7046](https://github.com/trycua/cua/issues/7046)) ([14f9e88](https://github.com/trycua/cua/commit/14f9e8845134af1c13a71e2f57e5b18565df0320))

## [0.17.1](https://github.com/trycua/cua/compare/cua-sdk-v0.17.0...cua-sdk-v0.17.1) (2026-11-09)


### Bug Fixes

* **cluster:** GPU jobs are admitted by free video memory and run on Windows hosts ([#6943](https://github.com/trycua/cua/issues/6943)) ([c62abf3](https://github.com/trycua/cua/commit/c62abf39ba407f385701b0a82a5a4d8b8eacc426))
* **cua-volume:** a saved presence heartbeat replaces the last one, and a saved change whose bytes are gone is forgotten (fixes [#6859](https://github.com/trycua/cua/issues/6859)) ([#6864](https://github.com/trycua/cua/issues/6864)) ([2349735](https://github.com/trycua/cua/commit/2349735830d4f6e8391c97557d578374e79a4ee2))
* **spaces:** runs from a Space are confined children and a Space cannot start its own daemon ([#7055](https://github.com/trycua/cua/issues/7055)) ([8275285](https://github.com/trycua/cua/commit/82752858f6256eaf4e8eccf9033cf27ad3163cea))

## [0.17.0](https://github.com/trycua/cua/compare/cua-sdk-v0.16.0...cua-sdk-v0.17.0) (2026-11-09)


### Features

* **spaces:** a Space uses the pool, its runs are its children and are cleaned up with it ([#6945](https://github.com/trycua/cua/issues/6945)) ([a35099d](https://github.com/trycua/cua/commit/a35099d7bcc5548d9d1b86b901d20e025a0594b9))
* **spaces:** Ask first and Secure Spaces are always on, their Labs switches are gone (CUA-3551) ([#6775](https://github.com/trycua/cua/issues/6775)) ([cc67e71](https://github.com/trycua/cua/commit/cc67e715d11b7450a82663bf7e308e6680422beb))


### Bug Fixes

* **cua-agents:** an agent's log event bigger than the read window no longer stalls the reader, and the journal keeps lines small ([#7004](https://github.com/trycua/cua/issues/7004)) ([8b339c5](https://github.com/trycua/cua/commit/8b339c56feb1b03252d3e72ff7a6e3d961af856f))
* **spaces:** hold back an agent's narration of its own process in the thread ([#7027](https://github.com/trycua/cua/issues/7027)) ([fa605be](https://github.com/trycua/cua/commit/fa605be7c8dc15c7042205ae98d5a85699c36a47))

## [0.16.0](https://github.com/trycua/cua/compare/cua-sdk-v0.15.1...cua-sdk-v0.16.0) (2026-11-08)


### Features

* **spaces:** show "Local · Staging" or "Local · Prod" in the title bar (CUA-3814) ([#6962](https://github.com/trycua/cua/issues/6962)) ([17b37ef](https://github.com/trycua/cua/commit/17b37ef5775b4b06f0d1483bf0a9db6074a8d2ff))


### Bug Fixes

* **cua-agents:** say a lost provider connection in plain words, not raw http2 ([#6963](https://github.com/trycua/cua/issues/6963)) ([a2bbc28](https://github.com/trycua/cua/commit/a2bbc280957e5ca674d5ca5a0422da22be1d6cdf))
* **spaces:** Ready lists only the picked agent's chats, and shows at once (CUA-3774) ([#6916](https://github.com/trycua/cua/issues/6916)) ([4392fe0](https://github.com/trycua/cua/commit/4392fe0502790f4fdd1d9b9e5bc42c1418281146))
* **vmm:** the runtime doctor's report makes no Local Network connect, so macOS doesn't ask at launch ([#6969](https://github.com/trycua/cua/issues/6969)) ([e002d4f](https://github.com/trycua/cua/commit/e002d4f29f58a1457fe998874ba4ea2e8dbc3016))

## [0.15.1](https://github.com/trycua/cua/compare/cua-sdk-v0.15.0...cua-sdk-v0.15.1) (2026-11-08)


### Bug Fixes

* **cua:** ship the static cua-spacesd in every platform wheel ([#6959](https://github.com/trycua/cua/issues/6959)) ([16ac156](https://github.com/trycua/cua/commit/16ac1561354530107bd86cc721b044587d448f6c))

## [0.15.0](https://github.com/trycua/cua/compare/cua-sdk-v0.14.0...cua-sdk-v0.15.0) (2026-11-08)


### Features

* **spaces:** record the Ready screen, Copy, chat pickup and Done in the first-run funnel ([#6924](https://github.com/trycua/cua/issues/6924)) ([2a1b66e](https://github.com/trycua/cua/commit/2a1b66e731d446c05fa443916eaaaa6ba26c22aa))


### Bug Fixes

* **cua-vmm:** forward the agent port of a macOS Space behind the secure gateway ([#6793](https://github.com/trycua/cua/issues/6793)) ([55eaa9b](https://github.com/trycua/cua/commit/55eaa9b6c618bbe2f723ec392436ceb9e2b4e541))
* **spaces:** a slow-starting desktop no longer keeps a sleeping Space awake ([#6868](https://github.com/trycua/cua/issues/6868)) ([066f837](https://github.com/trycua/cua/commit/066f837d6e9a84483d02076dcd8a3ea5f67fe29c))
* **spaces:** start Claude agents on Sonnet 5.5 and list the Claude 5 family ([#6810](https://github.com/trycua/cua/issues/6810)) ([1511490](https://github.com/trycua/cua/commit/15114908cae2b3d70a85356651142faa1414ebf8))


### Performance Improvements

* **spaces:** connect Space desktop streams in under a second ([#6818](https://github.com/trycua/cua/issues/6818)) ([1f060b6](https://github.com/trycua/cua/commit/1f060b62491bce2fb010b64a6569bb1974decdc0))

## [0.14.0](https://github.com/trycua/cua/compare/cua-sdk-v0.13.0...cua-sdk-v0.14.0) (2026-11-07)


### Features

* **spaces:** setup says what Spaces is for, asks for the rest after the first job, offers only what this computer can make ([#6787](https://github.com/trycua/cua/issues/6787)) ([7ecb202](https://github.com/trycua/cua/commit/7ecb202e25329e41985b19b66025f9c02480e25d))


### Bug Fixes

* **spaces:** Home's apps row opens again after a relaunch and shows each switch live (CUA-3551) ([#6872](https://github.com/trycua/cua/issues/6872)) ([26858cd](https://github.com/trycua/cua/commit/26858cd5050728f64aaa1ef78a9bf3a248fdccc8))

## [0.13.0](https://github.com/trycua/cua/compare/cua-sdk-v0.12.1...cua-sdk-v0.13.0) (2026-11-07)


### Features

* **spaces:** put quiet agent home Spaces to sleep ([#6754](https://github.com/trycua/cua/issues/6754)) ([c131a07](https://github.com/trycua/cua/commit/c131a070740605de7b0d86243ddfece22ba9f88d))


### Bug Fixes

* **spaces:** every agent message is appended to the thread, child Spaces work from inside a Space, and child agents show in the chat ([#6733](https://github.com/trycua/cua/issues/6733)) ([515371a](https://github.com/trycua/cua/commit/515371a48a616aea3bba5f0ea13e0863c8a26041))
* **spaces:** say computers in use at once for the plan limit (CUA-3690) ([#6790](https://github.com/trycua/cua/issues/6790)) ([891ccb7](https://github.com/trycua/cua/commit/891ccb7153dde86f6219da719c583d2de0b88b14))

## [0.12.1](https://github.com/trycua/cua/compare/cua-sdk-v0.12.0...cua-sdk-v0.12.1) (2026-11-07)


### Bug Fixes

* **cua-spaces:** the relay-turned-off mesh test no longer waits out the reconnect spread, and CI runs cua-spaces unit tests ([#6795](https://github.com/trycua/cua/issues/6795)) ([6c52391](https://github.com/trycua/cua/commit/6c52391390d721a22e53073176edabf010cc7db9))

## [0.12.0](https://github.com/trycua/cua/compare/cua-sdk-v0.11.0...cua-sdk-v0.12.0) (2026-11-07)


### Features

* **spaces:** Local dev builds keep agent keys in a private file without the Keychain; install-local.sh signs with its own Keychain ([#6587](https://github.com/trycua/cua/issues/6587)) ([ffa4c06](https://github.com/trycua/cua/commit/ffa4c068ac246b015c13b30982b1f57086443b5c))


### Bug Fixes

* **cua-vmm:** macOS Spaces run on Lume 0.7.0, the first release that takes a network gateway ([#6767](https://github.com/trycua/cua/issues/6767)) ([b7b1ae2](https://github.com/trycua/cua/commit/b7b1ae20b78ba27ecfb7cc08779db4ceffa97f92))
* **spaces:** explain a refused new computer and offer a code from another computer ([#6710](https://github.com/trycua/cua/issues/6710)) ([327f41c](https://github.com/trycua/cua/commit/327f41c89be3e8209a4bfc3b5785cb423f97de9c))
* **spaces:** New Space Continue did nothing for a Windows image on a Windows PC ([#6724](https://github.com/trycua/cua/issues/6724)) ([2f2af69](https://github.com/trycua/cua/commit/2f2af6925c21eb372b0a2a90a3b0f1fb09d1222c))
* **spaces:** say This PC (not This Mac) for the current computer on Windows ([#6735](https://github.com/trycua/cua/issues/6735)) ([35407b5](https://github.com/trycua/cua/commit/35407b58467cfddd049547aae9f944d7e0956ded))
* **spaces:** start never pulls a moved image, pending first message for a new home, pet-name default names ([#6707](https://github.com/trycua/cua/issues/6707)) ([8ef19f2](https://github.com/trycua/cua/commit/8ef19f259a99e2b3b1153bf3e53025f730409736))

## [0.11.0](https://github.com/trycua/cua/compare/cua-sdk-v0.10.1...cua-sdk-v0.11.0) (2026-11-06)


### Features

* **cua-driver:** opt-in refusal of ungranted macOS Automation scripting ([#5777](https://github.com/trycua/cua/issues/5777)) ([8d9ec70](https://github.com/trycua/cua/commit/8d9ec709d2e2a73f6ebdaf5b5d838d2b5baf0014))
* **images:** the slim Linux image carries the cua CLI and a spawn-capable cua-spacesd; drop agent-home ([#6515](https://github.com/trycua/cua/issues/6515)) ([a1d98e8](https://github.com/trycua/cua/commit/a1d98e8ba0510b6decc63b0840f51345adae00c1))
* **relay,spacesd:** a removed computer deletes its Spaces when it next comes online (CUA-3479) ([#6532](https://github.com/trycua/cua/issues/6532)) ([e7e78c8](https://github.com/trycua/cua/commit/e7e78c8217b126e4bd02ff67ef7d733d11ec243f))
* **spaces-macos:** Cua Driver built into Cua Spaces (CUA-2771, CUA-2775) ([#6505](https://github.com/trycua/cua/issues/6505)) ([b5125d2](https://github.com/trycua/cua/commit/b5125d2eaf2463b7bbac219b19fd281420213e1a))
* **spaces:** ask first, CI runners, TCP shares, secure networks and the phone app, behind experiments ([#6355](https://github.com/trycua/cua/issues/6355)) ([8cfff8e](https://github.com/trycua/cua/commit/8cfff8ef0e4a904c1534eac44cea3d65e741e6fd))
* **spaces:** every running Space of a signed-in computer shows on your account and phone ([#6487](https://github.com/trycua/cua/issues/6487)) ([1ca3a30](https://github.com/trycua/cua/commit/1ca3a30cfebfc6fde6910bb2412f19e2ab44454c))
* **spaces:** Network picker with an "Also allow" list (localhost and local network are siblings) ([#6610](https://github.com/trycua/cua/issues/6610)) ([dff4d01](https://github.com/trycua/cua/commit/dff4d01d8e4fce3fa65c1c20264ea8b06605a599))
* **spaces:** pick up where you left off in the first run (CUA-3319) ([#6463](https://github.com/trycua/cua/issues/6463)) ([a79ecf8](https://github.com/trycua/cua/commit/a79ecf8dddf225d11367360cc2dfcf6a9c135340))
* **spaces:** Secure Spaces, child Spaces, conformance, space jobs and the macOS developer-tools image, behind experiments ([#6306](https://github.com/trycua/cua/issues/6306)) ([4715a72](https://github.com/trycua/cua/commit/4715a723a450610cdf77f688776c1fdd40660149))
* **spaces:** Spaces as a tree and a default Space, behind Labs, with the SDK bindings and CLI (CUA-3413) ([#6508](https://github.com/trycua/cua/issues/6508)) ([b6090f6](https://github.com/trycua/cua/commit/b6090f6fb2f55ba8e75a458b7dbb4fc815ef161d))
* **spaces:** Stable / Nightly update channel; betas move to Nightly (CUA-3503) ([#6548](https://github.com/trycua/cua/issues/6548)) ([53dd9d8](https://github.com/trycua/cua/commit/53dd9d868fc1c6b513257ba84f00161ddf999cc1))
* **spaces:** the new prompt box with a model and an optional repo ([#6490](https://github.com/trycua/cua/issues/6490)) ([2ea6cf3](https://github.com/trycua/cua/commit/2ea6cf34dde37afd37913566ce47ea92413c1cb3))
* **spaces:** the new setup prepares the first Space while you set up, behind Labs (CUA-2851) ([#6360](https://github.com/trycua/cua/issues/6360)) ([2829e00](https://github.com/trycua/cua/commit/2829e00b180dfb08ca5ff4cbd7c95e0a14ad3409))
* **spaces:** the warming home stays out of the top bar and off the phone until a person writes into it ([#6513](https://github.com/trycua/cua/issues/6513)) ([a7f27cc](https://github.com/trycua/cua/commit/a7f27ccecbf8c6fcb0fdb4093dcb45b3eb42940d))
* **spaces:** warm pools, a prepared Linux Space and agent harnesses ready in about one second (behind the warm-pools experiment) ([#6472](https://github.com/trycua/cua/issues/6472)) ([d592250](https://github.com/trycua/cua/commit/d592250ea79f607f948b13de8fa557f8299d673b))
* **spaces:** Windows Spaces on nested AWS KVM hosts (windows-kvm experiment, off by default) ([#6358](https://github.com/trycua/cua/issues/6358)) ([6bdfdc5](https://github.com/trycua/cua/commit/6bdfdc53972063fa1def9cc8886d627404c53ffe))
* **telemetry:** report which step a Space start failed at (CUA-3139) ([#6377](https://github.com/trycua/cua/issues/6377)) ([4d985c1](https://github.com/trycua/cua/commit/4d985c120924e3210b9fdcd60907377a74eae579))


### Bug Fixes

* **ci:** cua-auth tests never ran on GitHub; fix the broken flows tests (CUA-3363) ([#6478](https://github.com/trycua/cua/issues/6478)) ([1d18983](https://github.com/trycua/cua/commit/1d18983a470ca5e31f9b2de2b2faaf015e83deee))
* **cua-cli:** bound process start and Space lookup so a silent relay stream fails instead of hanging ([#6601](https://github.com/trycua/cua/issues/6601)) ([c6129be](https://github.com/trycua/cua/commit/c6129be4b8ed61e42eaf355bc07d81dc6e736984))
* **cua-cli:** run the spacesd mock test on an 8 MiB stack ([#6640](https://github.com/trycua/cua/issues/6640)) ([aec5fa1](https://github.com/trycua/cua/commit/aec5fa1d82f9044d8399ed751b0250c577b98501))
* **cua-compat:** build Basic auth test headers at runtime ([#6639](https://github.com/trycua/cua/issues/6639)) ([3d9c982](https://github.com/trycua/cua/commit/3d9c9828ee4459d5264dc19487ed7091e0b0a2b3))
* **cua-compat:** keep sweeping a Space a dead gateway's create makes late ([#6544](https://github.com/trycua/cua/issues/6544)) ([24bf099](https://github.com/trycua/cua/commit/24bf09959b36b289886d2a9570e9129d71b5fadd))
* **cua-compat:** name root to a guest agent that switches users itself ([#6459](https://github.com/trycua/cua/issues/6459)) ([c2b7a90](https://github.com/trycua/cua/commit/c2b7a906cac682aaa67a0ed0334f2c670641db45))
* **cua-compat:** name the user on file calls to an agent that switches users ([#6523](https://github.com/trycua/cua/issues/6523)) ([1e1b948](https://github.com/trycua/cua/commit/1e1b9489f736d00a7f841db0ea4127dd75dc682c))
* **cua-compat:** wait for cua-spacesd when a gateway creates a Space ([#6480](https://github.com/trycua/cua/issues/6480)) ([915281d](https://github.com/trycua/cua/commit/915281d210651bbce7a20726e8fad7ee1189da76))
* **cua-daemon:** give the density burst tests a vCPU limit that fits the burst ([#6474](https://github.com/trycua/cua/issues/6474)) ([9b83a6c](https://github.com/trycua/cua/commit/9b83a6c3972fadd3f21856f27fa8901dca078f02))
* **cua-daemon:** list Spaces added by address in the sandbox list ([#6414](https://github.com/trycua/cua/issues/6414)) ([7cf6874](https://github.com/trycua/cua/commit/7cf68742443419c2c7892c098473182338fc5122))
* **cua-daemon:** name the holder's pid when a second daemon is refused on Windows ([#6402](https://github.com/trycua/cua/issues/6402)) ([c4b67c4](https://github.com/trycua/cua/commit/c4b67c4a5fec7cf65e6056c6ea70f229fd469a86))
* **cua-daemon:** wait for the lock after release in the instance-lock test ([#6492](https://github.com/trycua/cua/issues/6492)) ([1618ca1](https://github.com/trycua/cua/commit/1618ca188893df1d9c77453e9186002205b9560f))
* **cua-driver:** admit any native Wayland app to Hyprland background input ([#6706](https://github.com/trycua/cua/issues/6706)) ([895aec7](https://github.com/trycua/cua/commit/895aec7e51b0409b2b22c958920d52c22b0d2b03))
* **cua-driver:** move the skill version under metadata.version ([#6647](https://github.com/trycua/cua/issues/6647)) ([0402233](https://github.com/trycua/cua/commit/0402233c44647f16e5af1bef23af5bc89e64d2ab))
* **cua-driver:** stop the Node runtime leaking a libuv handle per future poll ([#5691](https://github.com/trycua/cua/issues/5691)) ([f49571c](https://github.com/trycua/cua/commit/f49571cf3f96d4b8ced7807ca634bd4f978eda8e))
* **cua-host:** keep staging and production host services apart; Windows host online right after setup ([#6557](https://github.com/trycua/cua/issues/6557)) ([126b01e](https://github.com/trycua/cua/commit/126b01ec330951195b7275cd2b5936d4a707e2fb))
* **cua-mesh:** a client that hangs up mid-dial, a connection burst and a forward's UDP flows no longer hold descriptors (CUA-2159) ([#6475](https://github.com/trycua/cua/issues/6475)) ([2cac516](https://github.com/trycua/cua/commit/2cac516f93eb5eeb9a5734f87bc8b978782a7e33))
* **cua-mesh:** a failed dial no longer answers for a peer that came online since ([#6479](https://github.com/trycua/cua/issues/6479)) ([ebf96e5](https://github.com/trycua/cua/commit/ebf96e5ff17a12d92f67f7d32a740662a1bc435c))
* **cua-mesh:** a mesh forward keeps its UDP port when Windows excludes a UDP range ([#6418](https://github.com/trycua/cua/issues/6418)) ([d685580](https://github.com/trycua/cua/commit/d6855806e6eebfa9813b91986f59931e7f103a51))
* **cua-mesh:** the NAT rebinding test accepts either of the client's new ports (CUA-3277) ([#6509](https://github.com/trycua/cua/issues/6509)) ([f02a9a0](https://github.com/trycua/cua/commit/f02a9a061c99dbc8aa5afeeb6630d5677d6902b3))
* **cua-spacesd-client:** mock runs commands and files as the user they name ([#6527](https://github.com/trycua/cua/issues/6527)) ([f294774](https://github.com/trycua/cua/commit/f2947748770bc9e3215b8417965d6193dddfbfe1))
* **cua-spaces:** publish running Spaces on the relay after a late cua-spacesd ([#6581](https://github.com/trycua/cua/issues/6581)) ([2ee2221](https://github.com/trycua/cua/commit/2ee22216370dafd161436e18364dd4c5cf26b2d7))
* **cua-spaces:** retire interrupted-create journals whose sandbox is gone ([#6650](https://github.com/trycua/cua/issues/6650)) ([1bb7e7f](https://github.com/trycua/cua/commit/1bb7e7f2b54e839784a090f374e37f7a933d0c0f))
* **cua-spaces:** save relay-reported.json off the connect path ([#6456](https://github.com/trycua/cua/issues/6456)) ([2acdbd9](https://github.com/trycua/cua/commit/2acdbd9f690c35f7848f8232d7982a82f71ff603))
* **cua-volume:** a store being created by a clone is no longer taken for removed ([#6496](https://github.com/trycua/cua/issues/6496)) ([cbc348f](https://github.com/trycua/cua/commit/cbc348fba9a70addd1634b2ff56beb034d6106a8))
* **cua-volume:** sync an installed version through a writable handle on Windows ([#6455](https://github.com/trycua/cua/issues/6455)) ([38d3929](https://github.com/trycua/cua/commit/38d39292a4534d930ecb683b8aa4f4f699ee8576))
* **cua:** split the rest of create runtime errors and explain the macOS VM limit (CUA-1356) ([#5243](https://github.com/trycua/cua/issues/5243)) ([3a4ec34](https://github.com/trycua/cua/commit/3a4ec34a77ee607be3d6460b7ea0d44bccaf08d6))
* **spaces:** a computer keeps one identity when re-enabled, renamed or removed (CUA-3316, CUA-3315) ([#6486](https://github.com/trycua/cua/issues/6486)) ([2163887](https://github.com/trycua/cua/commit/21638877b566df572f7d1f728086f0c09762b46f))
* **spaces:** a new message and an agent's tool calls no longer wait on a slow Space ([#6619](https://github.com/trycua/cua/issues/6619)) ([b2e8df8](https://github.com/trycua/cua/commit/b2e8df864277ec93098acf429eafbab3560a2794))
* **spaces:** a sign-in during the first run never fails silently (CUA-3144) ([#6408](https://github.com/trycua/cua/issues/6408)) ([7885b58](https://github.com/trycua/cua/commit/7885b58da7df43951382b6365e6533f7639ce341))
* **spaces:** Cua Driver runs after the new first run, whatever the Mac shares (CUA-3485) ([#6534](https://github.com/trycua/cua/issues/6534)) ([97eabde](https://github.com/trycua/cua/commit/97eabde88dd1fdf4548dbe2d12af8396211ff4ff))
* **spacesd:** volume attach no longer deadlocks a 1 vCPU Space ([#6565](https://github.com/trycua/cua/issues/6565)) ([8a708ab](https://github.com/trycua/cua/commit/8a708abf6a60b76894df8d95ff94450436a3032f))
* **spaces:** open picked-up agent chats from the end of their file, never read them whole (CUA-3548) ([#6577](https://github.com/trycua/cua/issues/6577)) ([d152839](https://github.com/trycua/cua/commit/d152839d00991547e59fa8b63cd77f3a6858cf7a))
* **spaces:** reach macOS VMs over vsock, not Local Network or VPN routes (CUA-3139) ([#6380](https://github.com/trycua/cua/issues/6380)) ([37af9df](https://github.com/trycua/cua/commit/37af9dffdd0a79d9bcbcee0667832272b556435a))
* **spaces:** say why a running Space is not on your account ([#6536](https://github.com/trycua/cua/issues/6536)) ([84a1353](https://github.com/trycua/cua/commit/84a1353ca5ee43c5c3f1ab8ef9b9b827453d5480))
* **spaces:** say why a warm-pool refill waits, and end the agents of a deleted Space ([#6632](https://github.com/trycua/cua/issues/6632)) ([1c4cfba](https://github.com/trycua/cua/commit/1c4cfba90f013998e983a64dbaeaa5244c707977))
* **spaces:** signing in to Claude Code, Codex and Hermes from the chat says what happened ([#6576](https://github.com/trycua/cua/issues/6576)) ([0c5e26f](https://github.com/trycua/cua/commit/0c5e26f86e16809afee3c5d5c9ddd0e8925f0585))
* **spaces:** three first-run fixes for the new setup (CUA-3193) ([#6409](https://github.com/trycua/cua/issues/6409)) ([a6005a4](https://github.com/trycua/cua/commit/a6005a4a490e4cfbc5596c419ce4d0209a1be4d7))
* **windows:** no firewall prompt for cua and cua-spacesd, and a real publisher name ([#6558](https://github.com/trycua/cua/issues/6558)) ([0d196ad](https://github.com/trycua/cua/commit/0d196ad7669848951b7f5fa336af01e3f571d1f8))


### Performance Improvements

* **pool:** a home claims the warm Space that serves it ([#6566](https://github.com/trycua/cua/issues/6566)) ([4316dc7](https://github.com/trycua/cua/commit/4316dc7cb193d1cc4009e3c9457f85a95938b176))
* **pool:** a warm member is prepared for every harness the app offers ([#6611](https://github.com/trycua/cua/issues/6611)) ([6dd499f](https://github.com/trycua/cua/commit/6dd499f5561df29c901e4255bff1377be8c171f4))
* **spaces:** a new home is warm as soon as the person's agents are prepared ([#6645](https://github.com/trycua/cua/issues/6645)) ([397c66c](https://github.com/trycua/cua/commit/397c66c05e97bb8d4b85918d66e600b11079e313))

## [0.10.1](https://github.com/trycua/cua/compare/cua-sdk-v0.10.0...cua-sdk-v0.10.1) (2026-11-02)


### Bug Fixes

* **cua-sdk:** a restarted microVM comes back, restart waits for the guest, and Apple's Containerization pulls private images ([#6308](https://github.com/trycua/cua/issues/6308)) ([f48a905](https://github.com/trycua/cua/commit/f48a905ca688303bac07a4134784a116b928a042))
* **cua-sdk:** the QEMU monitor listens on a private socket and the display asks for a password ([#6301](https://github.com/trycua/cua/issues/6301)) ([504c0a4](https://github.com/trycua/cua/commit/504c0a4828d2687905cd7b82fbc9e55207e35e86))
* **cua-spacesd:** a file write on the Cua Volume mount no longer fails after the file is in place ([#6322](https://github.com/trycua/cua/issues/6322)) ([8c55aa4](https://github.com/trycua/cua/commit/8c55aa4ca8b998c0198625b4df517c5987ecfb0e))
* **e2e:** Python/TS fork-batch no longer writes to the real ~/.cua; batched forks keep their first error (CUA-3092) ([#6312](https://github.com/trycua/cua/issues/6312)) ([662432d](https://github.com/trycua/cua/commit/662432d9ecdd58941c99b3e65ee0e482cc9275f2))
* **spaces:** plain message when a Mac cannot run Spaces, and first-run fixes (CUA-3063) ([#6323](https://github.com/trycua/cua/issues/6323)) ([7f3b4dd](https://github.com/trycua/cua/commit/7f3b4dd742d128bbd867ca20974ef08b69ccbdd9))

## [0.10.0](https://github.com/trycua/cua/compare/cua-sdk-v0.9.0...cua-sdk-v0.10.0) (2026-11-02)


### Features

* **cli:** cua machines rm removes a computer that is gone ([#6092](https://github.com/trycua/cua/issues/6092)) ([6660c7d](https://github.com/trycua/cua/commit/6660c7d56ba73649bfd4018637f69396e956bfaf))
* **cluster:** place durable Spaces on the most reliable machine ([#6279](https://github.com/trycua/cua/issues/6279)) ([ed887bb](https://github.com/trycua/cua/commit/ed887bb55f0eb27491b061764d679fb086e5a583))
* **connections:** one Connections list driven by a provider registry ([#6134](https://github.com/trycua/cua/issues/6134)) ([74ff486](https://github.com/trycua/cua/commit/74ff486506e5e203b3a6fd371931ed717a405dba))
* **connections:** per-provider model lists and a connectors.models op ([#6173](https://github.com/trycua/cua/issues/6173)) ([1976208](https://github.com/trycua/cua/commit/19762088609eeac34becee1848459ed3da75019a))
* **cua-cli:** cua spaces ls tags this computer's own row "(this computer)" ([#6112](https://github.com/trycua/cua/issues/6112)) ([ded0c17](https://github.com/trycua/cua/commit/ded0c1748619687613e19051ae60453bdbf700aa))
* **cua-cli:** one --share desktop|spaces|both|none flag for cua host setup ([#6096](https://github.com/trycua/cua/issues/6096)) ([903d8c5](https://github.com/trycua/cua/commit/903d8c5b7d942faa632cabdade4145af9f368ca7))
* **cua-vmm:** support Windows 11 ARM64 guests with an automatic swtpm TPM ([#5506](https://github.com/trycua/cua/issues/5506)) ([56725dd](https://github.com/trycua/cua/commit/56725dd7e47dc62bc13ab8229f9e242a0d4467d5))
* **mesh:** report each machine's path and pin it, with plain reasons ([#6278](https://github.com/trycua/cua/issues/6278)) ([4ece1e9](https://github.com/trycua/cua/commit/4ece1e9161aa8265b567a8f97a2f034b8985ab75))
* **relay:** keep path mode beside a host domain, send old share links to their origin ([#6061](https://github.com/trycua/cua/issues/6061)) ([8ed5bd3](https://github.com/trycua/cua/commit/8ed5bd3c5d041cb24428e57bceef7f5224b11374))
* **settings:** Providers section as compact rows, per-app Uses switcher, bundled provider marks ([#6170](https://github.com/trycua/cua/issues/6170)) ([34f12df](https://github.com/trycua/cua/commit/34f12df58fc0d9f58c0a4134f0dfaf3424ceb3fc))
* **spaces:** add a machine by address from New Space's arrow, system order, short Disk ([#6208](https://github.com/trycua/cua/issues/6208)) ([0e2a7b0](https://github.com/trycua/cua/commit/0e2a7b0f7d3cd76354edf32ad06fe4417f6f87c7))
* **spaces:** contextual Pro and Teams limit cards behind the upsell_cards flag ([#5952](https://github.com/trycua/cua/issues/5952)) ([e3d0e52](https://github.com/trycua/cua/commit/e3d0e5291e56b339a981a31a269c3bd00553abc6))
* **spaces:** drop the line under Kind; MicroVM status in Settings, Advanced (CUA-2446) ([#6048](https://github.com/trycua/cua/issues/6048)) ([a87d74c](https://github.com/trycua/cua/commit/a87d74c401778bff4a5371cbab0c751cbe8a14dc))
* **spaces:** keep the Volume on all your machines, started by the daemon ([#6286](https://github.com/trycua/cua/issues/6286)) ([e6a88bf](https://github.com/trycua/cua/commit/e6a88bf12766b6ab85e4ab51aac6e5007588e747))
* **spaces:** link System Status from Help, Settings About and the troubleshoot guide (CUA-2228) ([#5902](https://github.com/trycua/cua/issues/5902)) ([aab2775](https://github.com/trycua/cua/commit/aab27757205bf1dba41e9ea4d91df61a22b92764))
* **spaces:** list signed-in devices under "Your devices" on the Machines page ([#6085](https://github.com/trycua/cua/issues/6085)) ([9758f85](https://github.com/trycua/cua/commit/9758f8518b6e6b37a7bfb5d23c97378319046c0f))
* **spaces:** onboarding Back on every step, and a try request per system ([#5885](https://github.com/trycua/cua/issues/5885)) ([f59e242](https://github.com/trycua/cua/commit/f59e242271cd211a24329d48bb79e05eeb89f699))
* **spaces:** onboarding v2 PostHog funnel events (CUA-2292) ([#5951](https://github.com/trycua/cua/issues/5951)) ([feafec6](https://github.com/trycua/cua/commit/feafec648a406d0cffba14c0b4d84aaa29662f86))
* **spaces:** pin Spaces and shared machine desktops to the notch ([#6163](https://github.com/trycua/cua/issues/6163)) ([b269d38](https://github.com/trycua/cua/commit/b269d38db580afbd7b9f3b1d8409d0df540e1b21))
* **spaces:** Sentry crash reporting for the desktop app + crash gate in auto rollout (CUA-2343) ([#6011](https://github.com/trycua/cua/issues/6011)) ([d991e3b](https://github.com/trycua/cua/commit/d991e3b4cc229881a6b5a3018956668a14b61e5b))
* **spaces:** show a Space another computer is still creating (CUA-2349) ([#6010](https://github.com/trycua/cua/issues/6010)) ([7b24a2c](https://github.com/trycua/cua/commit/7b24a2c61546bf38d9cfdcd11c1aa66e1377bfe6))
* **spaces:** Stop frees a local VM's memory; cua spaces suspend and resize ([#5912](https://github.com/trycua/cua/issues/5912)) ([ff40ec3](https://github.com/trycua/cua/commit/ff40ec3e2c8aacc77d67e60f8dc3e1eeae43f247))
* **spaces:** talk to agents without typing @ every time ([#6135](https://github.com/trycua/cua/issues/6135)) ([19c1f22](https://github.com/trycua/cua/commit/19c1f22cbeb09e8f133cdde8d499606c36b5cb52))
* **telemetry:** report how durable agents behave in the field ([#6207](https://github.com/trycua/cua/issues/6207)) ([514b2a8](https://github.com/trycua/cua/commit/514b2a8f0c0cc38acd0fd2c23dff5931514ed9b9))
* **volume:** replicate the Volume across the account's machines ([#6280](https://github.com/trycua/cua/issues/6280)) ([74657cf](https://github.com/trycua/cua/commit/74657cf5a9b44ab73127c2b1fd29df3e28f35bd6))


### Bug Fixes

* **agents:** a durable drive keeps its journal across a driver restart on the same machine ([#6228](https://github.com/trycua/cua/issues/6228)) ([7509a74](https://github.com/trycua/cua/commit/7509a74060fc144b0c49103010ca38ea7c8b2db8))
* **agents:** a durable harness driver reserves 1 CPU and 1.5 GiB, so four run on 8 GB ([#6245](https://github.com/trycua/cua/issues/6245)) ([93adeae](https://github.com/trycua/cua/commit/93adeaef137e39536cbd49b44bd6009601076694))
* **agents:** a durable Hermes drive that lost its session opens a new one instead of ending the task ([#6234](https://github.com/trycua/cua/issues/6234)) ([e101c99](https://github.com/trycua/cua/commit/e101c9956b944b0afb5b289cc28ab13f8e0669f6))
* **agents:** a durable OpenCode call that asks permission twice is approved, not refused as a repeat ([#6235](https://github.com/trycua/cua/issues/6235)) ([2b7bd1b](https://github.com/trycua/cua/commit/2b7bd1b9046b63079f684810e8729a84f967a128))
* **agents:** a durable Pi agent runs through the durable ACP driver ([#6242](https://github.com/trycua/cua/issues/6242)) ([7c68907](https://github.com/trycua/cua/commit/7c689079d238d507927ae86282b191c8a26478c7))
* **agents:** an idle agent run on the Cua Volume starts again, and a start that fails says what is missing (CUA-2830) ([#6174](https://github.com/trycua/cua/issues/6174)) ([1e8a808](https://github.com/trycua/cua/commit/1e8a8082fdfdbe28e94fa0567018a9f51b1cd3ec))
* **agents:** Codex persistent agent keeps its SQLite state off the Volume (CUA-2856) ([#6179](https://github.com/trycua/cua/issues/6179)) ([0d278d9](https://github.com/trycua/cua/commit/0d278d9d1ef339140f441f6ffb3638e2f738c04c))
* **agents:** cua agent logs session:NAME shows what the agent did ([#6243](https://github.com/trycua/cua/issues/6243)) ([414ab83](https://github.com/trycua/cua/commit/414ab83b3a44068cc350bed9fea08689305f574f))
* **agents:** durable agents start again: give the harness an absolute working folder ([#6220](https://github.com/trycua/cua/issues/6220)) ([cba650d](https://github.com/trycua/cua/commit/cba650dab5add68c4cf770e45437e732a3ddb818))
* **agents:** import the Claude Code sign-in instead of running claude setup-token ([#6136](https://github.com/trycua/cua/issues/6136)) ([08a2f0d](https://github.com/trycua/cua/commit/08a2f0df1cf2ac61014b8c49c700fc5c7a277cb9))
* **agents:** install Hermes in a job Space without git, and fail a permanent install error at once ([#6229](https://github.com/trycua/cua/issues/6229)) ([6e029a9](https://github.com/trycua/cua/commit/6e029a92cfaa2bc87ef11e229536cdb1d95f9546))
* **agents:** plain words for how many drives a durable agent used and why one failed ([#6247](https://github.com/trycua/cua/issues/6247)) ([321938c](https://github.com/trycua/cua/commit/321938ca5b143aba0aea83765bebb0ca3527d9ce))
* **agent:** surface rejected model keys and silent runs; refuse --wire without --base-url (CUA-3069) ([#6293](https://github.com/trycua/cua/issues/6293)) ([f595a4f](https://github.com/trycua/cua/commit/f595a4fb6f524e555e5e422aab81c3457fe4a713))
* **cli:** cua agent add --durable takes --max-attempts and --stall-secs ([#6241](https://github.com/trycua/cua/issues/6241)) ([7e94c85](https://github.com/trycua/cua/commit/7e94c857ae9e812f540896adb40272e7ae89dc7f))
* **cli:** give a durable harness driver a Space big enough to install and run the harness ([#6222](https://github.com/trycua/cua/issues/6222)) ([0f31704](https://github.com/trycua/cua/commit/0f317041321fe7ce040daa29c4afe53f35c8818d))
* **cli:** stream stdin into remote Spaces in large batches, retry would-block reads (CUA-2188) ([#5886](https://github.com/trycua/cua/issues/5886)) ([d6ee37b](https://github.com/trycua/cua/commit/d6ee37b4fb7bed81f6c495e1a2a2a21d2298d015))
* **cli:** warn when a durable agent can move to a machine that does not see its files ([#6246](https://github.com/trycua/cua/issues/6246)) ([b1b5679](https://github.com/trycua/cua/commit/b1b567975bab08536cad32fe7d1f3eba14d21b08))
* **connections:** clearer Connect sheet header, per-app provider status, why only a key ([#6161](https://github.com/trycua/cua/issues/6161)) ([f4b7649](https://github.com/trycua/cua/commit/f4b76494e0dcfe18bfc35415c7454525458cda80))
* **cua-cli:** Cua Spaces Local's hints and errors name cua-local, not cua ([#5892](https://github.com/trycua/cua/issues/5892)) ([5309f1e](https://github.com/trycua/cua/commit/5309f1edd49be0edd06a6f0a91c5a171f3f29be2))
* **cua-cli:** Cua Spaces uninstall leaves Cua Driver's agent entries alone (CUA-2223) ([#5891](https://github.com/trycua/cua/issues/5891)) ([d68aaf2](https://github.com/trycua/cua/commit/d68aaf216d2db09acb4c4c08aa5dd35d66591c81))
* **cua-host:** warn when host setup falls back to a plain process ([#6087](https://github.com/trycua/cua/issues/6087)) ([d6fde99](https://github.com/trycua/cua/commit/d6fde9926e10dc528c516599e9a3b414e5ca34f7))
* **cua-sdk:** `cua runtime setup` with no components no longer exits 2 on "managed" ([#6283](https://github.com/trycua/cua/issues/6283)) ([aa52b7d](https://github.com/trycua/cua/commit/aa52b7d67a964ed5f1710dbb43da1c7cf8551410))
* **cua-sdk:** a skill installed another way no longer leaves an agent "not set up" ([#6282](https://github.com/trycua/cua/issues/6282)) ([9cad89e](https://github.com/trycua/cua/commit/9cad89e24d4fde039c001256286b79ac9592a523))
* **cua-sdk:** keep no history for a device's presence record in the local Volume store ([#6290](https://github.com/trycua/cua/issues/6290)) ([173eddf](https://github.com/trycua/cua/commit/173eddfdaabeed0c0dd98966e2119930a62e10b4))
* **cua-vmm:** count a fork's checkpoint RAM once when it is the first to map it ([#5890](https://github.com/trycua/cua/issues/5890)) ([c961059](https://github.com/trycua/cua/commit/c96105986c391a43ed07a721e12bd5a030c9c687))
* **cua-vmm:** count a recording VM's RAM file once; say when one VM is bigger than the cap ([#5877](https://github.com/trycua/cua/issues/5877)) ([1382b4f](https://github.com/trycua/cua/commit/1382b4fecfa2263d1bef0e1522f8a888e62a2c6d))
* **cua:** Spaces created on a host use its runtime; clear runtime errors; the daemon stops on SIGTERM ([#6013](https://github.com/trycua/cua/issues/6013)) ([3dea77a](https://github.com/trycua/cua/commit/3dea77a7a58ba3625740895a2cd99a857a50d860))
* **cua:** the daemon ignores caller headers from loopback clients ([#5887](https://github.com/trycua/cua/issues/5887)) ([b7d1b29](https://github.com/trycua/cua/commit/b7d1b29944aff4a20af4c3c088e3982cd2c03cd0))
* **host:** sign-in as another account enrolls this computer with a new key (CUA-2293) ([#5955](https://github.com/trycua/cua/issues/5955)) ([62d1dc3](https://github.com/trycua/cua/commit/62d1dc3779c66655b8669d85dc5d63b7035d683a))
* **host:** Windows setup no longer fails at download when the host service is running ([#5949](https://github.com/trycua/cua/issues/5949)) ([58f22b9](https://github.com/trycua/cua/commit/58f22b9cd8f26807916abb65fb99880fdf4ab25b))
* **mesh:** honour 429 Retry-After, send a client version, retire a taken-over identity; relay holds refused connects (CUA-2362) ([#6016](https://github.com/trycua/cua/issues/6016)) ([87650e1](https://github.com/trycua/cua/commit/87650e144354783f0d9cee17f48e4f91bd2fdd41))
* **pool:** a coordinator's own 404 is an answer, not "this relay has no pool API" ([#6236](https://github.com/trycua/cua/issues/6236)) ([a9902d2](https://github.com/trycua/cua/commit/a9902d272655b65180fa713a8b1f2f1c1935e70b))
* **spaces-macos:** run cua-spacesd from Contents/Helpers so opening the app shows a window ([#5871](https://github.com/trycua/cua/issues/5871)) ([92c4cbe](https://github.com/trycua/cua/commit/92c4cbe2241190b5f5a1488ba2a94859e7e51829))
* **spaces-macos:** uninstall-local.sh takes this Mac off the relay and Local's entries out of your coding agents (CUA-2212, CUA-2222) ([#5882](https://github.com/trycua/cua/issues/5882)) ([b43f30b](https://github.com/trycua/cua/commit/b43f30bd667ebba0c188f6cce541124c3fd5703d))
* **spaces:** "Skip for now" sticks across launches, and Local's agent setup names are guarded (CUA-2221) ([#5889](https://github.com/trycua/cua/issues/5889)) ([59cf287](https://github.com/trycua/cua/commit/59cf2877722c49671ebf7b697cec706fa71ef00f))
* **spaces:** a damaged settings file is kept as settings.json.damaged ([#6066](https://github.com/trycua/cua/issues/6066)) ([92706b2](https://github.com/trycua/cua/commit/92706b2a01c0558a5ec34cee55aa4995cb76fb05))
* **spaces:** a durable driver whose Space container is killed is noticed in seconds, not by the stall limit ([#6233](https://github.com/trycua/cua/issues/6233)) ([0aebcf2](https://github.com/trycua/cua/commit/0aebcf2b4a0a94fbb6f87a99fb4844c63e5c9cdc))
* **spaces:** a macOS create on a Mac nobody is at fails fast with the no-screen fix (CUA-2090) ([#5756](https://github.com/trycua/cua/issues/5756)) ([788a575](https://github.com/trycua/cua/commit/788a575dc9929cecd7f9a25722334e1d4346eeb1))
* **spaces:** a Space placed in your cloud by auto or overflow keeps its network rules or is refused ([#6070](https://github.com/trycua/cua/issues/6070)) ([dec726e](https://github.com/trycua/cua/commit/dec726eaa63a1efcb4e9f7b6f31f6810c7e8f74d))
* **spaces:** calmer onboarding layout and clearer words (CUA-2284) ([#5935](https://github.com/trycua/cua/issues/5935)) ([05f5dc8](https://github.com/trycua/cua/commit/05f5dc80e922d5fba1f8596b01e0670021b6fdc5))
* **spaces:** Cua Spaces Local uses the bundle id com.trycua.spaces.macos.local ([#5884](https://github.com/trycua/cua/issues/5884)) ([152a565](https://github.com/trycua/cua/commit/152a5653b482cbfbd01520343d0f6243f1fca467))
* **spaces:** dogfooding polish (welcome copy first) ([#5893](https://github.com/trycua/cua/issues/5893)) ([928fe85](https://github.com/trycua/cua/commit/928fe850d48b3a9d591ba7b18e93fd4f8db239a1))
* **spacesd:** teleport lands a Chrome session in one profile and launches the guest's Chromium (CUA-2853) ([#6180](https://github.com/trycua/cua/issues/6180)) ([dbf08f7](https://github.com/trycua/cua/commit/dbf08f753422fad92a6eef0093dfe5a5dd95f1f7))
* **spaces:** honest numbers in a Space's resources dialog ([#6058](https://github.com/trycua/cua/issues/6058)) ([9c7d8b5](https://github.com/trycua/cua/commit/9c7d8b5c4a4e945f0c2c740166a02787b13ba03f))
* **spaces:** mesh reconnects back off with jitter instead of looping (CUA-2276) ([#5932](https://github.com/trycua/cua/issues/5932)) ([b126954](https://github.com/trycua/cua/commit/b12695426c82738c227a6f1d7116617c17dcad14))
* **spaces:** never show an empty Spaces page while computers are still answering ([#5905](https://github.com/trycua/cua/issues/5905)) ([a0f98b1](https://github.com/trycua/cua/commit/a0f98b1a35dcce9aac11653efab82a9972484aad))
* **spaces:** one teleport setup sheet and a profile picker (CUA-2855, CUA-2853) ([#6194](https://github.com/trycua/cua/issues/6194)) ([2d974f1](https://github.com/trycua/cua/commit/2d974f1843c7343a70affba9d21e7362979edec3))
* **spaces:** remove the job Spaces a daemon restart leaves behind ([#6244](https://github.com/trycua/cua/issues/6244)) ([d0739a8](https://github.com/trycua/cua/commit/d0739a8c3fe2f2445b6e953f847ecd0f1467c52e))
* **spaces:** retry 'too many requests' and say so in plain words (CUA-2275) ([#5929](https://github.com/trycua/cua/issues/5929)) ([c421b1c](https://github.com/trycua/cua/commit/c421b1caa948c3f54f72425d70c0891ac08074d0))
* **spaces:** show a machine's shared desktop in its Machines row instead of "No Spaces" ([#6084](https://github.com/trycua/cua/issues/6084)) ([d537387](https://github.com/trycua/cua/commit/d5373879e9e0baf5a63999ac4d26902340c0ce20))
* **spaces:** show running Spaces first in the notch, never cut them for stopped ones ([#6215](https://github.com/trycua/cua/issues/6215)) ([a68e2c9](https://github.com/trycua/cua/commit/a68e2c986eb29cf70c38dcde1f9fa961af3144e6))
* **spaces:** sign-in that never comes back offers a code (CUA-2291) ([#5950](https://github.com/trycua/cua/issues/5950)) ([3cc240d](https://github.com/trycua/cua/commit/3cc240d9f2f2c3aefc670fc20ee915f5ec96372b))
* **spaces:** signing in as another account recovers the daemon session, host registration and mesh key (CUA-2295) ([#5961](https://github.com/trycua/cua/issues/5961)) ([3c0c264](https://github.com/trycua/cua/commit/3c0c26456d176672b5ce6880e0f80eac3b51c00e))
* **spaces:** Start an agent here uses real harness ids, teleport honors --profile (CUA-2857, CUA-2853) ([#6178](https://github.com/trycua/cua/issues/6178)) ([23c168b](https://github.com/trycua/cua/commit/23c168b06aae11e7ab1ef2463e314e73d14323f7))
* **spaces:** stop re-attaching the Cua Volume of a Space that stopped on its own (CUA-1759) ([#5293](https://github.com/trycua/cua/issues/5293)) ([be8490a](https://github.com/trycua/cua/commit/be8490ae49383f66d22e164580d7b5ce2bb06ccf))
* **spaces:** stop the pool re-adding a direct host under a new name every 30 s ([#6206](https://github.com/trycua/cua/issues/6206)) ([cd395c3](https://github.com/trycua/cua/commit/cd395c30df3ce1b55c1198082fa1f3411fe945ca))
* **spaces:** tell a Windows PC without QEMU before a virtual computer create (CUA-2089) ([#5755](https://github.com/trycua/cua/issues/5755)) ([1b99a27](https://github.com/trycua/cua/commit/1b99a27ee41af5bc2d8d05d555c53992132d9a7e))
* **spaces:** tell VPN users how to keep their VPN on when it blocks a macOS Space ([#6262](https://github.com/trycua/cua/issues/6262)) ([29c1f75](https://github.com/trycua/cua/commit/29c1f756971f4f4d2276c8bacad8d309395705d9))
* **spaces:** the notch counts only running or starting Spaces and collapses at zero ([#5872](https://github.com/trycua/cua/issues/5872)) ([3a31781](https://github.com/trycua/cua/commit/3a317816793479208da4c7b138680f9a438de6d6))
* **teams:** say a device waits for approval instead of "still starting" (CUA-2835) ([#6168](https://github.com/trycua/cua/issues/6168)) ([8e314e4](https://github.com/trycua/cua/commit/8e314e40971804663868359c89cda7f7cfc80b65))
* **teleport:** push --url reads in the local daemon with Keyvault consent (CUA-2854) ([#6193](https://github.com/trycua/cua/issues/6193)) ([00d7a30](https://github.com/trycua/cua/commit/00d7a3015c0ce940cbeb68c663b47505f6c8c5a0))
* **teleport:** say what macOS permission is missing before a teleport fails (CUA-2854) ([#6183](https://github.com/trycua/cua/issues/6183)) ([f4c3516](https://github.com/trycua/cua/commit/f4c3516d6b9e67daf9577478ef1e49875184ec84))


### Performance Improvements

* **cua-mesh:** an offline or unresponsive computer no longer holds up the others ([#5907](https://github.com/trycua/cua/issues/5907)) ([3a4b0aa](https://github.com/trycua/cua/commit/3a4b0aa68fb9b434538961121332c446249180d9))
* **cua-spaces:** dial a slow computer once, not once a poll ([#5908](https://github.com/trycua/cua/issues/5908)) ([45a0cb1](https://github.com/trycua/cua/commit/45a0cb1106ef6a71181d7bad0d287d2c5d69960a))
* **spaces:** a Space starting or stopping on another computer shows within a second, pushed by the daemon ([#5915](https://github.com/trycua/cua/issues/5915)) ([74a6057](https://github.com/trycua/cua/commit/74a6057dba1bbb04ff624ef92685366307f79e2d))
* **spaces:** fill in the Agents page as Spaces answer, bound slow machines ([#6177](https://github.com/trycua/cua/issues/6177)) ([886179f](https://github.com/trycua/cua/commit/886179f87f2610d143de9e0a24012dbb80704126))
* **spaces:** reconnect to a Space's desktop in under a second (CUA-2850) ([#6176](https://github.com/trycua/cua/issues/6176)) ([8aa59c7](https://github.com/trycua/cua/commit/8aa59c7eb48b67c0cd8e363cbdef6a9219db9b4b))

## [0.9.0](https://github.com/trycua/cua/compare/cua-sdk-v0.8.0...cua-sdk-v0.9.0) (2026-10-24)


### Features

* **cua-home:** CuaChannel, the names that keep Cua Spaces Local apart from stable (CUA-1776) ([#5800](https://github.com/trycua/cua/issues/5800)) ([e896967](https://github.com/trycua/cua/commit/e896967501382d6dc26ef6d96282c0f8af882648))
* **spaces:** agent-first onboarding and a quieter Home ([#5869](https://github.com/trycua/cua/issues/5869)) ([be2dad3](https://github.com/trycua/cua/commit/be2dad3345b53b6195192505a63db0538fcc035d))


### Bug Fixes

* **cua-sdk:** ship the cua CLI as its own cua-cli wheel, guard wheel size ([#5852](https://github.com/trycua/cua/issues/5852)) ([63b54fd](https://github.com/trycua/cua/commit/63b54fd55ddb3769c41b043a364517d4f60eb30d))
* **cua-sdk:** strip release binaries and allow publishing without PyPI ([#5847](https://github.com/trycua/cua/issues/5847)) ([6883d6a](https://github.com/trycua/cua/commit/6883d6aadb983cb5295e6d7c06632a30e5a0c562))
* **cua:** guest volume paths are POSIX on every host; Windows test fixes ([#5860](https://github.com/trycua/cua/issues/5860)) ([7ce3324](https://github.com/trycua/cua/commit/7ce332441a1b07e797b6cc7c02fefecc938a9ea7))
* **spaces-app:** host setup follows CUA_ENV for the relay (CUA-1610) ([#5241](https://github.com/trycua/cua/issues/5241)) ([601963f](https://github.com/trycua/cua/commit/601963fc09589ca415b2eba56e194e173b44a0dd))

## [0.8.0](https://github.com/trycua/cua/compare/cua-sdk-v0.7.0...cua-sdk-v0.8.0) (2026-10-24)


### Features

* compat gateways for E2B and Daytona, Hetzner as your own cloud, the elastic pool, cua share and the public relay ([#5778](https://github.com/trycua/cua/issues/5778)) ([d4f3f80](https://github.com/trycua/cua/commit/d4f3f8063d7b30a3b3bda2b4c05c565a9dfcb460))
* **spaces-macos:** run the host service as the Cua Spaces app itself, one name in System Settings (CUA-1885) ([#5494](https://github.com/trycua/cua/issues/5494)) ([cd134c2](https://github.com/trycua/cua/commit/cd134c2431ee9c88118080b5a40a0b120eaaecd1))
* **spaces:** onboarding agents step: pick any agents or none, per computer, with Codex's ChatGPT sign-in (CUA-1995) ([#5707](https://github.com/trycua/cua/issues/5707)) ([4a44f4d](https://github.com/trycua/cua/commit/4a44f4d39677c2d906d588b5b205a85aa8353354))


### Bug Fixes

* **cua:** stabilize the known flaky release-gate tests ([#5538](https://github.com/trycua/cua/issues/5538)) ([c3b557e](https://github.com/trycua/cua/commit/c3b557e1dced1238bf5b479af876aad9ea409498))
* **spaces-macos:** newer-Cua note with a real action, shared desktop listed first, bundle new cua in Cua Spaces (CUA-1989) ([#5702](https://github.com/trycua/cua/issues/5702)) ([b131466](https://github.com/trycua/cua/commit/b131466103eccede0c65213a59ef2d082813fb9e))
* **spaces:** recognize configured Claude Code without PATH evidence ([#5749](https://github.com/trycua/cua/issues/5749)) ([4ecb58e](https://github.com/trycua/cua/commit/4ecb58e45b9a9af3e2235d44e2ac57c79d502a98))
* **spaces:** show every sign-in state, with a reason and Try again ([#5708](https://github.com/trycua/cua/issues/5708)) ([c2a21e7](https://github.com/trycua/cua/commit/c2a21e764e2cc8b3a0a187aa9ca7d688265fe389))
* **spaces:** this Mac's own desktop says why instead of loading forever (CUA-2072) ([#5735](https://github.com/trycua/cua/issues/5735)) ([eac64d7](https://github.com/trycua/cua/commit/eac64d72828a33c82be4068a7119edf1fa09d2a4))

## [0.7.0](https://github.com/trycua/cua/compare/cua-sdk-v0.6.0...cua-sdk-v0.7.0) (2026-10-21)


### Features

* **mesh:** the mesh is on by default for every client (CUA-1947) ([#5630](https://github.com/trycua/cua/issues/5630)) ([e5d09e4](https://github.com/trycua/cua/commit/e5d09e40291a080124da6469b3c7023e02e229fd))
* **sdk:** cua sb exec -i streams a command's stdio into a Space (CUA-1949) ([#5629](https://github.com/trycua/cua/issues/5629)) ([cd50985](https://github.com/trycua/cua/commit/cd509859675ec2e16ad8509eb484f846302ef0bc))


### Bug Fixes

* **sdk:** gate the Unix-only exec -i signal test, format test_compile_lane (CUA-1949) ([#5666](https://github.com/trycua/cua/issues/5666)) ([7b590f5](https://github.com/trycua/cua/commit/7b590f5bc98340432cf9d4b5ba753fc9a4dbf09d))
* **sdk:** scope the Duration import to the Unix-only exec -i test (CUA-1949) ([#5668](https://github.com/trycua/cua/issues/5668)) ([f8f1bc2](https://github.com/trycua/cua/commit/f8f1bc2ee9d4b7471867ca95d7227085e10c37a0))
* **sdk:** unused variable in exec -i on Windows, format the e2e (CUA-1949) ([#5662](https://github.com/trycua/cua/issues/5662)) ([d86b5a3](https://github.com/trycua/cua/commit/d86b5a309d8951f531ad5ab181a75bd29b1bd149))
* **telemetry:** Cua Cloud-ended creates report cloud_ended, not unsupported (CUA-1957) ([#5644](https://github.com/trycua/cua/issues/5644)) ([1516be7](https://github.com/trycua/cua/commit/1516be78c69c2924e779187d0c45804cb7f64ba1))
* **vmm:** retry a container image pull when the network drops (CUA-1957) ([#5646](https://github.com/trycua/cua/issues/5646)) ([af9a819](https://github.com/trycua/cua/commit/af9a81994ac77404fc75c8a035a936176953a161))

## [0.6.0](https://github.com/trycua/cua/compare/cua-sdk-v0.5.0...cua-sdk-v0.6.0) (2026-10-20)


### Features

* **cli:** cua sb info --connection prints a Space's spacesd URL and token (CUA-1902) ([#5551](https://github.com/trycua/cua/issues/5551)) ([b8c764e](https://github.com/trycua/cua/commit/b8c764e34e2f907f3cc0943fc4ad7c9bc630cd06))
* **cua-driver:** install the skill pack when setting up an agent, and add get_guide ([#5491](https://github.com/trycua/cua/issues/5491)) ([8637cae](https://github.com/trycua/cua/commit/8637cae79b3decfd8e9e43291a63ffab66bfb867))
* **sdk:** publish cua-sdk and the proto contract so you can build your own harness (CUA-1902) ([#5532](https://github.com/trycua/cua/issues/5532)) ([78175bd](https://github.com/trycua/cua/commit/78175bd5d425c4945df8887aa308bfafd682cd2b))
* **spaces:** "remote hours" everywhere users see it; self-hosting under Settings → Advanced ([#5461](https://github.com/trycua/cua/issues/5461)) ([b9f1874](https://github.com/trycua/cua/commit/b9f1874ba871a13b9e8efc772cb39472a9b0dd58))
* **spaces:** core returns message codes for teleport opt-ins (CUA-1928) ([#5599](https://github.com/trycua/cua/issues/5599)) ([12720d6](https://github.com/trycua/cua/commit/12720d6ab44e99550728d3b54500004b85183046))
* **spaces:** core returns message codes for the sidebar (CUA-1928) ([#5608](https://github.com/trycua/cua/issues/5608)) ([4cddeec](https://github.com/trycua/cua/commit/4cddeec8308871eedb0f26e57f76f3fff065505c))
* **spaces:** each computer's issues on the Machines page from its own health checks, cua doctor --machine (CUA-1820) ([#5379](https://github.com/trycua/cua/issues/5379)) ([904436a](https://github.com/trycua/cua/commit/904436adc45435959566f911290d9e56dd79c516))
* **spaces:** one first run everywhere: connect your agent, then This Mac is your first Space (0.10.0) ([#5462](https://github.com/trycua/cua/issues/5462)) ([c4b4b09](https://github.com/trycua/cua/commit/c4b4b0922f7f0c4d87982a972eea0a0c073e5883))
* **spaces:** preflight checks before creating a local Space (CUA-1916) ([#5553](https://github.com/trycua/cua/issues/5553)) ([7bfb8e1](https://github.com/trycua/cua/commit/7bfb8e17e5a1baecf43a5682835821f7dbc1cd47))
* **spaces:** resources view, daemon resource sampler, relay summaries, CLI columns and SDK resources API (CUA-1880) ([#5469](https://github.com/trycua/cua/issues/5469)) ([5380f80](https://github.com/trycua/cua/commit/5380f800502440243ec6db76e99f731b0e853b29))
* **spaces:** Run an agent here when the first Space is ready, teleport explainer, Skip on first run (CUA-1915, CUA-1917) ([#5550](https://github.com/trycua/cua/issues/5550)) ([5e6b6a3](https://github.com/trycua/cua/commit/5e6b6a3840b5a3950487ab17b7ec921882f3e0f3))
* **spaces:** second-agent and risky-action prompts for This Mac ([#5459](https://github.com/trycua/cua/issues/5459)) ([41648bc](https://github.com/trycua/cua/commit/41648bc84e65d21223db86e01075740e74480f06))
* **telemetry:** installer funnel events and an agent-used-a-Space event (CUA-1881) ([#5457](https://github.com/trycua/cua/issues/5457)) ([ba3e14f](https://github.com/trycua/cua/commit/ba3e14fd8beb441c532bf75e3ef84c29d454213c))
* **telemetry:** link signed-in usage data to an opaque account id, off until the policy is published (CUA-1881) ([#5472](https://github.com/trycua/cua/issues/5472)) ([911fc16](https://github.com/trycua/cua/commit/911fc16042b2f7327947f47dd935095e174e6b1c))
* **telemetry:** ui_locale on cua_app_active (CUA-1928) ([#5569](https://github.com/trycua/cua/issues/5569)) ([e4b0baa](https://github.com/trycua/cua/commit/e4b0baaf5f3c91cd0da739e91a76a866550812be))


### Bug Fixes

* **cua-cli:** treat provisionable Mac engines as usable in runtime checks ([b31ff43](https://github.com/trycua/cua/commit/b31ff4332cbd547dcbc7d5d6f3d3a4ae0dc7ad46))
* **cua-driver:** fix Windows selected-browser launch guidance ([50313e7](https://github.com/trycua/cua/commit/50313e7a7475846a459048b89758e6d0bc14df6d))
* **cua-driver:** launch the installed app by path, stop daemons from any copy, drop extra copies ([#5471](https://github.com/trycua/cua/issues/5471)) ([41eee2f](https://github.com/trycua/cua/commit/41eee2fb91c20a77a9e55e4f013fc968081ad74c))
* **cua-spaces:** teleport_app moves only the selected items ([#5504](https://github.com/trycua/cua/issues/5504)) ([d247bf3](https://github.com/trycua/cua/commit/d247bf36adb77f9b76bac43f6295defa2a8769bc))
* **sdk:** delete the Fleet SDK, pools and claims from the Rust SDK, cua-sandbox, Cua Bench and the apps (CUA-1836) ([#5488](https://github.com/trycua/cua/issues/5488)) ([348c47c](https://github.com/trycua/cua/commit/348c47cc30b1d222844480838ccc4acf53f7a511))
* **security:** distrust the compromised eMudhra intermediates in every Cua client (CUA-1948) ([#5622](https://github.com/trycua/cua/issues/5622)) ([149ea51](https://github.com/trycua/cua/commit/149ea51c613cc840c3760dec822c57b7e65df05a))
* **spaces:** honest Done screen that sees the CLI and sign-in you already have (CUA-1862) ([#5447](https://github.com/trycua/cua/issues/5447)) ([bea1cfd](https://github.com/trycua/cua/commit/bea1cfd3f55b36af6f1d3d290e67d316dfecf5ff))
* **spaces:** Linux Share in remote desktops asks for the password, says what to do when it cannot, and cua_share says why it failed (CUA-1725, CUA-1855) ([#5273](https://github.com/trycua/cua/issues/5273)) ([522e032](https://github.com/trycua/cua/commit/522e032fb3ac340aca1f0e1301413d7f08839e31))
* **spaces:** plain plan errors and a way to upgrade when adding a team computer ([#5217](https://github.com/trycua/cua/issues/5217)) ([db87d66](https://github.com/trycua/cua/commit/db87d6676d820da152291072df3715c05a0d7e62))
* **spaces:** say computer instead of machine (CUA-1863) ([#5508](https://github.com/trycua/cua/issues/5508)) ([5cbe31c](https://github.com/trycua/cua/commit/5cbe31c7109fa70eb596b6bde3b5964449e8676e))

## [0.5.0](https://github.com/trycua/cua/compare/cua-sdk-v0.4.1...cua-sdk-v0.5.0) (2026-10-19)


### ⚠ BREAKING CHANGES

* **cua-driver:** publish Vertex-compatible tool input schemas ([#4888](https://github.com/trycua/cua/issues/4888))

### Features

* **cli:** let cua do target relay Spaces ([#4797](https://github.com/trycua/cua/issues/4797)) ([ad563fe](https://github.com/trycua/cua/commit/ad563fef24973df11f9186f6bb8205cf91f69ebe))
* **cua-driver:** attach to an existing Chrome profile in the background when Chrome is on another Space (macOS) ([#5052](https://github.com/trycua/cua/issues/5052)) ([10c5d3f](https://github.com/trycua/cua/commit/10c5d3f3d40f96009ac542fd3725b832b2ff7a36))
* **cua-driver:** six agent cursor motion styles ([#4659](https://github.com/trycua/cua/issues/4659)) ([5e5370f](https://github.com/trycua/cua/commit/5e5370f7c9a57e9540eadc17c19b5bfeca0d9d91))
* **cua:** add a machine with a one-time pairing code (cua host setup --pair, installers --pair) (CUA-1358) ([#5003](https://github.com/trycua/cua/issues/5003)) ([2a5a7ca](https://github.com/trycua/cua/commit/2a5a7caacd323f0e39b94c4c607d189374a89718))
* **cua:** CUA_ENV=staging targets the Cua staging deployment (CUA-1395) ([#5013](https://github.com/trycua/cua/issues/5013)) ([72f329d](https://github.com/trycua/cua/commit/72f329d85246c42458b900d6b21b8cd34b830d05))
* **spaces-macos:** Machines page, Get started checklist, ⌘K palette, ⌘1-4 (CUA-1412) ([#5035](https://github.com/trycua/cua/issues/5035)) ([5b4a16a](https://github.com/trycua/cua/commit/5b4a16a522c71a3d154cea251b87225750a17fdc))
* **spaces-web:** Spaces filter box, ⌘K machine keywords, power errors on tiles ([#5101](https://github.com/trycua/cua/issues/5101)) ([1ec1c09](https://github.com/trycua/cua/commit/1ec1c09bd2ced296024373826b551841fe7c0411))
* **spaces:** empty home with one-click Linux and macOS create ([#4870](https://github.com/trycua/cua/issues/4870)) ([2e4736b](https://github.com/trycua/cua/commit/2e4736b3ebff61ef99e8c0c74270b5cd75894643))
* **spaces:** end-to-end encrypted connections between your machines, an opt-in beta (CUA-1505) ([#5094](https://github.com/trycua/cua/issues/5094)) ([ae2c117](https://github.com/trycua/cua/commit/ae2c117cd96673bb2dc0c2b3bff023ad70586b92))
* **spaces:** getting-started checklist in the shared web UI (CUA-1359) ([#4999](https://github.com/trycua/cua/issues/4999)) ([9ad6a39](https://github.com/trycua/cua/commit/9ad6a396cd1961827b0ddb89a5a56f3324ee68d3))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))
* **spaces:** New UI on by default for Beta (0.8.1), saved only as an explicit choice (CUA-1514) ([#5108](https://github.com/trycua/cua/issues/5108)) ([3eb7c09](https://github.com/trycua/cua/commit/3eb7c09ce3cd343cf4e1d5b2dba7ce49c6e47989))
* **spaces:** real Teams screen, api.cua.ai and app.cua.ai (0.8.1) ([#5113](https://github.com/trycua/cua/issues/5113)) ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))
* **spaces:** relay allowance usage and upgrade notice in the app and CLI (CUA-1360) ([#5002](https://github.com/trycua/cua/issues/5002)) ([c9a6079](https://github.com/trycua/cua/commit/c9a607993875d341f70124c7d7f4398297317016))
* **spaces:** relay usage row and allowance notice on the Mac app (CUA-1410) ([#5034](https://github.com/trycua/cua/issues/5034)) ([a329fcf](https://github.com/trycua/cua/commit/a329fcf435191655a8dc8271c0df9ea824c2b564))
* **spaces:** T3-style in-app updates on macOS, Windows and Linux (CUA-1780) ([#5326](https://github.com/trycua/cua/issues/5326)) ([a6de100](https://github.com/trycua/cua/commit/a6de100971b217395cad6445f5ec547cdef5c3e8))
* **spaces:** Teams on by default, CUA_EXPERIMENTAL_TEAMS=0 as kill switch ([#5142](https://github.com/trycua/cua/issues/5142)) ([e0a49bc](https://github.com/trycua/cua/commit/e0a49bcc392460fc815093895d6b26e34ba1a1b0))


### Bug Fixes

* **cli:** fail fast on a daemon share without a terminal; test shared relay Spaces ([#5017](https://github.com/trycua/cua/issues/5017)) ([5c74ac3](https://github.com/trycua/cua/commit/5c74ac36b357c7413e9c256fb3c51ad0b478481c))
* **cli:** unselect a direct cua do target that stops answering ([#4868](https://github.com/trycua/cua/issues/4868)) ([97ef165](https://github.com/trycua/cua/commit/97ef165a7b5024087861879ff3b43d96f309e7b7)), closes [#4788](https://github.com/trycua/cua/issues/4788)
* **cua mcp:** act as a named agent so Spaces draw its cursor (CUA-1750) ([#5290](https://github.com/trycua/cua/issues/5290)) ([afed1fc](https://github.com/trycua/cua/commit/afed1fcbba64acadcc154b8d5d377549ae62bc7e))
* **cua-agent-setup:** run Claude Code's own exe on Windows, hide the console (CUA-1703) ([#5261](https://github.com/trycua/cua/issues/5261)) ([850e4d7](https://github.com/trycua/cua/commit/850e4d7e982e8315e352b74a56df30d6bebd5c01))
* **cua-auth:** keep the sign-in under ai.cua.session and move it from run.cua.ai (CUA-1877) ([#5436](https://github.com/trycua/cua/issues/5436)) ([7eb732a](https://github.com/trycua/cua/commit/7eb732acd3643741c6c13ccbe41f79124fb7fcc5))
* **cua-auth:** never wait on a macOS keychain prompt without a terminal ([#5427](https://github.com/trycua/cua/issues/5427)) ([04f75fa](https://github.com/trycua/cua/commit/04f75fa0d34c842e6e204ff69670ae49a397640e))
* **cua-cli:** gate agents_login Pty::output to Unix so Windows clippy passes ([#5449](https://github.com/trycua/cua/issues/5449)) ([eb679b9](https://github.com/trycua/cua/commit/eb679b9e021146c5d948e58b8422e6b0589cf3b1))
* **cua-cli:** make cloud providers an opt-in byoc feature ([#4969](https://github.com/trycua/cua/issues/4969)) ([fa8c81d](https://github.com/trycua/cua/commit/fa8c81de38c587f9d18ffde5daeea40f26dce69a))
* **cua-cli:** never take a stray key or end of input as the answer to the agents question (CUA-1862) ([#5435](https://github.com/trycua/cua/issues/5435)) ([5df19ea](https://github.com/trycua/cua/commit/5df19ea9ba78a4aa0a396c682685c69ae1eca51a))
* **cua-cli:** open the sign-in URL on Windows without cmd.exe (CUA-1701) ([#5260](https://github.com/trycua/cua/issues/5260)) ([c6fed85](https://github.com/trycua/cua/commit/c6fed85957547ca4c5aef8cccd755ff7c11cf9e3))
* **cua-daemon:** compile without the client feature ([#4937](https://github.com/trycua/cua/issues/4937)) ([ac967b9](https://github.com/trycua/cua/commit/ac967b9006aafa10870aadb5a645b038cba9721a))
* **cua-daemon:** replace another app's daemon whose executable is gone or replaced ([#4942](https://github.com/trycua/cua/issues/4942)) ([d7a713c](https://github.com/trycua/cua/commit/d7a713c0338d78bf3dcbe997e911fa0a0d34d8e1))
* **cua-daemon:** stop the setup tests running the real Lume installer ([#5428](https://github.com/trycua/cua/issues/5428)) ([044ed87](https://github.com/trycua/cua/commit/044ed871aa2df4dd54e56575efcc66fe9d245043))
* **cua-driver:** background browser clicks reach pointer-driven rows ([#5110](https://github.com/trycua/cua/issues/5110)) ([a7c4a0e](https://github.com/trycua/cua/commit/a7c4a0ea5e197fdfb422fa07a53649419296e842))
* **cua-driver:** publish Vertex-compatible tool input schemas ([#4888](https://github.com/trycua/cua/issues/4888)) ([9f78f0e](https://github.com/trycua/cua/commit/9f78f0e4f598866f210f069e7e66b27a19c661f7))
* **cua-driver:** refuse background existing-profile attach on macOS instead of bringing Chrome forward ([#5007](https://github.com/trycua/cua/issues/5007)) ([e1a6396](https://github.com/trycua/cua/commit/e1a6396b42a0f0dae992a79560522aefbd53cf8c))
* **cua-host:** Windows unattended access without admin rights (CUA-1702) ([#5262](https://github.com/trycua/cua/issues/5262)) ([7f7114c](https://github.com/trycua/cua/commit/7f7114caa84003d0392fc5a19012d3496783e91c))
* **cua-sdk:** confirm daemon exit before replacement ([#5086](https://github.com/trycua/cua/issues/5086)) ([97fea58](https://github.com/trycua/cua/commit/97fea581e214dbec0a80132945886409aa5c3874))
* **cua-spaces-macos:** ask before installing cua and adding it to PATH ([#5039](https://github.com/trycua/cua/issues/5039)) ([80ae9a4](https://github.com/trycua/cua/commit/80ae9a47ad9998a819bd5636b84e7eccb4a15841))
* **cua-spaces:** a brief relay outage no longer flashes "Could not refresh Spaces" (CUA-1770) ([#5315](https://github.com/trycua/cua/issues/5315)) ([84a55c2](https://github.com/trycua/cua/commit/84a55c29d4838bd9ab831e3b2629a335a17ae90e))
* **cua-spacesd:** stream 6K displays and carry tool results over 4 MiB ([#4850](https://github.com/trycua/cua/issues/4850)) ([a88f100](https://github.com/trycua/cua/commit/a88f100b428a3eb9a73bd60757540d6ecd1fd00f)), closes [#4623](https://github.com/trycua/cua/issues/4623)
* **cua-spaces:** fail a macOS create within seconds when its image download stops ([#5214](https://github.com/trycua/cua/issues/5214)) ([0bd9d11](https://github.com/trycua/cua/commit/0bd9d113fe84640c723a0c170480d08186f5db58))
* **cua-spaces:** keep a create on another machine alive while it downloads (CUA-1760) ([#5295](https://github.com/trycua/cua/issues/5295)) ([4fe8c46](https://github.com/trycua/cua/commit/4fe8c461f600e827b9e4418a1332e7536a083ca1))
* **cua-spaces:** name a local Space what it was created as ([#4849](https://github.com/trycua/cua/issues/4849)) ([b0a6348](https://github.com/trycua/cua/commit/b0a6348133a34b3d158eae38d06a606215eec6df)), closes [#4707](https://github.com/trycua/cua/issues/4707)
* **cua-spaces:** onboarding This machine Continue button, launch at login default, static prompts ([#5317](https://github.com/trycua/cua/issues/5317)) ([861c4b0](https://github.com/trycua/cua/commit/861c4b0f9e74fc51935333692374b8bb4d8618da))
* **cua-spaces:** report missing Spaces in share queries ([#4730](https://github.com/trycua/cua/issues/4730)) ([149b35c](https://github.com/trycua/cua/commit/149b35cc3298cad93644619c1dbf5e67c827d560)), closes [#4706](https://github.com/trycua/cua/issues/4706)
* **cua-spaces:** show a create's real stages and download progress, also on another machine ([#5198](https://github.com/trycua/cua/issues/5198)) ([13facff](https://github.com/trycua/cua/commit/13facfffdd95927ebac4218f10b8275d69f40759))
* **cua-spaces:** typed resource values, the generated Space name, and picking saved passwords ([#5205](https://github.com/trycua/cua/issues/5205)) ([a14eadd](https://github.com/trycua/cua/commit/a14eadd3b8a563dc4b73d58ff07cd4a2df1a9dc1))
* **cua-vmm:** detect live processes on Windows in pid_alive ([#4845](https://github.com/trycua/cua/issues/4845)) ([43e4c42](https://github.com/trycua/cua/commit/43e4c42ed3cfc3af1a31015715d6429cc47976a1)), closes [#4756](https://github.com/trycua/cua/issues/4756)
* **cua-vmm:** explain KVM fallback and refuse unusable /dev/kvm for native guests ([#4971](https://github.com/trycua/cua/issues/4971)) ([e9dc657](https://github.com/trycua/cua/commit/e9dc65780bdbf6e68d42aafa959c1925dbba2a56))
* **cua-vmm:** make /dev/kvm usable in cua runtime setup qemu ([8f9a41d](https://github.com/trycua/cua/commit/8f9a41d1fc5ca2a53232cba241a9f2912a761644))
* **cua-vmm:** refresh apt before installing QEMU and print the plan that runs ([#4972](https://github.com/trycua/cua/issues/4972)) ([8ac7727](https://github.com/trycua/cua/commit/8ac7727281ec72b76258ce0821ecb43359690768))
* **cua-vmm:** reserve overlay-sized space for QEMU creates and check it before pulling ([#5042](https://github.com/trycua/cua/issues/5042)) ([40f82b4](https://github.com/trycua/cua/commit/40f82b4b61d3d2fc00c38e7be457140c6b981d87))
* **cua:** CUA_ENV=staging signs in at auth.staging.cua.ai (CUA-1522) ([#5107](https://github.com/trycua/cua/issues/5107)) ([c2a16d3](https://github.com/trycua/cua/commit/c2a16d3b27cbf18beb396fc86284b87d1b9449c9))
* **cua:** end an old cua daemon that never exits after an app update ([#5191](https://github.com/trycua/cua/issues/5191)) ([7bdb2a2](https://github.com/trycua/cua/commit/7bdb2a2fed02c77f50fc01942249ee3002af84a5))
* **cua:** open a direct Space added by address with its registered token ([#5077](https://github.com/trycua/cua/issues/5077)) ([c543782](https://github.com/trycua/cua/commit/c54378228377c5792c9f70e414121fbf0a810ab8))
* **cua:** split create runtime errors by cause; actionable engine and cloud hints ([#5000](https://github.com/trycua/cua/issues/5000)) ([43a76de](https://github.com/trycua/cua/commit/43a76de6f8dea32b7f76a5b0b47e33303b0c405f))
* **cua:** support host-only sign-in from a remote machine ([#4675](https://github.com/trycua/cua/issues/4675)) ([ea763a1](https://github.com/trycua/cua/commit/ea763a110ff967e8caaea4ff551b1b083d113849))
* **daemon:** run bare images on QEMU when no container engine is installed ([#5022](https://github.com/trycua/cua/issues/5022)) ([dfb4901](https://github.com/trycua/cua/commit/dfb49015a741851c99f2c4ab3b726cf599629b68))
* **keyvault:** delete items whose delivery target or import is gone ([#4889](https://github.com/trycua/cua/issues/4889)) ([cf1217c](https://github.com/trycua/cua/commit/cf1217ce90092f1409b195d951cf2bb40be3cbba)), closes [#4887](https://github.com/trycua/cua/issues/4887)
* **sandbox:** name a Local Network block when a resumed Space waits for its services (CUA-1807) ([#5363](https://github.com/trycua/cua/issues/5363)) ([3bfb1d1](https://github.com/trycua/cua/commit/3bfb1d1d146408a628345af7d23b6b16648dd316))
* **sandbox:** report the live status of local VMs in sandbox ls ([#4657](https://github.com/trycua/cua/issues/4657)) ([9db2208](https://github.com/trycua/cua/commit/9db2208f942460445c0f5b589f6862b91ebf09a6))
* **sdk:** honor HTTP proxies for controller RPC ([#4795](https://github.com/trycua/cua/issues/4795)) ([23e23d4](https://github.com/trycua/cua/commit/23e23d4cc321b1e64f5832d44a150667ff0d5c04))
* **spaces:** boot local Windows VMs from AHCI like the image build ([#4851](https://github.com/trycua/cua/issues/4851)) ([7d0354f](https://github.com/trycua/cua/commit/7d0354fdfbe4ad8135ad8c4e7f9d0755f72bf3d8)), closes [#4777](https://github.com/trycua/cua/issues/4777)
* **spaces:** drop the last run.cua.ai billing link from the Swift fixture ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))
* **spaces:** GPU acceleration works for a macOS Space created from the app (CUA-1778) ([#5328](https://github.com/trycua/cua/issues/5328)) ([384077f](https://github.com/trycua/cua/commit/384077f579b2229c428b16ddd2f0c2eb3ea33906))
* **spaces:** leave the machines that provide Spaces out of the menu bar count ([#5050](https://github.com/trycua/cua/issues/5050)) ([97b2394](https://github.com/trycua/cua/commit/97b2394fc1173588e04af6901385a167d7f89029))
* **spaces:** let Linux Spaces use the Mac's memory instead of a fixed 4 GiB runtime (CUA-1759) ([#5298](https://github.com/trycua/cua/issues/5298)) ([57af836](https://github.com/trycua/cua/commit/57af836ee0e8aca4ee39c258a658c47d9ade5824))
* **spaces:** list only Spaces in the notch, never the machines that provide them ([#5190](https://github.com/trycua/cua/issues/5190)) ([1ed129f](https://github.com/trycua/cua/commit/1ed129f64eceaa97cb9f0a082d91512f58f523a7))
* **spaces:** make the Ubuntu .deb removable and close the Linux gaps (CUA-1521, CUA-1524, CUA-1530) ([#5104](https://github.com/trycua/cua/issues/5104)) ([b1ec651](https://github.com/trycua/cua/commit/b1ec651278660affe5f43223d3faa3c587c26acc))
* **spaces:** mount a Space's Cua Volume again once it is back, instead of saying its cua-spacesd predates the volume (CUA-1759) ([#5300](https://github.com/trycua/cua/issues/5300)) ([2fa609c](https://github.com/trycua/cua/commit/2fa609c12a65daf5f99c605f90d48ed549a3f72e))
* **spaces:** name the app blocked from the Local Network in cua doctor, the daemon log and Turn on (CUA-1819) ([#5378](https://github.com/trycua/cua/issues/5378)) ([075add5](https://github.com/trycua/cua/commit/075add55cf938f4e351b74539efa18d4976696ae))
* **spaces:** name why a Space create failed, and wait for Local Network access on macOS (CUA-1368) ([#4998](https://github.com/trycua/cua/issues/4998)) ([cd07939](https://github.com/trycua/cua/commit/cd07939d962de8be30f053ad21f2e97971ca3429))
* **spaces:** one machine state in both UIs, from the core (CUA-1617) ([#5188](https://github.com/trycua/cua/issues/5188)) ([f820274](https://github.com/trycua/cua/commit/f8202745a66a844bbafc4811878f3a43da0c36fb))
* **spaces:** one owner for relay machine state ([9994fc2](https://github.com/trycua/cua/commit/9994fc2e41a87808514cdaa0b62f9e20bfc235db)), closes [#4784](https://github.com/trycua/cua/issues/4784)
* **spaces:** one-time migration turns Space sharing on for 0.9.0 settings (CUA-1668) ([#5225](https://github.com/trycua/cua/issues/5225)) ([749636a](https://github.com/trycua/cua/commit/749636a5df27e773aaeee03baf083e9761a7f99d))
* **spaces:** prefill the computer name and explain host setup in plain words (CUA-1862) ([#5440](https://github.com/trycua/cua/issues/5440)) ([9215d41](https://github.com/trycua/cua/commit/9215d41ae20f92e80fdd867f5ca07ca2d36e6137))
* **spaces:** preserve live OS metadata and show unknown honestly ([#4671](https://github.com/trycua/cua/issues/4671)) ([f25fe36](https://github.com/trycua/cua/commit/f25fe3691e651cc8ac6adf92931940ed25d9a6bb))
* **spaces:** rc.2 smoke fixes: Continue on This machine, example prompts, update defaults, plain errors (CUA-1816) ([#5367](https://github.com/trycua/cua/issues/5367)) ([9fd19f3](https://github.com/trycua/cua/commit/9fd19f31db4d05196e7ea2775063bb2b1322b607))
* **spaces:** reach a macOS Space again after lume serve exits ([#4661](https://github.com/trycua/cua/issues/4661)) ([bba73a6](https://github.com/trycua/cua/commit/bba73a6e51339567819120f91f1c7c65fce0d999))
* **spaces:** refuse a Windows VM create without QEMU before the 6 GB pull (CUA-1761) ([#5294](https://github.com/trycua/cua/issues/5294)) ([e16a462](https://github.com/trycua/cua/commit/e16a462d854bb78079ea4d8679b20b5e2c68d9b2))
* **spaces:** register gVisor with --allow-suid so the Space user can sudo and mount FUSE ([#4852](https://github.com/trycua/cua/issues/4852)) ([cacc440](https://github.com/trycua/cua/commit/cacc4409baedd1693017029a4876e4109a126e06)), closes [#4667](https://github.com/trycua/cua/issues/4667)
* **spaces:** renew rejected device sessions during discovery ([#4780](https://github.com/trycua/cua/issues/4780)) ([12af913](https://github.com/trycua/cua/commit/12af9135471d03d96863a910f9020b2845d67a21))
* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))
* **spaces:** same checklist count and every machine in every UI ([#5106](https://github.com/trycua/cua/issues/5106)) ([ff6079b](https://github.com/trycua/cua/commit/ff6079bef5b6089a5ef649c9447e441c72c9519b))
* **spaces:** say why a Space stopped on its own (out of memory) (CUA-1759) ([#5303](https://github.com/trycua/cua/issues/5303)) ([9737d6f](https://github.com/trycua/cua/commit/9737d6f22bad592038335fd156d1e02766fe768b))
* **spaces:** shorten the macOS team-share refusal message ([#5056](https://github.com/trycua/cua/issues/5056)) ([1c985c2](https://github.com/trycua/cua/commit/1c985c2dee92e0f23d4a5f348bdd2ec14ddf30cd))
* **spaces:** show the runtime-inclusive Linux disk footprint ([#5038](https://github.com/trycua/cua/issues/5038)) ([a3f9a25](https://github.com/trycua/cua/commit/a3f9a2595f5358b78d3ba03a7657ac127ef43f2e))
* **spaces:** Teams clients use Keycloak orgs via the Spaces API ([#5157](https://github.com/trycua/cua/issues/5157)) ([49a89ff](https://github.com/trycua/cua/commit/49a89ff38a06e5b17413d52d2cf6534a13cdba7f))
* **spaces:** teleport refusal wording, picker title after Back, Status of an unreachable Space ([#5103](https://github.com/trycua/cua/issues/5103)) ([154f225](https://github.com/trycua/cua/commit/154f225f54670d7e50def027616f324cafa82498))
* **spaces:** Teleport's review says what happens for "Just the app" (CUA-1616) ([#5187](https://github.com/trycua/cua/issues/5187)) ([a8d9f38](https://github.com/trycua/cua/commit/a8d9f38ff49f5f0ed8c5fd9681945f3aa97491ae))
* **spaces:** turn Space sharing on by default ([#5218](https://github.com/trycua/cua/issues/5218)) ([8049050](https://github.com/trycua/cua/commit/8049050559f77647f08c78fc276a70f19a660a7f))
* **spaces:** Upgrade falls back to the account app's billing page (CUA-1515) ([#5128](https://github.com/trycua/cua/issues/5128)) ([4b37441](https://github.com/trycua/cua/commit/4b37441420ca4c5fe09ac4fee575e9db221b410b))
* **spaces:** Windows Space image pull continues after a broken download (CUA-1856) ([#5408](https://github.com/trycua/cua/issues/5408)) ([15ac317](https://github.com/trycua/cua/commit/15ac317bf21dc148488636825f7641eb9ac9f14f))
* stop Cua Cloud (on="cloud") with a clear "Cua Cloud has ended" error (CUA-1854) ([#5415](https://github.com/trycua/cua/issues/5415)) ([5d78ec8](https://github.com/trycua/cua/commit/5d78ec8b7705d942a3d260b276bcce76614a634c))
* **vmm:** install ssh-keygen with cua runtime setup qemu ([#5020](https://github.com/trycua/cua/issues/5020)) ([88aff7e](https://github.com/trycua/cua/commit/88aff7e5424e9ed45b93ded85eb4111472ca5190))
* **vmm:** report QEMU's exit instead of a QMP timeout when it dies during launch ([#5021](https://github.com/trycua/cua/issues/5021)) ([beb2174](https://github.com/trycua/cua/commit/beb217484ab3c5372800d6b8f9a4ecf495bd6735))
* **windows:** uninstall and host remove leave no driver or driver.log behind (CUA-1749) ([#5289](https://github.com/trycua/cua/issues/5289)) ([0b5bf47](https://github.com/trycua/cua/commit/0b5bf4704caf3497d0f8523b08533dc89e593b7d))

## [0.4.1](https://github.com/trycua/cua/compare/cua-sdk-v0.4.0...cua-sdk-v0.4.1) (2026-10-05)


### Bug Fixes

* **auth:** single-flight token refresh across processes and keep sessions alive ([#4633](https://github.com/trycua/cua/issues/4633)) ([bc3a0f0](https://github.com/trycua/cua/commit/bc3a0f074b6f9dcfb5200586224d300fd8a26afa))

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
