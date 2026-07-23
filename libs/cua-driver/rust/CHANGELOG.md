# Changelog

## [0.12.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.1...cua-driver-rs-v0.12.2) (2026-07-23)


### Bug Fixes

* **cua-driver:** tolerate fileno-less Python stdio ([#2482](https://github.com/trycua/cua/issues/2482)) ([d041f1a](https://github.com/trycua/cua/commit/d041f1a580d413bbe5da6645fd58b9088b2cdd4a))
* **cua-driver:** trust native macOS sentinel focus ([#2481](https://github.com/trycua/cua/issues/2481)) ([6c2bbcc](https://github.com/trycua/cua/commit/6c2bbccfe5ad6690ef0961c492b34a3493354e84))

## [0.12.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.0...cua-driver-rs-v0.12.1) (2026-07-23)


### Bug Fixes

* **cua-driver:** make macOS SDK load standalone ([#2477](https://github.com/trycua/cua/issues/2477)) ([457b17e](https://github.com/trycua/cua/commit/457b17eae8ea2d2ca1b45ad5b23fc342787769a7))

## [0.12.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.11.0...cua-driver-rs-v0.12.0) (2026-07-22)


### Features

* **cua-driver:** add client and modality telemetry ([#2441](https://github.com/trycua/cua/issues/2441)) ([5cd0000](https://github.com/trycua/cua/commit/5cd0000e2e018a835303a8f8bdd82a88ea483a6e))
* **cua-driver:** add true in-process SDK runtime ([#2461](https://github.com/trycua/cua/issues/2461)) ([617508a](https://github.com/trycua/cua/commit/617508a7ae123f277b203d31fca29933927a4636))
* **cua-driver:** add versioned native ABI ([b172afc](https://github.com/trycua/cua/commit/b172afc75f39832a4bdfaa9040d6d4f556449b49))
* **cua-driver:** expose macOS control state in window elements ([db281a3](https://github.com/trycua/cua/commit/db281a3b0d9bbabaf74f3c6847adf6abbb66995a))


### Bug Fixes

* **cua-driver:** advertise delivery mode capability ([7f6657c](https://github.com/trycua/cua/commit/7f6657c440cedf1f20c6307c849a89487f867b41)), closes [#2425](https://github.com/trycua/cua/issues/2425)
* **cua-driver:** avoid SIGCHLD handler in doctor ([31bbc07](https://github.com/trycua/cua/commit/31bbc074d75f59fe2c417cddc66cf4093bd8330b)), closes [#2348](https://github.com/trycua/cua/issues/2348)
* **cua-driver:** enforce scope for mouse button actions ([caa241b](https://github.com/trycua/cua/commit/caa241be5f801e2060d4f5823fde64f9e6dc2be4)), closes [#2368](https://github.com/trycua/cua/issues/2368)
* **cua-driver:** support Electron UniFFI buffers ([#2455](https://github.com/trycua/cua/issues/2455)) ([d3bf82a](https://github.com/trycua/cua/commit/d3bf82a2cbb27fd83cfa836c04f9f5678e3e3aad))

## [0.11.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.10.0...cua-driver-rs-v0.11.0) (2026-07-22)


### ⚠ BREAKING CHANGES

* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341))

### Features

* **cua-driver:** add persistent macOS interactive input sessions ([2dad3e5](https://github.com/trycua/cua/commit/2dad3e519e17b27eaa793151b8671957f578072c))
* **cua-driver:** add Rust-owned embedded host for SDK and MCP ([#2427](https://github.com/trycua/cua/issues/2427)) ([5016dc1](https://github.com/trycua/cua/commit/5016dc16bfe54c165e6678104ec521a6d85f76db))
* **cua-driver:** capture inactive tabs and retain modal controls ([#2426](https://github.com/trycua/cua/issues/2426)) ([c4d7ddc](https://github.com/trycua/cua/commit/c4d7ddc5bc7c00faf3e9102bee664ea47b2f5fac))
* **cua-driver:** replace language MCP clients with Rust SDKs ([#2341](https://github.com/trycua/cua/issues/2341)) ([b8a0f32](https://github.com/trycua/cua/commit/b8a0f32a06c75225ba24ebb5ab14f6507fa90d15))
* **cua-driver:** track update funnel telemetry ([#2392](https://github.com/trycua/cua/issues/2392)) ([994308a](https://github.com/trycua/cua/commit/994308a96649109c4e6334acba7179acc8542155))


### Bug Fixes

* **cua-driver:** classify health_report as read-only diagnostics ([#2408](https://github.com/trycua/cua/issues/2408)) ([16e7573](https://github.com/trycua/cua/commit/16e7573363adaf3cb6abcfbe8ebf04b7172ff160)), closes [#2399](https://github.com/trycua/cua/issues/2399)
* **cua-driver:** make macOS permission consent explicit and durable ([#2407](https://github.com/trycua/cua/issues/2407)) ([7ac41be](https://github.com/trycua/cua/commit/7ac41be61e6b1c0034cd9d4de48ac2d5f7bf6b13))
* **cua-driver:** request macOS permissions from running daemon ([#2414](https://github.com/trycua/cua/issues/2414)) ([231839d](https://github.com/trycua/cua/commit/231839de8161c85b63b86f8a54678a0cd9816335))
* **cua-driver:** separate local and release identities ([#2404](https://github.com/trycua/cua/issues/2404)) ([a8d8142](https://github.com/trycua/cua/commit/a8d8142bc1dfef641cae80680e0aa9c9230fffb8))

## [0.10.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.9.1...cua-driver-rs-v0.10.0) (2026-07-20)


### Features

* **cua-driver:** add protected permission modes and consent grants ([#2383](https://github.com/trycua/cua/issues/2383)) ([c75e606](https://github.com/trycua/cua/commit/c75e60636c11e21ef44f1ebbe1c1350339bae295))


### Bug Fixes

* **cua-driver:** bound tool telemetry volume ([#2382](https://github.com/trycua/cua/issues/2382)) ([99d4d61](https://github.com/trycua/cua/commit/99d4d613abc35a67753d206fcf3371673fb75edd))
* **cua-driver:** register bundle in LaunchServices before tccutil reset ([#2376](https://github.com/trycua/cua/issues/2376)) ([767acf2](https://github.com/trycua/cua/commit/767acf25f25ea668ffab428e4f2e8985896de98e))

## [0.9.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.9.0...cua-driver-rs-v0.9.1) (2026-07-20)


### Bug Fixes

* **cua-driver:** fail closed for unsupported browser and Wayland routes ([#2367](https://github.com/trycua/cua/pull/2367))
* **cua-driver:** harden exact-profile browser attachment across Chrome and Edge ([#2367](https://github.com/trycua/cua/pull/2367))
* **cua-driver:** restore focus after bounded browser setup ([#2367](https://github.com/trycua/cua/pull/2367))

## [0.9.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.3...cua-driver-rs-v0.9.0) (2026-07-19)


### Features

* **cua-driver-rs:** add Linux arm64 (aarch64) prebuilt support ([#1948](https://github.com/trycua/cua/issues/1948)) ([79884bd](https://github.com/trycua/cua/commit/79884bde1ea483affc907f2b6b264382023dbf17))
* **cua-driver-rs:** become own responsible process for true TCC status ([#1956](https://github.com/trycua/cua/issues/1956)) ([d61ce01](https://github.com/trycua/cua/commit/d61ce0197ee4f396220e4094da05745825a7b440))
* **cua-driver-rs:** caller-declared session identity + Streamable-HTTP transport for multi-agent parallelism ([#1798](https://github.com/trycua/cua/issues/1798)) ([e1e8c98](https://github.com/trycua/cua/commit/e1e8c98208f64c0e12d8683647091b1f264a7613))
* **cua-driver-rs:** check-update CLI verb + check_for_update MCP tool ([#1734](https://github.com/trycua/cua/issues/1734)) ([7d893ce](https://github.com/trycua/cua/commit/7d893ce827fe0d3730c696cef7eae70d98602bed))
* **cua-driver-rs:** daemon session identity — own + clean up session-scoped recording & config ([#1776](https://github.com/trycua/cua/issues/1776)) ([748e74a](https://github.com/trycua/cua/commit/748e74a84ef0581c98ff95e4a40b98ab28b07dae))
* **cua-driver-rs:** wire up --claude-code-computer-use-compat + make it default for claude ([#1678](https://github.com/trycua/cua/issues/1678)) ([ec07873](https://github.com/trycua/cua/commit/ec0787363da03595381b62c6a4781b1a0fcd955c))
* **cua-driver/macos:** implement page click_element + fix Chromium window targeting ([#2082](https://github.com/trycua/cua/issues/2082)) ([73fe822](https://github.com/trycua/cua/commit/73fe8227982c0b03795669e4cdf62384f0d54a34))
* **cua-driver:** add aggregate agent session telemetry ([aa41707](https://github.com/trycua/cua/commit/aa417076c7d31df9d78dc72e982b48b5506e1695))
* **cua-driver:** add aggregate agent session telemetry ([3a59246](https://github.com/trycua/cua/commit/3a5924640738e9881efc11a80b6e26ed6ef5ed0a))
* **cua-driver:** add bounded feature telemetry ([4972560](https://github.com/trycua/cua/commit/4972560226ea3ff81c5d07ca3e867fbaa5238f69))
* **cua-driver:** add bounded feature telemetry ([ebfb4eb](https://github.com/trycua/cua/commit/ebfb4eb7fa48e028d4a64ec94ddda2f466d8313d))
* **cua-driver:** add browser dialogs and uploads ([21ca689](https://github.com/trycua/cua/commit/21ca68928e546103a59ad851699a50e543ff601b))
* **cua-driver:** add browser telemetry contract ([fa13890](https://github.com/trycua/cua/commit/fa138903f779b33b4c8512c7d64bb960023161fe))
* **cua-driver:** add browser telemetry contract ([#2310](https://github.com/trycua/cua/issues/2310)) ([763a6ea](https://github.com/trycua/cua/commit/763a6ea21b86ad5f8a70ab2581edee1dd7370efa))
* **cua-driver:** add capability-aware browser tools ([#2257](https://github.com/trycua/cua/issues/2257)) ([0835daa](https://github.com/trycua/cua/commit/0835daa6d415c857c1d3ebefe2c29453a0bed923))
* **cua-driver:** add semantic browser snapshots ([1a9fbb4](https://github.com/trycua/cua/commit/1a9fbb480605f72c449d2c741c19ae32ed5659d8))
* **cua-driver:** add semantic browser snapshots ([#2301](https://github.com/trycua/cua/issues/2301)) ([e83d2d3](https://github.com/trycua/cua/commit/e83d2d3d6142fee78edca6352790db4336abacbb))
* **cua-driver:** add stable health_report MCP tool for end-to-end driver diagnostics ([be761fa](https://github.com/trycua/cua/commit/be761fac796d3f266d56ed7ce89c5a5ff6a89eac))
* **cua-driver:** complete browser mutations ([b4007a8](https://github.com/trycua/cua/commit/b4007a8442d3f3238226b5d5d11feda8da13c0af))
* **cua-driver:** complete telemetry lifecycle coverage ([f06b7cf](https://github.com/trycua/cua/commit/f06b7cf26f44721edd968f1dd072eb395212dd10))
* **cua-driver:** complete telemetry lifecycle coverage ([7ac5836](https://github.com/trycua/cua/commit/7ac5836714e99f945e13e840d400f0a637edf56d))
* **cua-driver:** complete the browser action surface ([#2323](https://github.com/trycua/cua/issues/2323)) ([01a9505](https://github.com/trycua/cua/commit/01a9505aeac5d2a3afc57a7b1ce98523b58929ae))
* **cua-driver:** desktop-scope Phase 1 — capture_scope config, get_desktop_state, Windows screen-absolute actions ([#1968](https://github.com/trycua/cua/issues/1968)) ([#2019](https://github.com/trycua/cua/issues/2019)) ([fc27185](https://github.com/trycua/cua/commit/fc271854149771adbb9f2e71ad64e58de5087de7))
* **cua-driver:** embedded mode — inherit the host app's TCC grants, never prompt ([#2102](https://github.com/trycua/cua/issues/2102)) ([b654f27](https://github.com/trycua/cua/commit/b654f27d609ecbac22ea63a000c868c90c0ee44d))
* **cua-driver:** fall back to key events when type-target is a terminal ([687d908](https://github.com/trycua/cua/commit/687d908f5efe19ddfd0915f6e654ffb9118bea01))
* **cua-driver:** Hermes-decoupling MCP surface + install UX ([e85cd87](https://github.com/trycua/cua/commit/e85cd878ae859e3c66f53ce83dd0060fdcb0528e))
* **cua-driver:** prepare ClawHub skill release ([#2265](https://github.com/trycua/cua/issues/2265)) ([393c984](https://github.com/trycua/cua/commit/393c984190b22543ed0d83ce457e7bfec0653492))
* **cua-driver:** standardize computer action telemetry ([#2318](https://github.com/trycua/cua/issues/2318)) ([9a29c8d](https://github.com/trycua/cua/commit/9a29c8dde15591713ddf8657050201894da3c2d8))
* **cua-driver:** support approved existing browser profiles ([#2261](https://github.com/trycua/cua/issues/2261)) ([8582630](https://github.com/trycua/cua/commit/8582630ca8f4d2d8ca0315d19dc5073b81a0fed9))
* **cua-driver:** unify tool telemetry ownership ([1435e84](https://github.com/trycua/cua/commit/1435e84a24f3d8de442091a37c229176faa2aba4))
* **cua-driver:** unify tool telemetry ownership ([ccdace9](https://github.com/trycua/cua/commit/ccdace9e7f6c01e08432e91e6eb8c882981c4d05))
* **cua-driver:** window-less desktop-scope click — macOS + Linux (unified interface) ([#2056](https://github.com/trycua/cua/issues/2056)) ([af2255c](https://github.com/trycua/cua/commit/af2255c78ba86dfdbdd49e1ce09e42b5101080b1))
* **cursor-overlay:** retina-aware rendering + arrow/teardrop selection ([c172e2a](https://github.com/trycua/cua/commit/c172e2acfa038ae437ca2d67795bf90c5c2c02e7))
* **driver:** add per-session capture scope ([#2329](https://github.com/trycua/cua/issues/2329)) ([db0ee87](https://github.com/trycua/cua/commit/db0ee870c200b5acc9a6542eabc84dc674b729c7))
* **driver:** add YAML and Rego permission policies ([#2235](https://github.com/trycua/cua/issues/2235)) ([afb25d7](https://github.com/trycua/cua/commit/afb25d740ba7f5b7bfd9970aee7b02919b10eb6e))
* **platform-linux:** native-Wayland parity (wlroots + portal + libei) ([#1966](https://github.com/trycua/cua/issues/1966)) ([519dfdf](https://github.com/trycua/cua/commit/519dfdfbd6c7cf8e10fa860b15201f84da73eb57))
* **release:** automate Driver and Lume attribution ([#2267](https://github.com/trycua/cua/issues/2267)) ([5019927](https://github.com/trycua/cua/commit/50199272cff60b11df0374dd26db365338be5744))
* **skills:** autodetect Hermes (NousResearch/hermes-agent) at ~/.hermes/skills ([#1963](https://github.com/trycua/cua/issues/1963)) ([9d90c81](https://github.com/trycua/cua/commit/9d90c815d31827f64a09d94c1d32d22c327f2770))
* **test-harness:** Linux GTK3 harness app + test (parity with macOS/Windows) ([#2050](https://github.com/trycua/cua/issues/2050)) ([aaa3f4c](https://github.com/trycua/cua/commit/aaa3f4c4ad0b88fda5f51763a200bb32863a96cd))
* **windows:** background input without z-raise + multi-cursor demos + overlay improvements ([#1809](https://github.com/trycua/cua/issues/1809)) ([53bb84c](https://github.com/trycua/cua/commit/53bb84cc71cba19f5a91c6cd99f80650a34c5c68))


### Bug Fixes

* **cua-driver-rs:** feature-gate portal+libei so bullseye CD publishes v0.6.1 ([#1967](https://github.com/trycua/cua/issues/1967)) ([5061c11](https://github.com/trycua/cua/commit/5061c11b3aa51f596f63838f87a2dca1991aa545))
* **cua-driver-rs:** make Cargo.toml the source of truth for release bumps ([#1907](https://github.com/trycua/cua/issues/1907)) ([dacde81](https://github.com/trycua/cua/commit/dacde8147e6c27ee15855e6060fe834c111ce616))
* **cua-driver-rs:** post-install + Skill hints route through mcp-config (not the broken `claude mcp add -- … --flag` line) ([#1681](https://github.com/trycua/cua/issues/1681)) ([ef7e2b6](https://github.com/trycua/cua/commit/ef7e2b605c4fa113bde25082812a2c10edb9de7a))
* **cua-driver-rs:** release installer unifies home on ~/.cua-driver + cleans up prior local install ([#1803](https://github.com/trycua/cua/issues/1803)) ([33a6893](https://github.com/trycua/cua/commit/33a6893634f47e809cf83eaed150f65304272948))
* **cua-driver-rs:** show the agent cursor by default ([#1955](https://github.com/trycua/cua/issues/1955)) ([1119035](https://github.com/trycua/cua/commit/11190356b3776f2003171872ef0f75c7f5d90d40))
* **cua-driver-rs:** wire/guide per-session cursors through the real mcp path + skills/docs ([#1787](https://github.com/trycua/cua/issues/1787)) ([18e2cbb](https://github.com/trycua/cua/commit/18e2cbb1c0c5ce221a2239abbdeb913a07777af9))
* **cua-driver/install.ps1:** resolve a valid temp dir when $env:TEMP is a missing 8.3 short path ([#1911](https://github.com/trycua/cua/issues/1911)) ([#1990](https://github.com/trycua/cua/issues/1990)) ([8395169](https://github.com/trycua/cua/commit/8395169dbe1c19a17ab4320a8e91066b1c9eefbf))
* **cua-driver/linux:** auto-discover XAUTHORITY for SSH-driven Wayland+Xwayland ([#1926](https://github.com/trycua/cua/issues/1926)) ([#1999](https://github.com/trycua/cua/issues/1999)) ([9091a30](https://github.com/trycua/cua/commit/9091a30fb4663dcadb65cdf800de774325c2deff))
* **cua-driver/linux:** bound X11 overlay render work ([#2331](https://github.com/trycua/cua/issues/2331)) ([428d7aa](https://github.com/trycua/cua/commit/428d7aa5364401c842a5e1428ba3b9ad7009e051))
* **cua-driver/linux:** fail loudly when X11 input can't be delivered on pure Wayland ([#1921](https://github.com/trycua/cua/issues/1921)) ([#1994](https://github.com/trycua/cua/issues/1994)) ([7c1d06b](https://github.com/trycua/cua/commit/7c1d06bf19d224e4690b6a537e85361cce8c81bb))
* **cua-driver/linux:** get_desktop_state screen size on pure Wayland ([#2047](https://github.com/trycua/cua/issues/2047)) ([6b2b262](https://github.com/trycua/cua/commit/6b2b2629debb2b19c57d0c0b2abdaf216ea41d3e))
* **cua-driver/linux:** glide the agent cursor to the desktop-scope click point ([#2061](https://github.com/trycua/cua/issues/2061)) ([7468487](https://github.com/trycua/cua/commit/7468487d303469b4993b960c8a2cb66289034bed))
* **cua-driver/linux:** remap a spare keycode for keysyms absent from the X keymap ([#2048](https://github.com/trycua/cua/issues/2048)) ([a139668](https://github.com/trycua/cua/commit/a139668f1c87b12bc2ce9a2d2c600252331557d9))
* **cua-driver/linux:** report dead input backend on KDE/GNOME Wayland instead of silent no-op ([#1982](https://github.com/trycua/cua/issues/1982)) ([#1992](https://github.com/trycua/cua/issues/1992)) ([3aff226](https://github.com/trycua/cua/commit/3aff22663a6409ecca1a42353ebbac3455256237))
* **cua-driver/linux:** retry root-only AT-SPI tree on cold Qt6 launch ([#1927](https://github.com/trycua/cua/issues/1927)) ([#1998](https://github.com/trycua/cua/issues/1998)) ([b64af30](https://github.com/trycua/cua/commit/b64af303f2f844f20e5de1d10214d6123bf7f680))
* **cua-driver/linux:** return structured launch_app result ([#2091](https://github.com/trycua/cua/issues/2091)) ([c0c6790](https://github.com/trycua/cua/commit/c0c67908167f3d0532683b005ad26659dcab8a34))
* **cua-driver/macos:** set_config accepts {key,value} shape (parity with Win/Linux) ([#2059](https://github.com/trycua/cua/issues/2059)) ([3def0d7](https://github.com/trycua/cua/commit/3def0d79df66b8a9d0b4dfeb71201ddebe672a1d))
* **cua-driver/windows:** retry transient BuildUpdatedCache failures instead of returning elements=0 ([#1881](https://github.com/trycua/cua/issues/1881)) ([#1996](https://github.com/trycua/cua/issues/1996)) ([7518f37](https://github.com/trycua/cua/commit/7518f37a3222761e4a3e391f0516c9ae8fe7fa34))
* **cua-driver/windows:** route DoubleClick/RightClick on Chromium targets via SendInput ([#1984](https://github.com/trycua/cua/issues/1984)) ([#1995](https://github.com/trycua/cua/issues/1995)) ([26d9298](https://github.com/trycua/cua/commit/26d9298e03cc2d6df5d470e9a3707eb21adde398))
* **cua-driver:** add DPI awareness manifest for Windows ([#1821](https://github.com/trycua/cua/issues/1821)) ([b286d9c](https://github.com/trycua/cua/commit/b286d9ccd8ea23d40da9e211dd5ad67fa69cd6a8))
* **cua-driver:** avoid opaque X11 cursor bloom ([#2328](https://github.com/trycua/cua/issues/2328)) ([e57fed5](https://github.com/trycua/cua/commit/e57fed576afba793db756d0b54e6ff6dde3387d6))
* **cua-driver:** default install.sh backend back to Swift ([#1762](https://github.com/trycua/cua/issues/1762)) ([af6bf41](https://github.com/trycua/cua/commit/af6bf41a491f39c28420aaf1abe70e257790678e))
* **cua-driver:** deliver background browser keystrokes ([a7b7413](https://github.com/trycua/cua/commit/a7b741317c86ccd3b1241421e411137f87115eb8))
* **cua-driver:** deliver trusted background browser clicks ([00f33c4](https://github.com/trycua/cua/commit/00f33c4cfaff223a0ade812bb16f4cdb16419d87))
* **cua-driver:** distinguish autostart query failures ([#2345](https://github.com/trycua/cua/issues/2345)) ([0b798a5](https://github.com/trycua/cua/commit/0b798a59cbe7f9e628ae488cb8023b9b6f990bd6))
* **cua-driver:** don't treat socket read-timeout (EAGAIN) as fatal in daemon proxy ([#1864](https://github.com/trycua/cua/issues/1864)) ([#1997](https://github.com/trycua/cua/issues/1997)) ([b651461](https://github.com/trycua/cua/commit/b651461eae8f44083e6d7e1257a4aa1031a05581))
* **cua-driver:** harden browser preview boundaries ([#2347](https://github.com/trycua/cua/pull/2347)) ([667646a](https://github.com/trycua/cua/commit/667646af2d6391d419ff2169e7b4ea3e8d7c9ffc))
* **cua-driver:** harden browser scroll and endpoint proof ([#2353](https://github.com/trycua/cua/pull/2353)) ([7510ed9](https://github.com/trycua/cua/commit/7510ed9c1f82f66c758e6d02e755350d705cacd5))
* **cua-driver:** harden existing browser attachment ([2ccb87c](https://github.com/trycua/cua/commit/2ccb87c8a9cbcaabc428243c8e7fbe809ac63709))
* **cua-driver:** harden Linux AT-SPI and input delivery ([3c63d71](https://github.com/trycua/cua/commit/3c63d712a5eab52103112cfab86fc2ca842442a3))
* **cua-driver:** harden Linux AT-SPI and input delivery ([ea599fd](https://github.com/trycua/cua/commit/ea599fdfd76f49f94a3fc4ddb5486dca68a3fbc4))
* **cua-driver:** harden macOS browser profile setup ([e6643f2](https://github.com/trycua/cua/commit/e6643f21e56e7fcee90c503f8fd39c7659d83b43))
* **cua-driver:** harden macOS input and capture delivery ([4e1aab4](https://github.com/trycua/cua/commit/4e1aab42ca4dba143aed20da72878a0ec4290739))
* **cua-driver:** harden macOS input and capture delivery ([ec908f7](https://github.com/trycua/cua/commit/ec908f74cb9f89cb6dbb7af14c18ff71105bd75a))
* **cua-driver:** harden telemetry release paths ([#2214](https://github.com/trycua/cua/issues/2214)) ([06fcc0f](https://github.com/trycua/cua/commit/06fcc0fb3bdf5d111069525d146dcb5aa2050e1b))
* **cua-driver:** harden Windows delivery and GUI validation ([5007f85](https://github.com/trycua/cua/commit/5007f85a37b4e29fea1d34c3fcc4590dd21f1e99))
* **cua-driver:** harden Windows delivery and GUI validation ([3c5c4c1](https://github.com/trycua/cua/commit/3c5c4c13552e349653e4d62e1f03e172e5cd5faa))
* **cua-driver:** Linux list_windows emits `bounds` for cross-platform parity ([#2017](https://github.com/trycua/cua/issues/2017)) ([#2018](https://github.com/trycua/cua/issues/2018)) ([9e84c1d](https://github.com/trycua/cua/commit/9e84c1d8826dc1bfba166456b34f86a9d6212e0d))
* **cua-driver:** Linux X11 and Wayland convergence ([#2182](https://github.com/trycua/cua/issues/2182)) ([51a8e25](https://github.com/trycua/cua/commit/51a8e2593fc88efd4f0678d9ab7bbba50c22c91b))
* **cua-driver:** macOS e2e convergence (conv2 3/7) ([#2183](https://github.com/trycua/cua/issues/2183)) ([1d0ff7c](https://github.com/trycua/cua/commit/1d0ff7cc3ef7cfcadbf86b8c4b96ab396e319317))
* **cua-driver:** make install-local TCC grants survive rebuilds and release↔local switches ([#2360](https://github.com/trycua/cua/pull/2360)) ([af75ca5](https://github.com/trycua/cua/commit/af75ca5e774e01efc0648a408726343e7133ccd1))
* **cua-driver:** model Linux dialog delivery ([31b4f85](https://github.com/trycua/cua/commit/31b4f85755da8d5f5e793745839e2b0639188b22))
* **cua-driver:** parse uninstall.sh under macOS /bin/bash (bash 3.2) ([#1723](https://github.com/trycua/cua/issues/1723)) ([d720891](https://github.com/trycua/cua/commit/d72089181db06d8530bf15023757fbc0a8a65c57))
* **cua-driver:** persist set_config to disk on Windows + Linux ([#2034](https://github.com/trycua/cua/issues/2034)) ([e08574c](https://github.com/trycua/cua/commit/e08574ca0b3c7ca5c0d9ae33e6ceeac9a45e0d20))
* **cua-driver:** preserve approved Windows CDP port proof ([2527e77](https://github.com/trycua/cua/commit/2527e771ca0ea5aaed0c3d62d52ff4b07322264a))
* **cua-driver:** preserve cross-platform browser posture ([6ea7806](https://github.com/trycua/cua/commit/6ea780690388c33498a8111bc618d7fd0bc8d196))
* **cua-driver:** report unknown browser tab selection ([6611622](https://github.com/trycua/cua/commit/6611622d6c6ef355661c80b6ecae765694088153))
* **cua-driver:** reset TCC grant on release/local signing transition ([fa309eb](https://github.com/trycua/cua/commit/fa309eb5ed0aac1e63c9eaf80dcb2a7749b8b472))
* **cua-driver:** retain country-only telemetry ([#2217](https://github.com/trycua/cua/issues/2217)) ([a705c2a](https://github.com/trycua/cua/commit/a705c2ac463392109063c171ca4586a29811103c))
* **cua-driver:** retry daemon socket writes on EAGAIN, write-side mirror of [#1997](https://github.com/trycua/cua/issues/1997) ([#2036](https://github.com/trycua/cua/issues/2036)) ([d524d97](https://github.com/trycua/cua/commit/d524d97b5a414a9df9d62767d627f24b17bb6353))
* **cua-driver:** retry transient Windows CDP discovery ([4f5abc2](https://github.com/trycua/cua/commit/4f5abc2a231e9a8565326d7f58e78477f09e2666))
* **cua-driver:** reuse self-signed local identity ([a27753e](https://github.com/trycua/cua/commit/a27753e457be1947728fcaa298f077bb306172e9))
* **cua-driver:** stabilize background browser tabs ([3348fc5](https://github.com/trycua/cua/commit/3348fc5778e5b0dfb9402d2c13048c301eda9f57))
* **cua-driver:** target page JavaScript exactly ([#2166](https://github.com/trycua/cua/issues/2166)) ([fb5bc19](https://github.com/trycua/cua/commit/fb5bc192a5311d0519447f1d301bdf0b0c93bbb0))
* **cua-driver:** uninstall.ps1 one-liner crashes iex with "Unexpected attribute 'CmdletBinding'" ([#1750](https://github.com/trycua/cua/issues/1750)) ([b8cbe85](https://github.com/trycua/cua/commit/b8cbe852b19e8b4de18f967636192e9175c91380))
* **cua-driver:** wait for background tab input readiness ([a19e909](https://github.com/trycua/cua/commit/a19e909db8bfd236c8f71ab56edfd4c3fdc7c807))
* **cua-driver:** Windows e2e convergence ([#2180](https://github.com/trycua/cua/issues/2180)) ([88f88e1](https://github.com/trycua/cua/commit/88f88e1b5a31810699781e49f2dd9b9733fbaa82))
* **driver:** remove current autostart services on uninstall ([#2358](https://github.com/trycua/cua/issues/2358)) ([4a492f3](https://github.com/trycua/cua/commit/4a492f36704eaf063a82eadd0469d08684c7a1f5))
* **driver:** require daemon-backed calls and stabilize E2E ([#2338](https://github.com/trycua/cua/issues/2338)) ([9251bb0](https://github.com/trycua/cua/commit/9251bb006018d829b452e7109e322eab3f8c9633))
* **get_window_state:** 30s timeout + 2000-node cap for heavy webview apps ([#1754](https://github.com/trycua/cua/issues/1754)) ([73a84f5](https://github.com/trycua/cua/commit/73a84f5d1f2092fbd58e3a53cd1d61f0f3c3807e))
* **release:** synchronize driver skill versions ([#2362](https://github.com/trycua/cua/pull/2362)) ([512e45a](https://github.com/trycua/cua/commit/512e45a03fc0b4390f3d29b6443db5821157f7a0))
