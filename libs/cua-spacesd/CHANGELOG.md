# Changelog

## [0.15.2](https://github.com/trycua/cua/compare/cua-spacesd-v0.15.1...cua-spacesd-v0.15.2) (2026-11-11)


### Bug Fixes

* **cua-spacesd-doctor:** type into the form only after its entry holds keyboard focus ([#7322](https://github.com/trycua/cua/issues/7322)) ([0470fc6](https://github.com/trycua/cua/commit/0470fc64e2ad0486b655182a5cc6321711378223))

## [0.15.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.15.0...cua-spacesd-v0.15.1) (2026-11-10)


### Bug Fixes

* **cua-spacesd-doctor:** retry input.type once when nothing reached the entry ([#7290](https://github.com/trycua/cua/issues/7290)) ([6e7a5a4](https://github.com/trycua/cua/commit/6e7a5a4e0c3717eb2d12e2e81ae4dde9c6ba3126))


### Performance Improvements

* **cua-ci:** build cua-ci beside cua-sdk, not after it (CUA-3971) ([#7240](https://github.com/trycua/cua/issues/7240)) ([1d23545](https://github.com/trycua/cua/commit/1d23545a03689800046f5fd7ecf78c3d128dec61))

## [0.15.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.14.0...cua-spacesd-v0.15.0) (2026-11-10)


### Features

* **spaces:** show a child Space's window in the parent's Stream section and stream it directly from the child ([#6819](https://github.com/trycua/cua/issues/6819)) ([5f09d89](https://github.com/trycua/cua/commit/5f09d890ad10a68c02de0cfd4e3fe24096537059))


### Bug Fixes

* **cua-spacesd:** remove the agent sandbox even when its setup fails ([#7232](https://github.com/trycua/cua/issues/7232)) ([7b8de09](https://github.com/trycua/cua/commit/7b8de092587a04926919b52e2dcb472a4e9e947a))
* **volume:** a stalled sync never hangs an agent start or the mount (CUA-4002) ([#7237](https://github.com/trycua/cua/issues/7237)) ([7f4a358](https://github.com/trycua/cua/commit/7f4a35873a00a101563394f54f8497d1288a3c8b))


### Performance Improvements

* **cua:** split cua-daemon into cua-daemon-core and the server (CUA-3971) ([#7214](https://github.com/trycua/cua/issues/7214)) ([9058fbe](https://github.com/trycua/cua/commit/9058fbe9f873c5ccc038a8350472045869275895))

## [0.14.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.13.0...cua-spacesd-v0.14.0) (2026-11-10)


### Features

* **spaces:** Child Spaces and Warm start are always on, their Labs switches are gone (CUA-3751) ([#6873](https://github.com/trycua/cua/issues/6873)) ([57942a0](https://github.com/trycua/cua/commit/57942a0e67625dffe0f79f9ea146395ff1cdac53))

## [0.13.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.8...cua-spacesd-v0.13.0) (2026-11-09)


### Features

* **spaces-macos:** install-local.sh --development for fast dev-loop builds ([#7019](https://github.com/trycua/cua/issues/7019)) ([7f9f5c2](https://github.com/trycua/cua/commit/7f9f5c20440510a999851010c00034d2afd433d1))

## [0.12.8](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.7...cua-spacesd-v0.12.8) (2026-11-09)


### Bug Fixes

* **cluster:** GPU jobs are admitted by free video memory and run on Windows hosts ([#6943](https://github.com/trycua/cua/issues/6943)) ([c62abf3](https://github.com/trycua/cua/commit/c62abf39ba407f385701b0a82a5a4d8b8eacc426))

## [0.12.7](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.6...cua-spacesd-v0.12.7) (2026-11-07)


### Bug Fixes

* **cua-driver:** give KWin windows exact bounds, stacking, capture and bring_to_front ([#6820](https://github.com/trycua/cua/issues/6820)) ([5ee57ab](https://github.com/trycua/cua/commit/5ee57ab5bba333f33e48df787b77d997cc1e8019))

## [0.12.6](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.5...cua-spacesd-v0.12.6) (2026-11-07)


### Bug Fixes

* **spaces:** every agent message is appended to the thread, child Spaces work from inside a Space, and child agents show in the chat ([#6733](https://github.com/trycua/cua/issues/6733)) ([515371a](https://github.com/trycua/cua/commit/515371a48a616aea3bba5f0ea13e0863c8a26041))

## [0.12.5](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.4...cua-spacesd-v0.12.5) (2026-11-07)


### Bug Fixes

* **spaces:** say computers in use at once for the plan limit (CUA-3690) ([#6790](https://github.com/trycua/cua/issues/6790)) ([891ccb7](https://github.com/trycua/cua/commit/891ccb7153dde86f6219da719c583d2de0b88b14))

## [0.12.4](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.3...cua-spacesd-v0.12.4) (2026-11-07)


### Bug Fixes

* **cua-driver:** keep a session's last use on its lifecycle record ([#6773](https://github.com/trycua/cua/issues/6773)) ([94e3d18](https://github.com/trycua/cua/commit/94e3d186262431c30cdf0891231eeee55909398e))

## [0.12.3](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.2...cua-spacesd-v0.12.3) (2026-11-06)


### Bug Fixes

* **cua-driver:** admit any native Wayland app to Hyprland background input ([#6706](https://github.com/trycua/cua/issues/6706)) ([895aec7](https://github.com/trycua/cua/commit/895aec7e51b0409b2b22c958920d52c22b0d2b03))
* **spaces:** explain a refused new computer and offer a code from another computer ([#6710](https://github.com/trycua/cua/issues/6710)) ([327f41c](https://github.com/trycua/cua/commit/327f41c89be3e8209a4bfc3b5785cb423f97de9c))

## [0.12.2](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.1...cua-spacesd-v0.12.2) (2026-11-06)


### Bug Fixes

* **cua-driver:** find a macOS app by any name it ships, without guessing ([#6657](https://github.com/trycua/cua/issues/6657)) ([22d8c1b](https://github.com/trycua/cua/commit/22d8c1b9867fa2cf1dff32e28d577f953144266f))
* **cua-spacesd:** wait for each VideoToolbox frame on macOS so encoder surfaces do not pile up ([#6621](https://github.com/trycua/cua/issues/6621)) ([d81ab3e](https://github.com/trycua/cua/commit/d81ab3e3b291029100f83b9273f49ca03c6dad25))

## [0.12.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.12.0...cua-spacesd-v0.12.1) (2026-11-05)


### Bug Fixes

* **spacesd:** volume attach no longer deadlocks a 1 vCPU Space ([#6565](https://github.com/trycua/cua/issues/6565)) ([8a708ab](https://github.com/trycua/cua/commit/8a708abf6a60b76894df8d95ff94450436a3032f))
* **windows:** no firewall prompt for cua and cua-spacesd, and a real publisher name ([#6558](https://github.com/trycua/cua/issues/6558)) ([0d196ad](https://github.com/trycua/cua/commit/0d196ad7669848951b7f5fa336af01e3f571d1f8))

## [0.12.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.11.0...cua-spacesd-v0.12.0) (2026-11-05)


### Features

* **relay,spacesd:** a removed computer deletes its Spaces when it next comes online (CUA-3479) ([#6532](https://github.com/trycua/cua/issues/6532)) ([e7e78c8](https://github.com/trycua/cua/commit/e7e78c8217b126e4bd02ff67ef7d733d11ec243f))


### Bug Fixes

* **cua-driver:** initialize recording hooks for embedded platform registries ([#5785](https://github.com/trycua/cua/issues/5785)) ([0a05cd4](https://github.com/trycua/cua/commit/0a05cd40069594f02738bc2f4ac6f6bc0dc6390d))
* **cua-mesh:** a failed dial no longer answers for a peer that came online since ([#6479](https://github.com/trycua/cua/issues/6479)) ([ebf96e5](https://github.com/trycua/cua/commit/ebf96e5ff17a12d92f67f7d32a740662a1bc435c))
* **spaces:** Cua Driver runs after the new first run, whatever the Mac shares (CUA-3485) ([#6534](https://github.com/trycua/cua/issues/6534)) ([97eabde](https://github.com/trycua/cua/commit/97eabde88dd1fdf4548dbe2d12af8396211ff4ff))

## [0.11.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.10.1...cua-spacesd-v0.11.0) (2026-11-05)


### Features

* **relay:** default Space settings and detach-not-delete for removed computers (CUA-3413) ([#6504](https://github.com/trycua/cua/issues/6504)) ([7e86b3e](https://github.com/trycua/cua/commit/7e86b3ea54412dace87954a67312dc3f0dcd399b))
* **relay:** strict device approval for phones only (CUA_RELAY_DEVICE_REQUIRE_APPROVAL=phones) ([#6452](https://github.com/trycua/cua/issues/6452)) ([3271e51](https://github.com/trycua/cua/commit/3271e514e91daa58847297d9d7f25e4138e5e4eb))
* **spaces-macos:** Cua Driver built into Cua Spaces (CUA-2771, CUA-2775) ([#6505](https://github.com/trycua/cua/issues/6505)) ([b5125d2](https://github.com/trycua/cua/commit/b5125d2eaf2463b7bbac219b19fd281420213e1a))
* **spaces:** ask first, CI runners, TCP shares, secure networks and the phone app, behind experiments ([#6355](https://github.com/trycua/cua/issues/6355)) ([8cfff8e](https://github.com/trycua/cua/commit/8cfff8ef0e4a904c1534eac44cea3d65e741e6fd))
* **spaces:** pick up where you left off in the first run (CUA-3319) ([#6463](https://github.com/trycua/cua/issues/6463)) ([a79ecf8](https://github.com/trycua/cua/commit/a79ecf8dddf225d11367360cc2dfcf6a9c135340))
* **spaces:** Secure Spaces, child Spaces, conformance, space jobs and the macOS developer-tools image, behind experiments ([#6306](https://github.com/trycua/cua/issues/6306)) ([4715a72](https://github.com/trycua/cua/commit/4715a723a450610cdf77f688776c1fdd40660149))
* **spaces:** the new setup prepares the first Space while you set up, behind Labs (CUA-2851) ([#6360](https://github.com/trycua/cua/issues/6360)) ([2829e00](https://github.com/trycua/cua/commit/2829e00b180dfb08ca5ff4cbd7c95e0a14ad3409))
* **spaces:** warm pools, a prepared Linux Space and agent harnesses ready in about one second (behind the warm-pools experiment) ([#6472](https://github.com/trycua/cua/issues/6472)) ([d592250](https://github.com/trycua/cua/commit/d592250ea79f607f948b13de8fa557f8299d673b))
* **spaces:** Windows Spaces on nested AWS KVM hosts (windows-kvm experiment, off by default) ([#6358](https://github.com/trycua/cua/issues/6358)) ([6bdfdc5](https://github.com/trycua/cua/commit/6bdfdc53972063fa1def9cc8886d627404c53ffe))


### Bug Fixes

* **ci:** quarantine the linux-kvm concurrent flake, fix the TCC seed lock and the Windows portmap pick ([#6365](https://github.com/trycua/cua/issues/6365)) ([16d5681](https://github.com/trycua/cua/commit/16d568184853eea3ea717f67a035b43527c1315b))
* **cua-driver:** list bound tabs in the order the browser reports them ([#5527](https://github.com/trycua/cua/issues/5527)) ([59eace1](https://github.com/trycua/cua/commit/59eace173488fca6e871b5226d3d6f17120d93a8))
* **cua-relay:** mint a new service token on a 401 and refresh at half-life (CUA-2959) ([#6348](https://github.com/trycua/cua/issues/6348)) ([eed1e78](https://github.com/trycua/cua/commit/eed1e7830867fbb3f11f209979897a0ac76fa9fb))
* **cua-relay:** the per-token computer limit holds when joins race ([#6391](https://github.com/trycua/cua/issues/6391)) ([83b9449](https://github.com/trycua/cua/commit/83b94491d637cbb416844e18ee713f8e9959a3cd))
* **cua-spacesd:** drain refused /files upload bodies so the reply is not reset ([#6464](https://github.com/trycua/cua/issues/6464)) ([5e3c961](https://github.com/trycua/cua/commit/5e3c9612a1c462684cf79b88a428bb2bcddc3376))
* **cua-spacesd:** keep the doctor's token out of the probes it spawns ([#6488](https://github.com/trycua/cua/issues/6488)) ([effe7a5](https://github.com/trycua/cua/commit/effe7a5ab279cc6d02404cebd42806a7bc81c699))
* **cua-spacesd:** stream the picture when the desktop audio cannot start ([#6400](https://github.com/trycua/cua/issues/6400)) ([0571c14](https://github.com/trycua/cua/commit/0571c14d9cb4c84886b3994392683de579502340))
* **spaces:** reach macOS VMs over vsock, not Local Network or VPN routes (CUA-3139) ([#6380](https://github.com/trycua/cua/issues/6380)) ([37af9df](https://github.com/trycua/cua/commit/37af9dffdd0a79d9bcbcee0667832272b556435a))

## [0.10.1](https://github.com/trycua/cua/compare/cua-spacesd-v0.10.0...cua-spacesd-v0.10.1) (2026-11-02)


### Bug Fixes

* **cua-spacesd:** a file write on the Cua Volume mount no longer fails after the file is in place ([#6322](https://github.com/trycua/cua/issues/6322)) ([8c55aa4](https://github.com/trycua/cua/commit/8c55aa4ca8b998c0198625b4df517c5987ecfb0e))

## [0.10.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.9.0...cua-spacesd-v0.10.0) (2026-11-02)


### Features

* **spaces:** keep the Volume on all your machines, started by the daemon ([#6286](https://github.com/trycua/cua/issues/6286)) ([e6a88bf](https://github.com/trycua/cua/commit/e6a88bf12766b6ab85e4ab51aac6e5007588e747))


### Bug Fixes

* **cua-spacesd:** end driver sessions idle past their five-minute TTL ([#6284](https://github.com/trycua/cua/issues/6284)) ([f118bca](https://github.com/trycua/cua/commit/f118bca8aa80b9628344e56dbbec4dd4fd0358ab))

## [0.9.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.8.0...cua-spacesd-v0.9.0) (2026-11-01)


### Features

* **connections:** one Connections list driven by a provider registry ([#6134](https://github.com/trycua/cua/issues/6134)) ([74ff486](https://github.com/trycua/cua/commit/74ff486506e5e203b3a6fd371931ed717a405dba))
* **cua-driver:** add a niri compositor adapter for Linux Wayland ([#6209](https://github.com/trycua/cua/issues/6209)) ([33a57bb](https://github.com/trycua/cua/commit/33a57bb0a82dd31dfab0cbbb43da06d60e242ee7))
* **cua-home:** CuaChannel, the names that keep Cua Spaces Local apart from stable (CUA-1776) ([#5800](https://github.com/trycua/cua/issues/5800)) ([e896967](https://github.com/trycua/cua/commit/e896967501382d6dc26ef6d96282c0f8af882648))
* **relay:** keep path mode beside a host domain, send old share links to their origin ([#6061](https://github.com/trycua/cua/issues/6061)) ([8ed5bd3](https://github.com/trycua/cua/commit/8ed5bd3c5d041cb24428e57bceef7f5224b11374))
* **spaces:** relay counts connection outcomes (CUA-2231) ([#5900](https://github.com/trycua/cua/issues/5900)) ([1ef93e7](https://github.com/trycua/cua/commit/1ef93e74cf6b5b76c1ac0c9a6ef30fabf8997e85))


### Bug Fixes

* **cua-driver:** share the keyring history key provider in cua-driver-core ([#5895](https://github.com/trycua/cua/issues/5895)) ([e9c8478](https://github.com/trycua/cua/commit/e9c847820bcafe7559742bd32f68162754efd506))
* **cua-relay:** Spaces API errors never repeat the request URL (it can carry an account id) (CUA-2339) ([#5997](https://github.com/trycua/cua/issues/5997)) ([243fae0](https://github.com/trycua/cua/commit/243fae03ec542f25b5b2f7e9bbcc4ba0aec26937))
* **images:** claim the features cua-spacesd 0.8.0 reports on every image ([#5823](https://github.com/trycua/cua/issues/5823)) ([9fd979a](https://github.com/trycua/cua/commit/9fd979a24a6e6103b201eaf3d646d2f5c23d14ad))
* **mesh:** honour 429 Retry-After, send a client version, retire a taken-over identity; relay holds refused connects (CUA-2362) ([#6016](https://github.com/trycua/cua/issues/6016)) ([87650e1](https://github.com/trycua/cua/commit/87650e144354783f0d9cee17f48e4f91bd2fdd41))
* **relay:** a temp file pending delete on Windows counts as gone in the sweep ([#5867](https://github.com/trycua/cua/issues/5867)) ([4c9a7bb](https://github.com/trycua/cua/commit/4c9a7bb8c1b76fba5ffbbdeca5326e3e257cd2f4))
* **relay:** an expired sign-in proof names the clock ([#6248](https://github.com/trycua/cua/issues/6248)) ([189df9d](https://github.com/trycua/cua/commit/189df9d24f963d210812e473f1186d9bfd857329))
* **relay:** map recomputation and device sessions no longer stall the async runtime (CUA-2323) ([#6004](https://github.com/trycua/cua/issues/6004)) ([95bff39](https://github.com/trycua/cua/commit/95bff3975a7d417b6e06b3d4c30f63c2280bb76d))
* **relay:** signing keys refresh in the background, never on the request path (CUA-2323) ([#5993](https://github.com/trycua/cua/issues/5993)) ([83601bc](https://github.com/trycua/cua/commit/83601bca086cdf41aad251913b9135d3c65e9ed4))
* **spacesd:** let a shared Space's editors act as agents (CUA-1750) ([#5291](https://github.com/trycua/cua/issues/5291)) ([403d762](https://github.com/trycua/cua/commit/403d762ac4e712281c0a7101f20d1866a9df9ae9))
* **spaces:** dogfooding polish (welcome copy first) ([#5893](https://github.com/trycua/cua/issues/5893)) ([928fe85](https://github.com/trycua/cua/commit/928fe850d48b3a9d591ba7b18e93fd4f8db239a1))
* **spacesd:** teleport lands a Chrome session in one profile and launches the guest's Chromium (CUA-2853) ([#6180](https://github.com/trycua/cua/issues/6180)) ([dbf08f7](https://github.com/trycua/cua/commit/dbf08f753422fad92a6eef0093dfe5a5dd95f1f7))
* **spaces:** honest numbers in a Space's resources dialog ([#6058](https://github.com/trycua/cua/issues/6058)) ([9c7d8b5](https://github.com/trycua/cua/commit/9c7d8b5c4a4e945f0c2c740166a02787b13ba03f))
* **spaces:** mesh reconnects back off with jitter instead of looping (CUA-2276) ([#5932](https://github.com/trycua/cua/issues/5932)) ([b126954](https://github.com/trycua/cua/commit/b12695426c82738c227a6f1d7116617c17dcad14))
* **spaces:** never show an empty Spaces page while computers are still answering ([#5905](https://github.com/trycua/cua/issues/5905)) ([a0f98b1](https://github.com/trycua/cua/commit/a0f98b1a35dcce9aac11653efab82a9972484aad))
* **spaces:** retry 'too many requests' and say so in plain words (CUA-2275) ([#5929](https://github.com/trycua/cua/issues/5929)) ([c421b1c](https://github.com/trycua/cua/commit/c421b1caa948c3f54f72425d70c0891ac08074d0))
* **teams:** say a device waits for approval instead of "still starting" (CUA-2835) ([#6168](https://github.com/trycua/cua/issues/6168)) ([8e314e4](https://github.com/trycua/cua/commit/8e314e40971804663868359c89cda7f7cfc80b65))

## [0.8.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.7.0...cua-spacesd-v0.8.0) (2026-10-24)


### Features

* compat gateways for E2B and Daytona, Hetzner as your own cloud, the elastic pool, cua share and the public relay ([#5778](https://github.com/trycua/cua/issues/5778)) ([d4f3f80](https://github.com/trycua/cua/commit/d4f3f8063d7b30a3b3bda2b4c05c565a9dfcb460))

## [0.7.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.6.0...cua-spacesd-v0.7.0) (2026-10-21)


### Features

* **mesh:** the mesh is on by default for every client (CUA-1947) ([#5630](https://github.com/trycua/cua/issues/5630)) ([e5d09e4](https://github.com/trycua/cua/commit/e5d09e40291a080124da6469b3c7023e02e229fd))

### Bug Fixes

* **cua-driver:** the macOS WindowServer health check treats any app-bundle host as bundled, so `cua-spacesd doctor --strict` passes in the macOS Space image ([#5680](https://github.com/trycua/cua/issues/5680)) ([88b4b63](https://github.com/trycua/cua/commit/88b4b63fdf36b611bbc00c287840e821b649c193))
* **security:** distrust the compromised eMudhra intermediate CAs: rustls-webpki 0.103.16, webpki-roots 1.0.10 and the Dockerfile CA changes (CUA-1948) ([#5622](https://github.com/trycua/cua/issues/5622)) ([149ea51](https://github.com/trycua/cua/commit/149ea51c613cc840c3760dec822c57b7e65df05a))

## [0.6.0](https://github.com/trycua/cua/compare/cua-spacesd-v0.5.3...cua-spacesd-v0.6.0) (2026-10-20)


### Features

* **cua-driver:** plan cursor motion with the new cua-cursor-motion crate ([#4758](https://github.com/trycua/cua/issues/4758)) ([558cb53](https://github.com/trycua/cua/commit/558cb534d26ad51680ffd0f85b249bdebe64989a))
* **cua-driver:** plan cursor motions from the SDKs through UniFFI ([#4767](https://github.com/trycua/cua/issues/4767)) ([365f5e3](https://github.com/trycua/cua/commit/365f5e3c5b92f9457dbd560ddea8ec0268565724))
* **cua-driver:** run_script tool: sandboxed JavaScript (QuickJS) that drives the driver's own tools in one call, with wall-time, driver-call, heap and stack limits (on by default since [#4936](https://github.com/trycua/cua/issues/4936)) (CUA-1214) ([#4822](https://github.com/trycua/cua/issues/4822)) ([45469a5](https://github.com/trycua/cua/commit/45469a59c931ddccd78f9c18f1bbba4cb4cdd7be))
* **sdk:** publish cua-sdk and the proto contract so you can build your own harness (CUA-1902) ([#5532](https://github.com/trycua/cua/issues/5532)) ([78175bd](https://github.com/trycua/cua/commit/78175bd5d425c4945df8887aa308bfafd682cd2b))
* **spaces:** each computer's issues on the Machines page from its own health checks, cua doctor --machine (CUA-1820) ([#5379](https://github.com/trycua/cua/issues/5379)) ([904436a](https://github.com/trycua/cua/commit/904436adc45435959566f911290d9e56dd79c516))
* **spaces:** end-to-end encrypted connections between your machines, an opt-in beta (CUA-1505) ([#5094](https://github.com/trycua/cua/issues/5094)) ([ae2c117](https://github.com/trycua/cua/commit/ae2c117cd96673bb2dc0c2b3bff023ad70586b92))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))
* **spaces:** preflight checks before creating a local Space (CUA-1916) ([#5553](https://github.com/trycua/cua/issues/5553)) ([7bfb8e1](https://github.com/trycua/cua/commit/7bfb8e17e5a1baecf43a5682835821f7dbc1cd47))
* **spaces:** resources view, daemon resource sampler, relay summaries, CLI columns and SDK resources API (CUA-1880) ([#5469](https://github.com/trycua/cua/issues/5469)) ([5380f80](https://github.com/trycua/cua/commit/5380f800502440243ec6db76e99f731b0e853b29))
* **spaces:** second-agent and risky-action prompts for This Mac ([#5459](https://github.com/trycua/cua/issues/5459)) ([41648bc](https://github.com/trycua/cua/commit/41648bc84e65d21223db86e01075740e74480f06))


### Bug Fixes

* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))
* **sdk:** honor HTTP proxies for controller RPC ([#4795](https://github.com/trycua/cua/issues/4795)) ([23e23d4](https://github.com/trycua/cua/commit/23e23d4cc321b1e64f5832d44a150667ff0d5c04))
* **cua-driver, spacesd:** say why a Windows desktop can't be captured (locked, RDP disconnected) (CUA-1727) ([#5266](https://github.com/trycua/cua/issues/5266)) ([a0c06f2](https://github.com/trycua/cua/commit/a0c06f20239424b8a9be19468dd3b902944f8ac5))
* **cua-driver:** prevent Linux accessibility-bus deadlocks ([#4903](https://github.com/trycua/cua/issues/4903)) ([e3ad831](https://github.com/trycua/cua/commit/e3ad8319a7742f8e346d52f492b4ba1278e9c2d1))
* **cua-driver:** route browser pages to browser_prepare and refuse desktop pointer over background apps ([#5066](https://github.com/trycua/cua/issues/5066)) ([18da1bc](https://github.com/trycua/cua/commit/18da1bcbfe98e0837c130fa46b24d5473c7725e4))
* **cua-driver:** track cursor placement explicitly ([88e6d9f](https://github.com/trycua/cua/commit/88e6d9facd33e2c258e0f9a0ba4d9b2bac51cd97))
* **cua-host:** Windows unattended access without admin rights (CUA-1702) ([#5262](https://github.com/trycua/cua/issues/5262)) ([7f7114c](https://github.com/trycua/cua/commit/7f7114caa84003d0392fc5a19012d3496783e91c))
* **cua-spacesd:** connect hosted machines through HTTP proxies ([#4778](https://github.com/trycua/cua/issues/4778)) ([853e2c5](https://github.com/trycua/cua/commit/853e2c57aedb9f5573d802a8326f966985e28ef7))
* **cua-spacesd:** declare DPI awareness for Windows Host ([#4805](https://github.com/trycua/cua/issues/4805)) ([34d69bf](https://github.com/trycua/cua/commit/34d69bf2c7d9470e9f8e18d5e5f225123afcaafb))
* **cua-spacesd:** restore Windows viewer input through driver tools ([#4528](https://github.com/trycua/cua/issues/4528)) ([6b95048](https://github.com/trycua/cua/commit/6b95048b048d79ad3ce4745abe58d8e9e522144c))
* **cua-spacesd:** stream 6K displays and carry tool results over 4 MiB ([#4850](https://github.com/trycua/cua/issues/4850)) ([a88f100](https://github.com/trycua/cua/commit/a88f100b428a3eb9a73bd60757540d6ecd1fd00f)), closes [#4623](https://github.com/trycua/cua/issues/4623)
* **cua-spaces:** keep a create on another machine alive while it downloads (CUA-1760) ([#5295](https://github.com/trycua/cua/issues/5295)) ([4fe8c46](https://github.com/trycua/cua/commit/4fe8c461f600e827b9e4418a1332e7536a083ca1))
* **cua:** end an old cua daemon that never exits after an app update ([#5191](https://github.com/trycua/cua/issues/5191)) ([7bdb2a2](https://github.com/trycua/cua/commit/7bdb2a2fed02c77f50fc01942249ee3002af84a5))
* **relay:** audit log in its own file, stale temp cleanup, sessions survive restarts ([#4939](https://github.com/trycua/cua/issues/4939)) ([cfc95fd](https://github.com/trycua/cua/commit/cfc95fd70369461339e1a7af97b15b1814211c06))
* **relay:** hand over to the next relay without downtime ([#5320](https://github.com/trycua/cua/issues/5320)) ([3c8ab18](https://github.com/trycua/cua/commit/3c8ab1808bfa4b794440f22e351598e678ad58f5))
* **sdk:** delete the Fleet SDK, pools and claims from the Rust SDK, cua-sandbox, Cua Bench and the apps (CUA-1836) ([#5488](https://github.com/trycua/cua/issues/5488)) ([348c47c](https://github.com/trycua/cua/commit/348c47cc30b1d222844480838ccc4acf53f7a511))
* **security:** distrust the compromised eMudhra intermediates in every Cua client (CUA-1948) ([#5622](https://github.com/trycua/cua/issues/5622)) ([149ea51](https://github.com/trycua/cua/commit/149ea51c613cc840c3760dec822c57b7e65df05a))
* **spacesd:** ask get_window_state for structured elements in the tool backend ([#4908](https://github.com/trycua/cua/issues/4908)) ([ba4c636](https://github.com/trycua/cua/commit/ba4c6369660ab4a9c4d3d8af942bc53ad376615f))
* **spacesd:** tickets record the relay assertion time; guests' media tickets last 60 s (CUA-1416) ([#5018](https://github.com/trycua/cua/issues/5018)) ([c1b584d](https://github.com/trycua/cua/commit/c1b584d6b5319d5f4d2a3de6c4ab761f23d67af9))
* **spaces:** name the app blocked from the Local Network in cua doctor, the daemon log and Turn on (CUA-1819) ([#5378](https://github.com/trycua/cua/issues/5378)) ([075add5](https://github.com/trycua/cua/commit/075add55cf938f4e351b74539efa18d4976696ae))
* **spaces:** say computer instead of machine (CUA-1863) ([#5508](https://github.com/trycua/cua/issues/5508)) ([5cbe31c](https://github.com/trycua/cua/commit/5cbe31c7109fa70eb596b6bde3b5964449e8676e))
* stop Cua Cloud (on="cloud") with a clear "Cua Cloud has ended" error (CUA-1854) ([#5415](https://github.com/trycua/cua/issues/5415)) ([5d78ec8](https://github.com/trycua/cua/commit/5d78ec8b7705d942a3d260b276bcce76614a634c))
* Track placement explicitly in shared render state and use it across macOS, Windows, X11, and Wayland. Position getters return None for absent/unplaced cursors. Preserve existing seeding clamps and main-screen clipping. ([88e6d9f](https://github.com/trycua/cua/commit/88e6d9facd33e2c258e0f9a0ba4d9b2bac51cd97))

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
