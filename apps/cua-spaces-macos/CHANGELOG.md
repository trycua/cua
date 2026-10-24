# Changelog

## [0.15.0](https://github.com/trycua/cua/compare/cua-spaces-v0.14.0...cua-spaces-v0.15.0) (2026-10-24)


### Features

* **cua-home:** CuaChannel, the names that keep Cua Spaces Local apart from stable (CUA-1776) ([#5800](https://github.com/trycua/cua/issues/5800)) ([e896967](https://github.com/trycua/cua/commit/e896967501382d6dc26ef6d96282c0f8af882648))
* **spaces:** agent-first onboarding and a quieter Home ([#5869](https://github.com/trycua/cua/issues/5869)) ([be2dad3](https://github.com/trycua/cua/commit/be2dad3345b53b6195192505a63db0538fcc035d))

## [0.14.0](https://github.com/trycua/cua/compare/cua-spaces-v0.13.1...cua-spaces-v0.14.0) (2026-10-24)


### Features

* compat gateways for E2B and Daytona, Hetzner as your own cloud, the elastic pool, cua share and the public relay ([#5778](https://github.com/trycua/cua/issues/5778)) ([d4f3f80](https://github.com/trycua/cua/commit/d4f3f8063d7b30a3b3bda2b4c05c565a9dfcb460))
* **spaces:** onboarding agents step: pick any agents or none, per computer, with Codex's ChatGPT sign-in (CUA-1995) ([#5707](https://github.com/trycua/cua/issues/5707)) ([4a44f4d](https://github.com/trycua/cua/commit/4a44f4d39677c2d906d588b5b205a85aa8353354))


### Bug Fixes

* **cua:** stabilize the known flaky release-gate tests ([#5538](https://github.com/trycua/cua/issues/5538)) ([c3b557e](https://github.com/trycua/cua/commit/c3b557e1dced1238bf5b479af876aad9ea409498))
* **spaces-macos:** bundle cua 0.8.0 in Cua Spaces ([#5811](https://github.com/trycua/cua/issues/5811)) ([c9321b9](https://github.com/trycua/cua/commit/c9321b92679305f103f5ffb336e953d788b1af80))
* **spaces-macos:** restart a daemon that dies before it is watched; stable create-watchdog tests ([#5825](https://github.com/trycua/cua/issues/5825)) ([f06f203](https://github.com/trycua/cua/commit/f06f203291c6c5a12e7c9134ec6b1b918acaf5d1))

## [0.13.1](https://github.com/trycua/cua/compare/cua-spaces-v0.13.0...cua-spaces-v0.13.1) (2026-10-22)


### Bug Fixes

* **spaces-macos:** newer-Cua note with a real action, shared desktop listed first, bundle new cua in Cua Spaces (CUA-1989) ([#5702](https://github.com/trycua/cua/issues/5702)) ([b131466](https://github.com/trycua/cua/commit/b131466103eccede0c65213a59ef2d082813fb9e))
* **spaces:** show every sign-in state, with a reason and Try again ([#5708](https://github.com/trycua/cua/issues/5708)) ([c2a21e7](https://github.com/trycua/cua/commit/c2a21e764e2cc8b3a0a187aa9ca7d688265fe389))

## [0.13.0](https://github.com/trycua/cua/compare/cua-spaces-v0.12.1...cua-spaces-v0.13.0) (2026-10-22)


### Features

* **spaces-macos:** run the host service as the Cua Spaces app itself, one name in System Settings (CUA-1885) ([#5494](https://github.com/trycua/cua/issues/5494)) ([cd134c2](https://github.com/trycua/cua/commit/cd134c2431ee9c88118080b5a40a0b120eaaecd1))

## [0.12.1](https://github.com/trycua/cua/compare/cua-spaces-v0.12.0...cua-spaces-v0.12.1) (2026-10-20)


### Bug Fixes

* **spaces-macos:** document the failed-payment notice and plain plan errors ([#5632](https://github.com/trycua/cua/issues/5632)) ([fe6498d](https://github.com/trycua/cua/commit/fe6498dbfa0869a1a64f45376c0bb411fa12f34c))

## [0.12.0](https://github.com/trycua/cua/compare/cua-spaces-v0.11.0...cua-spaces-v0.12.0) (2026-10-20)


### Features

* **spaces:** core returns message codes for teleport opt-ins (CUA-1928) ([#5599](https://github.com/trycua/cua/issues/5599)) ([12720d6](https://github.com/trycua/cua/commit/12720d6ab44e99550728d3b54500004b85183046))
* **spaces:** core returns message codes for the sidebar (CUA-1928) ([#5608](https://github.com/trycua/cua/issues/5608)) ([4cddeec](https://github.com/trycua/cua/commit/4cddeec8308871eedb0f26e57f76f3fff065505c))
* **spaces:** each computer's issues on the Machines page from its own health checks, cua doctor --machine (CUA-1820) ([#5379](https://github.com/trycua/cua/issues/5379)) ([904436a](https://github.com/trycua/cua/commit/904436adc45435959566f911290d9e56dd79c516))

## [0.11.0](https://github.com/trycua/cua/compare/cua-spaces-v0.10.0...cua-spaces-v0.11.0) (2026-10-20)


### Features

* **spaces:** "remote hours" everywhere users see it; self-hosting under Settings → Advanced ([#5461](https://github.com/trycua/cua/issues/5461)) ([b9f1874](https://github.com/trycua/cua/commit/b9f1874ba871a13b9e8efc772cb39472a9b0dd58))
* **spaces:** one first run everywhere: connect your agent, then This Mac is your first Space (0.10.0) ([#5462](https://github.com/trycua/cua/issues/5462)) ([c4b4b09](https://github.com/trycua/cua/commit/c4b4b0922f7f0c4d87982a972eea0a0c073e5883))
* **spaces:** processor, memory and disk on Home, a Space's page and the menu bar (CUA-1880) ([#5458](https://github.com/trycua/cua/issues/5458)) ([4690709](https://github.com/trycua/cua/commit/46907096b3511b13db00150f197647800871a72d))
* **spaces:** resources view, daemon resource sampler, relay summaries, CLI columns and SDK resources API (CUA-1880) ([#5469](https://github.com/trycua/cua/issues/5469)) ([5380f80](https://github.com/trycua/cua/commit/5380f800502440243ec6db76e99f731b0e853b29))
* **spaces:** second-agent and risky-action prompts for This Mac ([#5459](https://github.com/trycua/cua/issues/5459)) ([41648bc](https://github.com/trycua/cua/commit/41648bc84e65d21223db86e01075740e74480f06))
* **telemetry:** link signed-in usage data to an opaque account id, off until the policy is published (CUA-1881) ([#5472](https://github.com/trycua/cua/issues/5472)) ([911fc16](https://github.com/trycua/cua/commit/911fc16042b2f7327947f47dd935095e174e6b1c))


### Bug Fixes

* **sdk:** delete the Fleet SDK, pools and claims from the Rust SDK, cua-sandbox, Cua Bench and the apps (CUA-1836) ([#5488](https://github.com/trycua/cua/issues/5488)) ([348c47c](https://github.com/trycua/cua/commit/348c47cc30b1d222844480838ccc4acf53f7a511))
* **spaces:** honest Done screen that sees the CLI and sign-in you already have (CUA-1862) ([#5447](https://github.com/trycua/cua/issues/5447)) ([bea1cfd](https://github.com/trycua/cua/commit/bea1cfd3f55b36af6f1d3d290e67d316dfecf5ff))
* **spaces:** Linux Share in remote desktops asks for the password, says what to do when it cannot, and cua_share says why it failed (CUA-1725, CUA-1855) ([#5273](https://github.com/trycua/cua/issues/5273)) ([522e032](https://github.com/trycua/cua/commit/522e032fb3ac340aca1f0e1301413d7f08839e31))
* **spaces:** say computer instead of machine (CUA-1863) ([#5508](https://github.com/trycua/cua/issues/5508)) ([5cbe31c](https://github.com/trycua/cua/commit/5cbe31c7109fa70eb596b6bde3b5964449e8676e))

## [0.10.0](https://github.com/trycua/cua/compare/cua-spaces-v0.9.2...cua-spaces-v0.10.0) (2026-10-19)


### Features

* **spaces:** join with a code and a cua://add deep link (CUA-1772) ([#5316](https://github.com/trycua/cua/issues/5316)) ([c195318](https://github.com/trycua/cua/commit/c195318010ab05638076ad22d8bd53d0e2dd7ac1))
* **spaces:** T3-style in-app updates on macOS, Windows and Linux (CUA-1780) ([#5326](https://github.com/trycua/cua/issues/5326)) ([a6de100](https://github.com/trycua/cua/commit/a6de100971b217395cad6445f5ec547cdef5c3e8))


### Bug Fixes

* **cua-agent-setup:** set up Claude Code on Windows from the app (CUA-1703) ([#5261](https://github.com/trycua/cua/issues/5261)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-auth:** never wait on a macOS keychain prompt without a terminal ([#5427](https://github.com/trycua/cua/issues/5427)) ([04f75fa](https://github.com/trycua/cua/commit/04f75fa0d34c842e6e204ff69670ae49a397640e))
* **cua-cli:** open the sign-in URL on Windows without cmd.exe (CUA-1701) ([#5260](https://github.com/trycua/cua/issues/5260)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-driver, spacesd:** say why a Windows desktop can't be captured when locked or RDP is disconnected (CUA-1727) ([#5266](https://github.com/trycua/cua/issues/5266)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-host:** Windows unattended access without admin rights (CUA-1702) ([#5262](https://github.com/trycua/cua/issues/5262)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-spaces:** install or start Docker Desktop from New Space on Windows (CUA-1723) ([#5265](https://github.com/trycua/cua/issues/5265)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-spaces:** keep a create on another machine alive while it downloads (CUA-1760) ([#5295](https://github.com/trycua/cua/issues/5295)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **cua-spaces:** onboarding This machine Continue button, launch at login default, static prompts ([#5317](https://github.com/trycua/cua/issues/5317)) ([861c4b0](https://github.com/trycua/cua/commit/861c4b0f9e74fc51935333692374b8bb4d8618da))
* **cua-spaces:** sign-in completes after a retried browser sign-in (CUA-1728) ([#5268](https://github.com/trycua/cua/issues/5268)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **install.ps1:** put cua on PATH and install the app by default on Windows (CUA-1704) ([#5263](https://github.com/trycua/cua/issues/5263)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **spaces-desktop:** send first-run, sign-in and Space events on Windows and Linux (CUA-1739) ([#5277](https://github.com/trycua/cua/issues/5277)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **spaces-desktop:** uninstall stops and removes the Windows host service (CUA-1740) ([#5276](https://github.com/trycua/cua/issues/5276)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **spaces:** prefill the computer name and explain host setup in plain words (CUA-1862) ([#5440](https://github.com/trycua/cua/issues/5440)) ([9215d41](https://github.com/trycua/cua/commit/9215d41ae20f92e80fdd867f5ca07ca2d36e6137))
* **spaces:** rc.2 smoke fixes: Continue on This machine, example prompts, update defaults, plain errors (CUA-1816) ([#5367](https://github.com/trycua/cua/issues/5367)) ([9fd19f3](https://github.com/trycua/cua/commit/9fd19f31db4d05196e7ea2775063bb2b1322b607))
* **spaces:** refuse a Windows VM create without QEMU before the 6 GB pull (CUA-1761) ([#5294](https://github.com/trycua/cua/issues/5294)) ([d6cb68a](https://github.com/trycua/cua/commit/d6cb68a27d2226ca5e412e04aded97fbc5f31812))
* **spaces:** say why a Space stopped on its own (out of memory) (CUA-1759) ([#5303](https://github.com/trycua/cua/issues/5303)) ([9737d6f](https://github.com/trycua/cua/commit/9737d6f22bad592038335fd156d1e02766fe768b))
* stop Cua Cloud (on="cloud") with a clear "Cua Cloud has ended" error (CUA-1854) ([#5415](https://github.com/trycua/cua/issues/5415)) ([5d78ec8](https://github.com/trycua/cua/commit/5d78ec8b7705d942a3d260b276bcce76614a634c))

## [0.9.2](https://github.com/trycua/cua/compare/cua-spaces-v0.9.1...cua-spaces-v0.9.2) (2026-10-15)


### Bug Fixes

* **spaces:** one-time migration turns Space sharing on for 0.9.0 settings (CUA-1668) ([#5225](https://github.com/trycua/cua/issues/5225)) ([749636a](https://github.com/trycua/cua/commit/749636a5df27e773aaeee03baf083e9761a7f99d))

## [0.9.1](https://github.com/trycua/cua/compare/cua-spaces-v0.9.0...cua-spaces-v0.9.1) (2026-10-15)


### Bug Fixes

* **cua-spaces:** show a create's real stages and download progress, also on another machine ([#5198](https://github.com/trycua/cua/issues/5198)) ([13facff](https://github.com/trycua/cua/commit/13facfffdd95927ebac4218f10b8275d69f40759))
* **cua-spaces:** typed resource values, the generated Space name, and picking saved passwords ([#5205](https://github.com/trycua/cua/issues/5205)) ([a14eadd](https://github.com/trycua/cua/commit/a14eadd3b8a563dc4b73d58ff07cd4a2df1a9dc1))
* **spaces:** follow window drags when the notch setting turns on live (CUA-1618) ([#5189](https://github.com/trycua/cua/issues/5189)) ([29598e2](https://github.com/trycua/cua/commit/29598e25ba607bfb65e80c4acd49b8c4696edd69))
* **spaces:** list only Spaces in the notch, never the machines that provide them ([#5190](https://github.com/trycua/cua/issues/5190)) ([1ed129f](https://github.com/trycua/cua/commit/1ed129f64eceaa97cb9f0a082d91512f58f523a7))
* **spaces:** one machine state in both UIs, from the core (CUA-1617) ([#5188](https://github.com/trycua/cua/issues/5188)) ([f820274](https://github.com/trycua/cua/commit/f8202745a66a844bbafc4811878f3a43da0c36fb))
* **spaces:** open a notification to read it in full (CUA-1636) ([#5196](https://github.com/trycua/cua/issues/5196)) ([4582261](https://github.com/trycua/cua/commit/45822615665bb09585b7d0aa1faaaca99f65ca11))
* **spaces:** Teleport's review says what happens for "Just the app" (CUA-1616) ([#5187](https://github.com/trycua/cua/issues/5187)) ([a8d9f38](https://github.com/trycua/cua/commit/a8d9f38ff49f5f0ed8c5fd9681945f3aa97491ae))
* **spaces:** turn Space sharing on by default ([#5218](https://github.com/trycua/cua/issues/5218)) ([8049050](https://github.com/trycua/cua/commit/8049050559f77647f08c78fc276a70f19a660a7f))
* **spaces:** Upgrade falls back to the account app's billing page (CUA-1515) ([#5128](https://github.com/trycua/cua/issues/5128)) ([4b37441](https://github.com/trycua/cua/commit/4b37441420ca4c5fe09ac4fee575e9db221b410b))

## [0.9.0](https://github.com/trycua/cua/compare/cua-spaces-v0.8.0...cua-spaces-v0.9.0) (2026-10-14)


### Features

* **spaces:** drag a window onto a Space tile in the New UI to teleport it (CUA-1511) ([#5099](https://github.com/trycua/cua/issues/5099)) ([f157d9c](https://github.com/trycua/cua/commit/f157d9c2a91657ba5afd9ba07b34501629951bbe))
* **spaces:** New UI on by default for Beta (0.8.1), saved only as an explicit choice (CUA-1514) ([#5108](https://github.com/trycua/cua/issues/5108)) ([3eb7c09](https://github.com/trycua/cua/commit/3eb7c09ce3cd343cf4e1d5b2dba7ce49c6e47989))
* **spaces:** real Teams screen, api.cua.ai and app.cua.ai (0.8.1) ([#5113](https://github.com/trycua/cua/issues/5113)) ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))


### Bug Fixes

* **spaces:** drop the last run.cua.ai billing link from the Swift fixture ([8c02482](https://github.com/trycua/cua/commit/8c02482e3d10e67f1b7a084aa83ecb5b4eae03c4))
* **spaces:** same checklist count and every machine in every UI ([#5106](https://github.com/trycua/cua/issues/5106)) ([ff6079b](https://github.com/trycua/cua/commit/ff6079bef5b6089a5ef649c9447e441c72c9519b))
* **spaces:** Teams clients use Keycloak orgs via the Spaces API ([#5157](https://github.com/trycua/cua/issues/5157)) ([49a89ff](https://github.com/trycua/cua/commit/49a89ff38a06e5b17413d52d2cf6534a13cdba7f))
* **spaces:** the apps stop calling the retired run.cua.ai (CUA-1515) ([#5098](https://github.com/trycua/cua/issues/5098)) ([499130b](https://github.com/trycua/cua/commit/499130b2cb1394e973eafd286b4377beec847ab5))

## [0.8.0](https://github.com/trycua/cua/compare/cua-spaces-v0.7.2...cua-spaces-v0.8.0) (2026-10-13)


### Features

* **cua:** CUA_ENV=staging targets the Cua staging deployment (CUA-1395) ([#5013](https://github.com/trycua/cua/issues/5013)) ([72f329d](https://github.com/trycua/cua/commit/72f329d85246c42458b900d6b21b8cd34b830d05))
* **spaces-macos:** Machines page, Get started checklist, ⌘K palette, ⌘1-4 (CUA-1412) ([#5035](https://github.com/trycua/cua/issues/5035)) ([5b4a16a](https://github.com/trycua/cua/commit/5b4a16a522c71a3d154cea251b87225750a17fdc))
* **spaces-macos:** open the New UI from the notch and menu bar (CUA-1407) ([#5031](https://github.com/trycua/cua/issues/5031)) ([e088b19](https://github.com/trycua/cua/commit/e088b19c3c68b1a863a4af190f1d7ba20d6791b1))
* **spaces-macos:** passphrase setup and unlock in the New UI (CUA-1411) ([#5032](https://github.com/trycua/cua/issues/5032)) ([f982ea0](https://github.com/trycua/cua/commit/f982ea064bb1f6e29f6f2004e5021407f7006320))
* **spaces-macos:** run conversation timeline in the SwiftUI app (CUA-1413) ([#5028](https://github.com/trycua/cua/issues/5028)) ([75a81cf](https://github.com/trycua/cua/commit/75a81cf81629408f428139374b161d4c078eb708))
* **spaces-web:** persistent agent Memory, Routines and Access in the React UI (CUA-1408) ([#5033](https://github.com/trycua/cua/issues/5033)) ([541abc5](https://github.com/trycua/cua/commit/541abc55c64bdce0832e43d1002d6571407c098f))
* **spaces:** add a machine by link or QR code in the app (CUA-1358) ([#5006](https://github.com/trycua/cua/issues/5006)) ([a10b913](https://github.com/trycua/cua/commit/a10b91323d7082a2bb28fce3eb77c68dbcf9d45a))
* **spaces:** empty home with one-click Linux and macOS create ([#4870](https://github.com/trycua/cua/issues/4870)) ([2e4736b](https://github.com/trycua/cua/commit/2e4736b3ebff61ef99e8c0c74270b5cd75894643))
* **spaces:** new shared UI — React web UI in the Mac app and an Electron app (preview) ([#4893](https://github.com/trycua/cua/issues/4893)) ([7cc4bd0](https://github.com/trycua/cua/commit/7cc4bd0bfc3ce1a3246ae47fe02948dc9f64f010))
* **spaces:** presence cursors over the React video and window-drag teleport in Electron (CUA-1414) ([#5030](https://github.com/trycua/cua/issues/5030)) ([a77bd52](https://github.com/trycua/cua/commit/a77bd52c0e6132da280f8db61e2f4afa24105648))
* **spaces:** register the ⌘⇧Space global shortcut on macOS (CUA-1415) ([#5026](https://github.com/trycua/cua/issues/5026)) ([41047e2](https://github.com/trycua/cua/commit/41047e23d469b4e9b7dfd32331210692746cd8d7))
* **spaces:** relay usage row and allowance notice on the Mac app (CUA-1410) ([#5034](https://github.com/trycua/cua/issues/5034)) ([a329fcf](https://github.com/trycua/cua/commit/a329fcf435191655a8dc8271c0df9ea824c2b564))


### Bug Fixes

* **cua-spaces-macos:** ask before installing cua and adding it to PATH ([#5039](https://github.com/trycua/cua/issues/5039)) ([80ae9a4](https://github.com/trycua/cua/commit/80ae9a47ad9998a819bd5636b84e7eccb4a15841))
* **macos:** offer retry when stream provider creation fails ([#4807](https://github.com/trycua/cua/issues/4807)) ([8154096](https://github.com/trycua/cua/commit/81540962dbfbb0f9566cef8607288f3afdf65dd2))
* **macos:** refresh open detail after device approval ([#4794](https://github.com/trycua/cua/issues/4794)) ([59e372e](https://github.com/trycua/cua/commit/59e372ec6b29f05b3ae107e7c405fbf8f1fb81d1))
* **spaces-macos:** hit-test the notch tab as drawn, not at its widest ([#4611](https://github.com/trycua/cua/issues/4611)) ([a0394a6](https://github.com/trycua/cua/commit/a0394a648bc88af50c26d2a4c9e270186096e82a)), closes [#4610](https://github.com/trycua/cua/issues/4610)
* **spaces-macos:** keep the notch panel's search inside the glass panel ([#4847](https://github.com/trycua/cua/issues/4847)) ([384f0b3](https://github.com/trycua/cua/commit/384f0b3b28097a4aee90fb848f5348061e345fde)), closes [#4644](https://github.com/trycua/cua/issues/4644)
* **spaces-macos:** New UI (preview) replaces the main window instead of adding one ([#5054](https://github.com/trycua/cua/issues/5054)) ([d35d1cc](https://github.com/trycua/cua/commit/d35d1ccd08b266af28149f1afd200eeffbf2bf76))
* **spaces-macos:** open the viewer when ready, let the Space toolbar overflow, and unclip onboarding ([#5016](https://github.com/trycua/cua/issues/5016)) ([0f92100](https://github.com/trycua/cua/commit/0f921000ba7721ffc7be8b563dbb96ef8b3d543c))
* **spaces-macos:** send Command chords to the Space while the viewer has the keyboard ([#4848](https://github.com/trycua/cua/issues/4848)) ([f2b32eb](https://github.com/trycua/cua/commit/f2b32eb2d854cea6d0aa450e44fbac9983b063f6)), closes [#4609](https://github.com/trycua/cua/issues/4609)
* **spaces-macos:** stack empty-home tiles when the detail pane is narrow ([#4912](https://github.com/trycua/cua/issues/4912)) ([5274342](https://github.com/trycua/cua/commit/5274342fbf66ca2998325be5e900299406ca8650))
* **spaces:** leave the machines that provide Spaces out of the menu bar count ([#5050](https://github.com/trycua/cua/issues/5050)) ([97b2394](https://github.com/trycua/cua/commit/97b2394fc1173588e04af6901385a167d7f89029))
* **spaces:** preserve live OS metadata and show unknown honestly ([#4671](https://github.com/trycua/cua/issues/4671)) ([f25fe36](https://github.com/trycua/cua/commit/f25fe3691e651cc8ac6adf92931940ed25d9a6bb))
* **spaces:** report directory refresh failures ([#4716](https://github.com/trycua/cua/issues/4716)) ([1f96a4b](https://github.com/trycua/cua/commit/1f96a4bf0e3f02dcf79ab83d78a4fc9c017d24ff))
* **spaces:** show the runtime-inclusive Linux disk footprint ([#5038](https://github.com/trycua/cua/issues/5038)) ([a3f9a25](https://github.com/trycua/cua/commit/a3f9a2595f5358b78d3ba03a7657ac127ef43f2e))

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
