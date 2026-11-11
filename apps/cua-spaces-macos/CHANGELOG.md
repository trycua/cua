# Changelog

## [0.33.3](https://github.com/trycua/cua/compare/cua-spaces-v0.33.2...cua-spaces-v0.33.3) (2026-11-11)


### Bug Fixes

* **spaces-macos:** bundle cua 0.24.0 in Cua Spaces ([#7329](https://github.com/trycua/cua/issues/7329)) ([ced3db7](https://github.com/trycua/cua/commit/ced3db710caeeba1ae46853436521ddf7bd62a5e))

## [0.33.2](https://github.com/trycua/cua/compare/cua-spaces-v0.33.1...cua-spaces-v0.33.2) (2026-11-11)


### Bug Fixes

* **runtime:** setup runsc no longer says already available on a runc-only Docker ([#7314](https://github.com/trycua/cua/issues/7314)) ([bc640ca](https://github.com/trycua/cua/commit/bc640ca8186aed9999f98596decf4b5a9cd9a607))

## [0.33.1](https://github.com/trycua/cua/compare/cua-spaces-v0.33.0...cua-spaces-v0.33.1) (2026-11-11)


### Bug Fixes

* **spaces:** fork a checkpoint as the kind its engine makes on a direct host ([#7318](https://github.com/trycua/cua/issues/7318)) ([972ad01](https://github.com/trycua/cua/commit/972ad01b8e0a31ad81758aef367f40b55ea4e800))

## [0.33.0](https://github.com/trycua/cua/compare/cua-spaces-v0.32.0...cua-spaces-v0.33.0) (2026-11-11)


### Features

* **spaces:** closing the window keeps Cua Spaces running, Quit stops the background service ([#7136](https://github.com/trycua/cua/issues/7136)) ([4af5875](https://github.com/trycua/cua/commit/4af587598509724ca1c716cbda5092d13794b8ea))
* **spaces:** Home shows your computers as one supercomputer (CUA-3976) ([#7166](https://github.com/trycua/cua/issues/7166)) ([49f716e](https://github.com/trycua/cua/commit/49f716e9c7e224db0f1bafbb2957be385f308e01))


### Bug Fixes

* **spaces-macos:** bundle cua 0.22.0 in Cua Spaces ([#7293](https://github.com/trycua/cua/issues/7293)) ([2808b64](https://github.com/trycua/cua/commit/2808b642accf99c0a27b9667dc62f28a978c6837))
* **spaces-macos:** bundle cua 0.23.0 in Cua Spaces ([#7302](https://github.com/trycua/cua/issues/7302)) ([61cbd6a](https://github.com/trycua/cua/commit/61cbd6a3cdf6d5a793f5b92c0de1e97d65d5755c))


### Performance Improvements

* **cua-sdk:** build libcua_sdk in its own crate so cua-sdk is rlib-only (CUA-3971) ([#7250](https://github.com/trycua/cua/issues/7250)) ([73b54ae](https://github.com/trycua/cua/commit/73b54aeb62c5c5a253be761d30f4026dea2f1fd5))

## [0.32.0](https://github.com/trycua/cua/compare/cua-spaces-v0.31.4...cua-spaces-v0.32.0) (2026-11-10)


### Features

* **spaces:** Agents tab starts agents from the Home box, one entry per Space, easy to clear ([#7275](https://github.com/trycua/cua/issues/7275)) ([5a0b65f](https://github.com/trycua/cua/commit/5a0b65f7d6af5a1c09e7f3aced03d809cbd4d521))

## [0.31.4](https://github.com/trycua/cua/compare/cua-spaces-v0.31.3...cua-spaces-v0.31.4) (2026-11-10)


### Performance Improvements

* **cua-ci:** build cua-ci beside cua-sdk, not after it (CUA-3971) ([#7240](https://github.com/trycua/cua/issues/7240)) ([1d23545](https://github.com/trycua/cua/commit/1d23545a03689800046f5fd7ecf78c3d128dec61))

## [0.31.3](https://github.com/trycua/cua/compare/cua-spaces-v0.31.2...cua-spaces-v0.31.3) (2026-11-10)


### Bug Fixes

* **spaces-macos:** bundle cua 0.21.0 in Cua Spaces ([#7241](https://github.com/trycua/cua/issues/7241)) ([9ffb9f3](https://github.com/trycua/cua/commit/9ffb9f3234af232e0cfb38ad8650f9013bc232f3))
* **spaces:** a computer's row opens its shared desktop's Space page ([#7263](https://github.com/trycua/cua/issues/7263)) ([7b8d889](https://github.com/trycua/cua/commit/7b8d889130252a72972319d4f6791b59aeb442cb))
* **spaces:** first agent on This Mac after Set up no longer fails to reach it ([#7161](https://github.com/trycua/cua/issues/7161)) ([4611678](https://github.com/trycua/cua/commit/46116784b637483e750f624ef0ef6fb74c745bde))
* **spaces:** job Spaces show as one labeled row, not as raw runs ([#7006](https://github.com/trycua/cua/issues/7006)) ([8fe112f](https://github.com/trycua/cua/commit/8fe112f91046deb571c02bd0985d9fbbcf80e4b1))
* **spaces:** the first-run test finds the permissions button by its mark, not its old label ([#7251](https://github.com/trycua/cua/issues/7251)) ([8c9e15d](https://github.com/trycua/cua/commit/8c9e15d0f3f199f856f998e83cdc23659c2bdebf))
* **volume:** a stalled sync never hangs an agent start or the mount (CUA-4002) ([#7237](https://github.com/trycua/cua/issues/7237)) ([7f4a358](https://github.com/trycua/cua/commit/7f4a35873a00a101563394f54f8497d1288a3c8b))


### Performance Improvements

* **cua-cli:** build cua-compat beside cua-cli, not before it (CUA-3971) ([#7222](https://github.com/trycua/cua/issues/7222)) ([5bf6559](https://github.com/trycua/cua/commit/5bf6559ab9942885cb5972928dd764bca5b6f7e2))
* **cua:** split cua-daemon into cua-daemon-core and the server (CUA-3971) ([#7214](https://github.com/trycua/cua/issues/7214)) ([9058fbe](https://github.com/trycua/cua/commit/9058fbe9f873c5ccc038a8350472045869275895))

## [0.31.2](https://github.com/trycua/cua/compare/cua-spaces-v0.31.1...cua-spaces-v0.31.2) (2026-11-10)


### Bug Fixes

* **cua-spaces:** read spaces.json once per connect ([#7235](https://github.com/trycua/cua/issues/7235)) ([1344c86](https://github.com/trycua/cua/commit/1344c86d1cf892002c454b5c73139b9bca62f47a))

## [0.31.1](https://github.com/trycua/cua/compare/cua-spaces-v0.31.0...cua-spaces-v0.31.1) (2026-11-10)


### Bug Fixes

* **spaces:** an offline computer reads offline within seconds, not minutes (CUA-3981) ([#7175](https://github.com/trycua/cua/issues/7175)) ([ac7d344](https://github.com/trycua/cua/commit/ac7d344d5d7e5fedb83cf8d61f50252f49f02c52))
* **spaces:** declutter the Computers page ([#7143](https://github.com/trycua/cua/issues/7143)) ([f0d57a5](https://github.com/trycua/cua/commit/f0d57a53ccc77ffc6a8d4d3b375649f7e83cbf24))
* **spaces:** new-user friction from the 10 Nov device test (CUA-3980) ([#7176](https://github.com/trycua/cua/issues/7176)) ([55730e3](https://github.com/trycua/cua/commit/55730e3f48d43038276e22c6a8cf1fbf1934d9a5))
* **spaces:** setup's step bars count the steps it really shows (CUA-3973) ([#7159](https://github.com/trycua/cua/issues/7159)) ([742d386](https://github.com/trycua/cua/commit/742d386788f43c76e7752d038dc12f9676b39717))

## [0.31.0](https://github.com/trycua/cua/compare/cua-spaces-v0.30.0...cua-spaces-v0.31.0) (2026-11-10)


### Features

* **spaces:** Child Spaces and Warm start are always on, their Labs switches are gone (CUA-3751) ([#6873](https://github.com/trycua/cua/issues/6873)) ([57942a0](https://github.com/trycua/cua/commit/57942a0e67625dffe0f79f9ea146395ff1cdac53))

## [0.30.0](https://github.com/trycua/cua/compare/cua-spaces-v0.29.0...cua-spaces-v0.30.0) (2026-11-10)


### Features

* **spaces:** agent chip shows model and reasoning level, with an inline Reasoning row ([#6861](https://github.com/trycua/cua/issues/6861)) ([bec0f80](https://github.com/trycua/cua/commit/bec0f80852f79ff8c4f6201436a10dcc29ad58d5))


### Bug Fixes

* **cua-daemon:** gate client-only code so the build without the client feature is warning-free ([#7209](https://github.com/trycua/cua/issues/7209)) ([ac6e7bf](https://github.com/trycua/cua/commit/ac6e7bff58ed7884ceb59bf6dedd74681e45e340))


### Performance Improvements

* **spaces:** install-local dev-fast by default, parallel cua-spacesd, --build-only (CUA-3971) ([#7184](https://github.com/trycua/cua/issues/7184)) ([ffb587a](https://github.com/trycua/cua/commit/ffb587a7933d535b8c2d7c11e6a6ee9e2f9fb123))

## [0.29.0](https://github.com/trycua/cua/compare/cua-spaces-v0.28.1...cua-spaces-v0.29.0) (2026-11-10)


### Features

* **cua:** show each Space's progress when spaces create makes several ([#7185](https://github.com/trycua/cua/issues/7185)) ([c4403a6](https://github.com/trycua/cua/commit/c4403a69674986ac086666b3108903cd60e1ab46))


### Bug Fixes

* **spaces-macos:** bundle cua 0.20.0 in Cua Spaces ([#7199](https://github.com/trycua/cua/issues/7199)) ([4b867b2](https://github.com/trycua/cua/commit/4b867b2dc7c03ee6db90a543a956b73d6fb35456))

## [0.28.1](https://github.com/trycua/cua/compare/cua-spaces-v0.28.0...cua-spaces-v0.28.1) (2026-11-10)


### Bug Fixes

* **spaces-desktop:** bundle the web app's packages by their exports so the Electron shell boots ([#7194](https://github.com/trycua/cua/issues/7194)) ([8ed14fe](https://github.com/trycua/cua/commit/8ed14fe24bd2d253ed4486e33e6bede9294a73bd))

## [0.28.0](https://github.com/trycua/cua/compare/cua-spaces-v0.27.3...cua-spaces-v0.28.0) (2026-11-10)


### Features

* **cua-driver:** connect --replace switches an agent from another Cua Driver ([#7190](https://github.com/trycua/cua/issues/7190)) ([e15e9dd](https://github.com/trycua/cua/commit/e15e9dd47cdc970e7cbed5fec87e377c28ddae0f))

## [0.27.3](https://github.com/trycua/cua/compare/cua-spaces-v0.27.2...cua-spaces-v0.27.3) (2026-11-10)


### Bug Fixes

* **spaces:** Restart to update quits the app when it does nothing so the update installs, and the release tests the upgrade ([#7133](https://github.com/trycua/cua/issues/7133)) ([ff264dc](https://github.com/trycua/cua/commit/ff264dc99316c41e7d1c1ad95b8f5b9578a380fd))

## [0.27.2](https://github.com/trycua/cua/compare/cua-spaces-v0.27.1...cua-spaces-v0.27.2) (2026-11-10)


### Bug Fixes

* **spaces-macos:** bundle cua 0.19.0 in Cua Spaces ([#7182](https://github.com/trycua/cua/issues/7182)) ([1cf7418](https://github.com/trycua/cua/commit/1cf7418970b7c167765262d5ac38ada91ecc514e))

## [0.27.1](https://github.com/trycua/cua/compare/cua-spaces-v0.27.0...cua-spaces-v0.27.1) (2026-11-10)


### Bug Fixes

* **cua-spaces:** hold Child Spaces on in the relay_publish opt-out test ([#7181](https://github.com/trycua/cua/issues/7181)) ([5b89d98](https://github.com/trycua/cua/commit/5b89d98fb55496c104ecac4221229bb2080ebdb9))

## [0.27.0](https://github.com/trycua/cua/compare/cua-spaces-v0.26.2...cua-spaces-v0.27.0) (2026-11-10)


### Features

* **cua:** report agent run and Space create status to the terminal (OSC 7501) ([#5155](https://github.com/trycua/cua/issues/5155)) ([f54ca3d](https://github.com/trycua/cua/commit/f54ca3de1cd7468a5ec2e74be571a80d9b2b532b))

## [0.26.2](https://github.com/trycua/cua/compare/cua-spaces-v0.26.1...cua-spaces-v0.26.2) (2026-11-10)


### Bug Fixes

* **spaces-macos:** bundle cua 0.18.1 in Cua Spaces ([#7167](https://github.com/trycua/cua/issues/7167)) ([752a41b](https://github.com/trycua/cua/commit/752a41b0df800250863eca3cd949738926236ba5))
* **spaces-macos:** bundle Cua Driver 0.46.1 in Cua Spaces ([#7162](https://github.com/trycua/cua/issues/7162)) ([1a6184f](https://github.com/trycua/cua/commit/1a6184fc45c758f434fecf0109cd8071617b5f50))

## [0.26.1](https://github.com/trycua/cua/compare/cua-spaces-v0.26.0...cua-spaces-v0.26.1) (2026-11-09)


### Bug Fixes

* **cua-vmm:** one constant for the oldest Lume cua runs on ([#6946](https://github.com/trycua/cua/issues/6946)) ([311d525](https://github.com/trycua/cua/commit/311d525ab705dcebdc82dceefa175c32d2da1e18))

## [0.26.0](https://github.com/trycua/cua/compare/cua-spaces-v0.25.2...cua-spaces-v0.26.0) (2026-11-09)


### Features

* **agents:** official sign-in on each computer, stop copying sign-ins by default (CUA-3907) ([#7045](https://github.com/trycua/cua/issues/7045)) ([5a14a01](https://github.com/trycua/cua/commit/5a14a01e43fc350dead0f5098d08958b68cff3ce))
* **spaces-macos:** install-local.sh --development for fast dev-loop builds ([#7019](https://github.com/trycua/cua/issues/7019)) ([7f9f5c2](https://github.com/trycua/cua/commit/7f9f5c20440510a999851010c00034d2afd433d1))


### Bug Fixes

* **auth:** redact tokens in Debug output ([#7071](https://github.com/trycua/cua/issues/7071)) ([#7097](https://github.com/trycua/cua/issues/7097)) ([646d5eb](https://github.com/trycua/cua/commit/646d5eb76db3ffaef409aef77986ac7d368be1a6))
* **crash:** scrub tokens before sending to Sentry ([#7098](https://github.com/trycua/cua/issues/7098)) ([0d1eb93](https://github.com/trycua/cua/commit/0d1eb93c301e6ab1a9b1492ddb1db93f11f5fe7d))
* **spaces-macos:** bundle cua 0.16.0 in Cua Spaces ([#7014](https://github.com/trycua/cua/issues/7014)) ([4e18418](https://github.com/trycua/cua/commit/4e18418b82fd70c34587406026086798be1a203f))
* **spaces-macos:** bundle Cua Driver 0.45.4 in Cua Spaces ([#7100](https://github.com/trycua/cua/issues/7100)) ([a81c82c](https://github.com/trycua/cua/commit/a81c82c1d8cd8d28bf81df3de4fa9da1322b2ba0))
* **spaces:** dogfood fixes from 8 Nov (setup, sign-in, Cua Driver, Local Network, Home) ([#7046](https://github.com/trycua/cua/issues/7046)) ([14f9e88](https://github.com/trycua/cua/commit/14f9e8845134af1c13a71e2f57e5b18565df0320))
* **spaces:** install-local reads the pinned wasm-bindgen version without the trailing semicolon ([#7123](https://github.com/trycua/cua/issues/7123)) ([1a34d4e](https://github.com/trycua/cua/commit/1a34d4ecf173a8753d0d3d439fe770d3b189b59e))

## [0.25.2](https://github.com/trycua/cua/compare/cua-spaces-v0.25.1...cua-spaces-v0.25.2) (2026-11-09)


### Bug Fixes

* **spaces-desktop:** resolve the web app's i18n dependencies in the Electron build ([#7117](https://github.com/trycua/cua/issues/7117)) ([258724d](https://github.com/trycua/cua/commit/258724de5296e70d0a4de6e403439c63bd8f9892))

## [0.25.1](https://github.com/trycua/cua/compare/cua-spaces-v0.25.0...cua-spaces-v0.25.1) (2026-11-09)


### Bug Fixes

* **cua-volume:** a saved presence heartbeat replaces the last one, and a saved change whose bytes are gone is forgotten (fixes [#6859](https://github.com/trycua/cua/issues/6859)) ([#6864](https://github.com/trycua/cua/issues/6864)) ([2349735](https://github.com/trycua/cua/commit/2349735830d4f6e8391c97557d578374e79a4ee2))

## [0.25.0](https://github.com/trycua/cua/compare/cua-spaces-v0.24.0...cua-spaces-v0.25.0) (2026-11-09)


### Features

* **spaces:** a Space uses the pool, its runs are its children and are cleaned up with it ([#6945](https://github.com/trycua/cua/issues/6945)) ([a35099d](https://github.com/trycua/cua/commit/a35099d7bcc5548d9d1b86b901d20e025a0594b9))


### Bug Fixes

* **cluster:** GPU jobs are admitted by free video memory and run on Windows hosts ([#6943](https://github.com/trycua/cua/issues/6943)) ([c62abf3](https://github.com/trycua/cua/commit/c62abf39ba407f385701b0a82a5a4d8b8eacc426))
* **cua-agents:** an agent's log event bigger than the read window no longer stalls the reader, and the journal keeps lines small ([#7004](https://github.com/trycua/cua/issues/7004)) ([8b339c5](https://github.com/trycua/cua/commit/8b339c56feb1b03252d3e72ff7a6e3d961af856f))
* **spaces:** do not Authenticode-sign the Linux microVM init in the Windows build ([#7038](https://github.com/trycua/cua/issues/7038)) ([9ec4ba3](https://github.com/trycua/cua/commit/9ec4ba39013339524bfb54d2c0aeca8bd0035559))
* **spaces:** hold back an agent's narration of its own process in the thread ([#7027](https://github.com/trycua/cua/issues/7027)) ([fa605be](https://github.com/trycua/cua/commit/fa605be7c8dc15c7042205ae98d5a85699c36a47))
* **spaces:** New agent starts in your running Space, not a machine's desktop or an offline one ([#6976](https://github.com/trycua/cua/issues/6976)) ([f2068eb](https://github.com/trycua/cua/commit/f2068eb68198973a7463041ae32cb747dacd7aa0))
* **spaces:** run Prettier on the Cua Spaces apps again ([#7073](https://github.com/trycua/cua/issues/7073)) ([6285c48](https://github.com/trycua/cua/commit/6285c481d2d867eb954a6e6c5531ba8a88182832))
* **spaces:** runs from a Space are confined children and a Space cannot start its own daemon ([#7055](https://github.com/trycua/cua/issues/7055)) ([8275285](https://github.com/trycua/cua/commit/82752858f6256eaf4e8eccf9033cf27ad3163cea))
* **spaces:** Start an agent says before Start when the agent has no provider to use ([#6977](https://github.com/trycua/cua/issues/6977)) ([e16eeb3](https://github.com/trycua/cua/commit/e16eeb3e71a5604c339c41d7770ece6da6ab69ce))

## [0.24.0](https://github.com/trycua/cua/compare/cua-spaces-v0.23.1...cua-spaces-v0.24.0) (2026-11-08)


### Features

* **spaces:** Ask first and Secure Spaces are always on, their Labs switches are gone (CUA-3551) ([#6775](https://github.com/trycua/cua/issues/6775)) ([cc67e71](https://github.com/trycua/cua/commit/cc67e715d11b7450a82663bf7e308e6680422beb))


### Bug Fixes

* **spaces-macos:** bundle Cua Driver 0.44.0 and keep it current ([#7013](https://github.com/trycua/cua/issues/7013)) ([7e8a5b1](https://github.com/trycua/cua/commit/7e8a5b16cbf6cb3599bb975f3c6d2cb05fa17989))

## [0.23.1](https://github.com/trycua/cua/compare/cua-spaces-v0.23.0...cua-spaces-v0.23.1) (2026-11-08)


### Bug Fixes

* **spaces-macos:** ask for Local Network when this Mac starts accepting Spaces from your other computers ([#7005](https://github.com/trycua/cua/issues/7005)) ([69b459b](https://github.com/trycua/cua/commit/69b459b33ca001a5e7026dee4f57da1ec02ee53f))

## [0.23.0](https://github.com/trycua/cua/compare/cua-spaces-v0.22.5...cua-spaces-v0.23.0) (2026-11-08)


### Features

* **spaces:** show "Local · Staging" or "Local · Prod" in the title bar (CUA-3814) ([#6962](https://github.com/trycua/cua/issues/6962)) ([17b37ef](https://github.com/trycua/cua/commit/17b37ef5775b4b06f0d1483bf0a9db6074a8d2ff))


### Bug Fixes

* **spaces-macos:** ask for Local Network access before a macOS Space, not when this Mac starts providing Spaces ([#6970](https://github.com/trycua/cua/issues/6970)) ([d71463d](https://github.com/trycua/cua/commit/d71463dda18b8ffc6c655b6319785c1a33425d39))
* **spaces-macos:** bundle cua 0.15.1 in Cua Spaces ([#6979](https://github.com/trycua/cua/issues/6979)) ([c34374a](https://github.com/trycua/cua/commit/c34374a9302c59d424841fbdf1c858f9c4aef7dc))
* **spaces-macos:** uninstall-local.sh also removes the Local Cua Driver login item ([#6978](https://github.com/trycua/cua/issues/6978)) ([54813bf](https://github.com/trycua/cua/commit/54813bf1f85ac76636a94b813631efa4f8988454))
* **spaces:** Ready lists only the picked agent's chats, and shows at once (CUA-3774) ([#6916](https://github.com/trycua/cua/issues/6916)) ([4392fe0](https://github.com/trycua/cua/commit/4392fe0502790f4fdd1d9b9e5bc42c1418281146))

## [0.22.5](https://github.com/trycua/cua/compare/cua-spaces-v0.22.4...cua-spaces-v0.22.5) (2026-11-08)


### Bug Fixes

* **cua-vmm:** forward the agent port of a macOS Space behind the secure gateway ([#6793](https://github.com/trycua/cua/issues/6793)) ([55eaa9b](https://github.com/trycua/cua/commit/55eaa9b6c618bbe2f723ec392436ceb9e2b4e541))
* **spaces:** start Claude agents on Sonnet 5.5 and list the Claude 5 family ([#6810](https://github.com/trycua/cua/issues/6810)) ([1511490](https://github.com/trycua/cua/commit/15114908cae2b3d70a85356651142faa1414ebf8))


### Performance Improvements

* **spaces:** connect Space desktop streams in under a second ([#6818](https://github.com/trycua/cua/issues/6818)) ([1f060b6](https://github.com/trycua/cua/commit/1f060b62491bce2fb010b64a6569bb1974decdc0))

## [0.22.4](https://github.com/trycua/cua/compare/cua-spaces-v0.22.3...cua-spaces-v0.22.4) (2026-11-07)


### Bug Fixes

* **spaces-macos:** bundle cua 0.14.0 in Cua Spaces ([#6906](https://github.com/trycua/cua/issues/6906)) ([1ef20b2](https://github.com/trycua/cua/commit/1ef20b2f9c00a3f3db7cfb2d4d18130c860d0dd0))

## [0.22.3](https://github.com/trycua/cua/compare/cua-spaces-v0.22.2...cua-spaces-v0.22.3) (2026-11-07)


### Bug Fixes

* **spaces:** Home's apps row opens again after a relaunch and shows each switch live (CUA-3551) ([#6872](https://github.com/trycua/cua/issues/6872)) ([26858cd](https://github.com/trycua/cua/commit/26858cd5050728f64aaa1ef78a9bf3a248fdccc8))

## [0.22.2](https://github.com/trycua/cua/compare/cua-spaces-v0.22.1...cua-spaces-v0.22.2) (2026-11-07)


### Bug Fixes

* **spaces:** the permission guide always docks under System Settings and says what the pane needs; the rows show each switch live (CUA-3551) ([#6870](https://github.com/trycua/cua/issues/6870)) ([a947cac](https://github.com/trycua/cua/commit/a947cac8e345966db46a11e12f42593560e8e86d))

## [0.22.1](https://github.com/trycua/cua/compare/cua-spaces-v0.22.0...cua-spaces-v0.22.1) (2026-11-07)


### Bug Fixes

* **cua-spaces-macos:** make Restart to update install the downloaded update ([#6759](https://github.com/trycua/cua/issues/6759)) ([e1c7840](https://github.com/trycua/cua/commit/e1c784079a92c0cf9b9c32ebe80f27c41863e1c5))
* **spaces-macos:** bundle cua 0.12.0 in Cua Spaces ([#6781](https://github.com/trycua/cua/issues/6781)) ([2c4374a](https://github.com/trycua/cua/commit/2c4374a0af7d6e462acd1501ecc7c29777822a76))
* **spaces-macos:** bundle cua 0.12.1 in Cua Spaces ([#6801](https://github.com/trycua/cua/issues/6801)) ([d9665a6](https://github.com/trycua/cua/commit/d9665a631f8537b4dbb58230827512208bf7e58d))
* **spaces-macos:** bundle cua 0.13.0 in Cua Spaces ([#6856](https://github.com/trycua/cua/issues/6856)) ([0c778e2](https://github.com/trycua/cua/commit/0c778e2d6b790acbda635c1b43b338314a48ef26))
* **spaces-macos:** serve the loopback control server so open_space_viewer works ([#6342](https://github.com/trycua/cua/issues/6342)) ([fea2a64](https://github.com/trycua/cua/commit/fea2a64df02ad42cf1ed909d9cb4c4bd6d311f66))
* **spaces:** every agent message is appended to the thread, child Spaces work from inside a Space, and child agents show in the chat ([#6733](https://github.com/trycua/cua/issues/6733)) ([515371a](https://github.com/trycua/cua/commit/515371a48a616aea3bba5f0ea13e0863c8a26041))
* **spaces:** say computers in use at once for the plan limit (CUA-3690) ([#6790](https://github.com/trycua/cua/issues/6790)) ([891ccb7](https://github.com/trycua/cua/commit/891ccb7153dde86f6219da719c583d2de0b88b14))

## [0.22.0](https://github.com/trycua/cua/compare/cua-spaces-v0.21.1...cua-spaces-v0.22.0) (2026-11-07)


### Features

* **spaces:** Local dev builds keep agent keys in a private file without the Keychain; install-local.sh signs with its own Keychain ([#6587](https://github.com/trycua/cua/issues/6587)) ([ffa4c06](https://github.com/trycua/cua/commit/ffa4c068ac246b015c13b30982b1f57086443b5c))


### Bug Fixes

* **spaces-macos:** bundle cua 0.11.0 in Cua Spaces ([#6747](https://github.com/trycua/cua/issues/6747)) ([36be768](https://github.com/trycua/cua/commit/36be76831b8f373642c996898546478c5ea0215e))
* **spaces:** explain a refused new computer and offer a code from another computer ([#6710](https://github.com/trycua/cua/issues/6710)) ([327f41c](https://github.com/trycua/cua/commit/327f41c89be3e8209a4bfc3b5785cb423f97de9c))

## [0.21.1](https://github.com/trycua/cua/compare/cua-spaces-v0.21.0...cua-spaces-v0.21.1) (2026-11-06)


### Bug Fixes

* **spaces-macos:** Cua Driver runs on every Mac, not only one set up for sharing (CUA-3638) ([#6704](https://github.com/trycua/cua/issues/6704)) ([8b780ae](https://github.com/trycua/cua/commit/8b780ae694c1276933c4088bc366e810d418cac8))

## [0.21.0](https://github.com/trycua/cua/compare/cua-spaces-v0.20.0...cua-spaces-v0.21.0) (2026-11-06)


### Features

* **spaces-macos:** Cua Driver built into Cua Spaces (CUA-2771, CUA-2775) ([#6505](https://github.com/trycua/cua/issues/6505)) ([b5125d2](https://github.com/trycua/cua/commit/b5125d2eaf2463b7bbac219b19fd281420213e1a))
* **spaces:** ask first, CI runners, TCP shares, secure networks and the phone app, behind experiments ([#6355](https://github.com/trycua/cua/issues/6355)) ([8cfff8e](https://github.com/trycua/cua/commit/8cfff8ef0e4a904c1534eac44cea3d65e741e6fd))
* **spaces:** Network picker with an "Also allow" list (localhost and local network are siblings) ([#6610](https://github.com/trycua/cua/issues/6610)) ([dff4d01](https://github.com/trycua/cua/commit/dff4d01d8e4fce3fa65c1c20264ea8b06605a599))
* **spaces:** permissions step in the new first run, one permission flow with the Cua Driver page (CUA-3550) ([#6579](https://github.com/trycua/cua/issues/6579)) ([0f7a384](https://github.com/trycua/cua/commit/0f7a3846b190e31a71ce03a0d7274cfa5e67c8f8))
* **spaces:** pick up where you left off in the first run (CUA-3319) ([#6463](https://github.com/trycua/cua/issues/6463)) ([a79ecf8](https://github.com/trycua/cua/commit/a79ecf8dddf225d11367360cc2dfcf6a9c135340))
* **spaces:** Spaces as a tree and a default Space, behind Labs, with the SDK bindings and CLI (CUA-3413) ([#6508](https://github.com/trycua/cua/issues/6508)) ([b6090f6](https://github.com/trycua/cua/commit/b6090f6fb2f55ba8e75a458b7dbb4fc815ef161d))
* **spaces:** Stable / Nightly update channel; betas move to Nightly (CUA-3503) ([#6548](https://github.com/trycua/cua/issues/6548)) ([53dd9d8](https://github.com/trycua/cua/commit/53dd9d868fc1c6b513257ba84f00161ddf999cc1))
* **spaces:** the new prompt box with a model and an optional repo ([#6490](https://github.com/trycua/cua/issues/6490)) ([2ea6cf3](https://github.com/trycua/cua/commit/2ea6cf34dde37afd37913566ce47ea92413c1cb3))
* **spaces:** the warming home stays out of the top bar and off the phone until a person writes into it ([#6513](https://github.com/trycua/cua/issues/6513)) ([a7f27cc](https://github.com/trycua/cua/commit/a7f27ccecbf8c6fcb0fdb4093dcb45b3eb42940d))
* **spaces:** warm pools, a prepared Linux Space and agent harnesses ready in about one second (behind the warm-pools experiment) ([#6472](https://github.com/trycua/cua/issues/6472)) ([d592250](https://github.com/trycua/cua/commit/d592250ea79f607f948b13de8fa557f8299d673b))


### Bug Fixes

* **ci:** the macOS release build installs Go for the network gateway ([#6422](https://github.com/trycua/cua/issues/6422)) ([96dd383](https://github.com/trycua/cua/commit/96dd3838ecd3084a7c7274cf83309188d4e4c94d))
* **spaces:** Cua Driver runs after the new first run, whatever the Mac shares (CUA-3485) ([#6534](https://github.com/trycua/cua/issues/6534)) ([97eabde](https://github.com/trycua/cua/commit/97eabde88dd1fdf4548dbe2d12af8396211ff4ff))

## [0.20.0](https://github.com/trycua/cua/compare/cua-spaces-v0.19.4...cua-spaces-v0.20.0) (2026-11-03)


### Features

* **spaces:** Secure Spaces, child Spaces, conformance, space jobs and the macOS developer-tools image, behind experiments ([#6306](https://github.com/trycua/cua/issues/6306)) ([4715a72](https://github.com/trycua/cua/commit/4715a723a450610cdf77f688776c1fdd40660149))

## [0.19.4](https://github.com/trycua/cua/compare/cua-spaces-v0.19.3...cua-spaces-v0.19.4) (2026-11-03)


### Bug Fixes

* **spaces-macos:** bundle cua 0.10.1 in Cua Spaces ([#6336](https://github.com/trycua/cua/issues/6336)) ([546de7b](https://github.com/trycua/cua/commit/546de7b3701cbd4628e308c5f361e774518ae1c4))
* **spaces-macos:** install a downloaded update when the app quits (CUA-3065) ([#6281](https://github.com/trycua/cua/issues/6281)) ([964e3f6](https://github.com/trycua/cua/commit/964e3f62847be2fda9c31b098aef666106975d19))
* **spaces:** plain message when a Mac cannot run Spaces, and first-run fixes (CUA-3063) ([#6323](https://github.com/trycua/cua/issues/6323)) ([7f3b4dd](https://github.com/trycua/cua/commit/7f3b4dd742d128bbd867ca20974ef08b69ccbdd9))

## [0.19.3](https://github.com/trycua/cua/compare/cua-spaces-v0.19.2...cua-spaces-v0.19.3) (2026-11-02)


### Bug Fixes

* **spaces-macos:** bundle cua 0.10.0 in Cua Spaces ([#6298](https://github.com/trycua/cua/issues/6298)) ([20281e1](https://github.com/trycua/cua/commit/20281e1f0641abf08afc221e9deccae9b681b6fb))

## [0.19.2](https://github.com/trycua/cua/compare/cua-spaces-v0.19.1...cua-spaces-v0.19.2) (2026-10-30)


### Bug Fixes

* **spaces:** show running Spaces first in the notch, never cut them for stopped ones ([#6215](https://github.com/trycua/cua/issues/6215)) ([a68e2c9](https://github.com/trycua/cua/commit/a68e2c986eb29cf70c38dcde1f9fa961af3144e6))

## [0.19.1](https://github.com/trycua/cua/compare/cua-spaces-v0.19.0...cua-spaces-v0.19.1) (2026-10-30)


### Bug Fixes

* **spaces:** one teleport setup sheet and a profile picker (CUA-2855, CUA-2853) ([#6194](https://github.com/trycua/cua/issues/6194)) ([2d974f1](https://github.com/trycua/cua/commit/2d974f1843c7343a70affba9d21e7362979edec3))

## [0.19.0](https://github.com/trycua/cua/compare/cua-spaces-v0.18.0...cua-spaces-v0.19.0) (2026-10-30)


### Features

* **connections:** per-provider model lists and a connectors.models op ([#6173](https://github.com/trycua/cua/issues/6173)) ([1976208](https://github.com/trycua/cua/commit/19762088609eeac34becee1848459ed3da75019a))
* **settings:** Providers section as compact rows, per-app Uses switcher, bundled provider marks ([#6170](https://github.com/trycua/cua/issues/6170)) ([34f12df](https://github.com/trycua/cua/commit/34f12df58fc0d9f58c0a4134f0dfaf3424ceb3fc))


### Performance Improvements

* **spaces:** fill in the Agents page as Spaces answer, bound slow machines ([#6177](https://github.com/trycua/cua/issues/6177)) ([886179f](https://github.com/trycua/cua/commit/886179f87f2610d143de9e0a24012dbb80704126))
* **spaces:** reconnect to a Space's desktop in under a second (CUA-2850) ([#6176](https://github.com/trycua/cua/issues/6176)) ([8aa59c7](https://github.com/trycua/cua/commit/8aa59c7eb48b67c0cd8e363cbdef6a9219db9b4b))

## [0.18.0](https://github.com/trycua/cua/compare/cua-spaces-v0.17.0...cua-spaces-v0.18.0) (2026-10-30)


### Features

* **spaces:** pin Spaces and shared machine desktops to the notch ([#6163](https://github.com/trycua/cua/issues/6163)) ([b269d38](https://github.com/trycua/cua/commit/b269d38db580afbd7b9f3b1d8409d0df540e1b21))


### Bug Fixes

* **cua-spaces:** say "machine" in the macOS app's last device and computer messages ([#6162](https://github.com/trycua/cua/issues/6162)) ([204acb4](https://github.com/trycua/cua/commit/204acb4e5e81fa52affad28f4e9b0b8756f7842a))

## [0.17.0](https://github.com/trycua/cua/compare/cua-spaces-v0.16.0...cua-spaces-v0.17.0) (2026-10-29)


### Features

* **connections:** one Connections list driven by a provider registry ([#6134](https://github.com/trycua/cua/issues/6134)) ([74ff486](https://github.com/trycua/cua/commit/74ff486506e5e203b3a6fd371931ed717a405dba))
* **spaces-macos:** bundle the pinned Cua Driver in Contents/Helpers (CUA-2770) ([#6144](https://github.com/trycua/cua/issues/6144)) ([a6a2713](https://github.com/trycua/cua/commit/a6a2713a25b25cca62b18b18ee3c26e48a4284b1))
* **spaces-macos:** pass creatingElsewhere through the WKWebView host (CUA-2349) ([#6012](https://github.com/trycua/cua/issues/6012)) ([4b84caa](https://github.com/trycua/cua/commit/4b84caa4053bd86be7746c5527cee3d1626612ed))
* **spaces:** drop the line under Kind; MicroVM status in Settings, Advanced (CUA-2446) ([#6048](https://github.com/trycua/cua/issues/6048)) ([a87d74c](https://github.com/trycua/cua/commit/a87d74c401778bff4a5371cbab0c751cbe8a14dc))
* **spaces:** list signed-in devices under "Your devices" on the Machines page ([#6085](https://github.com/trycua/cua/issues/6085)) ([9758f85](https://github.com/trycua/cua/commit/9758f8518b6e6b37a7bfb5d23c97378319046c0f))
* **spaces:** Sentry crash reporting for the desktop app + crash gate in auto rollout (CUA-2343) ([#6011](https://github.com/trycua/cua/issues/6011)) ([d991e3b](https://github.com/trycua/cua/commit/d991e3b4cc229881a6b5a3018956668a14b61e5b))
* **spaces:** start a run and reply to it from the app (agents.start, agents.message) (CUA-2347) ([#6005](https://github.com/trycua/cua/issues/6005)) ([06afb7c](https://github.com/trycua/cua/commit/06afb7cda6cbc80e3aadbb7122a9d06338fc4f8f))
* **spaces:** thread view with a floating mini-player on the Space page (CUA-2480) ([#6059](https://github.com/trycua/cua/issues/6059)) ([6cd675e](https://github.com/trycua/cua/commit/6cd675e8ce2c949d155a562a328070a77a7174a4))


### Bug Fixes

* **agents:** import the Claude Code sign-in instead of running claude setup-token ([#6136](https://github.com/trycua/cua/issues/6136)) ([08a2f0d](https://github.com/trycua/cua/commit/08a2f0df1cf2ac61014b8c49c700fc5c7a277cb9))
* **spaces-macos:** install-local.sh says a locked Keychain is locked, not new (CUA-2213) ([#5883](https://github.com/trycua/cua/issues/5883)) ([fd93182](https://github.com/trycua/cua/commit/fd93182fcfee4f178e881cefb8e8cb9202f83150))
* **spaces-macos:** the host service never checks in as the app (Cua Spaces could not be reopened) ([#6057](https://github.com/trycua/cua/issues/6057)) ([e337937](https://github.com/trycua/cua/commit/e3379370551ac1db9ea0345d97344424bee4b2f2))
* **spaces-macos:** the host service restarts onto an updated app (CUA-2457) ([#6054](https://github.com/trycua/cua/issues/6054)) ([dacf7e7](https://github.com/trycua/cua/commit/dacf7e70d29126074aa561cf2603b23ff9831a15))
* **spaces:** app icon keycap 20% larger inside the squircle (CUA-2369) ([#6017](https://github.com/trycua/cua/issues/6017)) ([97e96b0](https://github.com/trycua/cua/commit/97e96b00ab4f1ce78d58bf58b27ed99fddf722d0))
* **spaces:** honest numbers in a Space's resources dialog ([#6058](https://github.com/trycua/cua/issues/6058)) ([9c7d8b5](https://github.com/trycua/cua/commit/9c7d8b5c4a4e945f0c2c740166a02787b13ba03f))
* **spaces:** picture in picture hands the live desktop back; the ⋯ menu no longer blacks it out (CUA-2473) ([#6055](https://github.com/trycua/cua/issues/6055)) ([3d6be51](https://github.com/trycua/cua/commit/3d6be5173e4feb3e0b2f5c303e3196216d33d216))
* **spaces:** retry 'too many requests' and say so in plain words (CUA-2275) ([#5929](https://github.com/trycua/cua/issues/5929)) ([c421b1c](https://github.com/trycua/cua/commit/c421b1caa948c3f54f72425d70c0891ac08074d0))
* **spaces:** show a machine's shared desktop in its Machines row instead of "No Spaces" ([#6084](https://github.com/trycua/cua/issues/6084)) ([d537387](https://github.com/trycua/cua/commit/d5373879e9e0baf5a63999ac4d26902340c0ce20))


### Performance Improvements

* **spaces:** a Space starting or stopping on another computer shows within a second, pushed by the daemon ([#5915](https://github.com/trycua/cua/issues/5915)) ([74a6057](https://github.com/trycua/cua/commit/74a6057dba1bbb04ff624ef92685366307f79e2d))

## [0.16.0](https://github.com/trycua/cua/compare/cua-spaces-v0.15.0...cua-spaces-v0.16.0) (2026-10-25)


### Features

* **spaces:** link System Status from Help, Settings About and the troubleshoot guide (CUA-2228) ([#5902](https://github.com/trycua/cua/issues/5902)) ([aab2775](https://github.com/trycua/cua/commit/aab27757205bf1dba41e9ea4d91df61a22b92764))
* **spaces:** new app icon, the terracotta koala keycap ([#5914](https://github.com/trycua/cua/issues/5914)) ([ef78ec8](https://github.com/trycua/cua/commit/ef78ec83775111c9f9ed12dc97cb265d05919e4c))
* **spaces:** onboarding v2 PostHog funnel events (CUA-2292) ([#5951](https://github.com/trycua/cua/issues/5951)) ([feafec6](https://github.com/trycua/cua/commit/feafec648a406d0cffba14c0b4d84aaa29662f86))


### Bug Fixes

* **cua-cli:** Cua Spaces Local's hints and errors name cua-local, not cua ([#5892](https://github.com/trycua/cua/issues/5892)) ([5309f1e](https://github.com/trycua/cua/commit/5309f1edd49be0edd06a6f0a91c5a171f3f29be2))
* **spaces-macos:** uninstall-local.sh also removes the Local preferences, caches and web view data (CUA-2207) ([#5880](https://github.com/trycua/cua/issues/5880)) ([929be2a](https://github.com/trycua/cua/commit/929be2a6766229fceb54686f039b90ac09d275a6))
* **spaces-macos:** uninstall-local.sh takes this Mac off the relay and Local's entries out of your coding agents (CUA-2212, CUA-2222) ([#5882](https://github.com/trycua/cua/issues/5882)) ([b43f30b](https://github.com/trycua/cua/commit/b43f30bd667ebba0c188f6cce541124c3fd5703d))
* **spaces:** "Skip for now" sticks across launches, and Local's agent setup names are guarded (CUA-2221) ([#5889](https://github.com/trycua/cua/issues/5889)) ([59cf287](https://github.com/trycua/cua/commit/59cf2877722c49671ebf7b697cec706fa71ef00f))
* **spaces:** app icon gets its own charcoal background, so macOS 26 stops adding a grey plate (CUA-2244) ([#5924](https://github.com/trycua/cua/issues/5924)) ([c9a8ead](https://github.com/trycua/cua/commit/c9a8eadb93fb8215522c4716cbcfc9a014615f6a))
* **spaces:** Cua Spaces Local uses the bundle id com.trycua.spaces.macos.local ([#5884](https://github.com/trycua/cua/issues/5884)) ([152a565](https://github.com/trycua/cua/commit/152a5653b482cbfbd01520343d0f6243f1fca467))
* **spaces:** dogfooding polish (welcome copy first) ([#5893](https://github.com/trycua/cua/issues/5893)) ([928fe85](https://github.com/trycua/cua/commit/928fe850d48b3a9d591ba7b18e93fd4f8db239a1))
* **spaces:** never show an empty Spaces page while computers are still answering ([#5905](https://github.com/trycua/cua/issues/5905)) ([a0f98b1](https://github.com/trycua/cua/commit/a0f98b1a35dcce9aac11653efab82a9972484aad))
* **spaces:** sign-in that never comes back offers a code (CUA-2291) ([#5950](https://github.com/trycua/cua/issues/5950)) ([3cc240d](https://github.com/trycua/cua/commit/3cc240d9f2f2c3aefc670fc20ee915f5ec96372b))
* **spaces:** signing in as another account recovers the daemon session, host registration and mesh key (CUA-2295) ([#5961](https://github.com/trycua/cua/issues/5961)) ([3c0c264](https://github.com/trycua/cua/commit/3c0c26456d176672b5ce6880e0f80eac3b51c00e))
* **spaces:** submit again when notarytool crashes before it returns a submission id ([#5874](https://github.com/trycua/cua/issues/5874)) ([411c273](https://github.com/trycua/cua/commit/411c273b16c38652a5ec16ec98a5c110fe70711b))
* **spaces:** the notch counts only running or starting Spaces and collapses at zero ([#5872](https://github.com/trycua/cua/issues/5872)) ([3a31781](https://github.com/trycua/cua/commit/3a317816793479208da4c7b138680f9a438de6d6))
* **spaces:** the permissions guide closes itself once access is granted (CUA-2274) ([#5927](https://github.com/trycua/cua/issues/5927)) ([80146f5](https://github.com/trycua/cua/commit/80146f54f52bc7864b657a81a817a6eaeefde64c))
* **spaces:** the web page's usage signals reach the SwiftUI app before Settings is opened (CUA-2291) ([#5953](https://github.com/trycua/cua/issues/5953)) ([c3addc0](https://github.com/trycua/cua/commit/c3addc03209690fdc77911dcc46738e9070c72a8))

## [0.15.0](https://github.com/trycua/cua/compare/cua-spaces-v0.14.0...cua-spaces-v0.15.0) (2026-10-24)


### Features

* **cua-home:** CuaChannel, the names that keep Cua Spaces Local apart from stable (CUA-1776) ([#5800](https://github.com/trycua/cua/issues/5800)) ([e896967](https://github.com/trycua/cua/commit/e896967501382d6dc26ef6d96282c0f8af882648))
* **spaces:** agent-first onboarding and a quieter Home ([#5869](https://github.com/trycua/cua/issues/5869)) ([be2dad3](https://github.com/trycua/cua/commit/be2dad3345b53b6195192505a63db0538fcc035d))


### Bug Fixes

* **spaces-macos:** bundle cua 0.9.0 in Cua Spaces ([#5873](https://github.com/trycua/cua/issues/5873)) ([c74795a](https://github.com/trycua/cua/commit/c74795a3d558a67a350d0b08ce3ed5966d09eb66))
* **spaces-macos:** run cua-spacesd from Contents/Helpers so opening the app shows a window ([#5871](https://github.com/trycua/cua/issues/5871)) ([92c4cbe](https://github.com/trycua/cua/commit/92c4cbe2241190b5f5a1488ba2a94859e7e51829))

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
