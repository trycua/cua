# Changelog

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
