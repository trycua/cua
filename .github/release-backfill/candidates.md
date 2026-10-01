# Historical release backfill candidates

# Review: Cua Driver 0.1.3

- Tag: `cua-driver-rs-v0.1.3`
- Release ID: `322238330`
- Original body SHA256: `2d53e3772e0815a00fc24d64df3ada43aef99b123d6194b94bdcf56062175f79`
- Proposed body SHA256: `edb0b944c01607e582aae8844abafc8e5a9f05cd07d6393119ff9c51ada71832`

<!-- cua-release-backfill:v1:start evidence-sha256=e16c3acd0338378003bbf23139c1bdde4077fee196fec40c4bc9aa57e8402802 -->
## Summary

This release introduces cua-driver-rs, a cross-platform Rust port of cua-driver that adds feature-complete Windows and Linux support alongside a beta macOS build.

## Features

- Added a cross-platform Rust port of cua-driver with feature-complete Windows and Linux automation backends, using PostMessage plus UI Automation on Windows and X11 plus AT-SPI on Linux, that can drive background windows without stealing focus from the user's foreground app. ([#1511](https://github.com/trycua/cua/pull/1511))

## Contributors

This release contains maintainer changes only.

## Full changelog

[Initial release...cua-driver-rs-v0.1.3](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.1.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.1.4

- Tag: `cua-driver-rs-v0.1.4`
- Release ID: `322692636`
- Original body SHA256: `447e1c147326efdc7438fb6784c3104b9985fcca90a6ccf2579d818210998ada`
- Proposed body SHA256: `ecf427e8038247f36f6e547dafbaad656f254d184b14d81e791099b74dcf2740`

<!-- cua-release-backfill:v1:start evidence-sha256=bd7aabc293a6df2cf14eea1d78dea3c8eb25cc07777bd362d21171652e759996 -->
## Summary

This patch fixes a macOS installer 404 and makes CLI configuration changes actually take effect in MCP sessions.

## Fixes

- Fixed the install script 404ing on macOS by downloading the universal binary tarball instead of a nonexistent per-architecture darwin build. ([#1518](https://github.com/trycua/cua/pull/1518))
- Fixed configuration changes made with config set not being applied during MCP sessions; the driver now loads saved configuration at startup and persists configuration changes made during a session to disk. ([#1518](https://github.com/trycua/cua/pull/1518))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.1.3...cua-driver-rs-v0.1.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.1.3...cua-driver-rs-v0.1.4)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.0

- Tag: `cua-driver-rs-v0.2.0`
- Release ID: `323861273`
- Original body SHA256: `53454007c53fa30a467cc2e9b2af5231086d3c0a6a7f4cac090b9e8772aa759f`
- Proposed body SHA256: `3fec13b303ea7485896dc214aa262fad3f6471c983d7c500d1dba6cdc189a764`

<!-- cua-release-backfill:v1:start evidence-sha256=c9f72a5f40b4bfc7497e81e6abe7c74565dc0aa89a2d1c5c24cc9c5460783dfd -->
## Summary

This release focuses on macOS readiness for cua-driver-rs, adding a first-launch permissions flow, focus-steal prevention for app launches and interactive actions, anonymous usage telemetry, and startup update notifications.

## Features

- Added a macOS first-launch permissions flow that prompts for and waits on Accessibility and Screen Recording permissions before the driver starts, with an opt-out for CI and headless environments. ([#1529](https://github.com/trycua/cua/pull/1529))
- Added anonymous usage telemetry with an environment variable opt-out. ([#1532](https://github.com/trycua/cua/pull/1532))
- Added a startup banner that announces when a newer release is available. ([#1536](https://github.com/trycua/cua/pull/1536))
- Added focus-steal prevention on macOS so that launching an application no longer steals focus from the user's active application. ([#1524](https://github.com/trycua/cua/pull/1524))
- Added per-action focus suppression and window-change detection on macOS so click, type, hotkey, drag, scroll, and set-value actions no longer steal focus and report any window changes the action caused. ([#1531](https://github.com/trycua/cua/pull/1531))

## Fixes

- Fixed permission prompts being attributed to the wrong app when the driver was started from an IDE terminal on macOS, by auto-relaunching under its own app bundle. ([#1530](https://github.com/trycua/cua/pull/1530))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.1.4...cua-driver-rs-v0.2.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.1.4...cua-driver-rs-v0.2.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.1

- Tag: `cua-driver-rs-v0.2.1`
- Release ID: `323899925`
- Original body SHA256: `3f58d7385cea00b4e5c90b306fe5f63c938905f0dac0de73da547ab396793765`
- Proposed body SHA256: `44bf876ae5e425723dca3c1e48a820c09434e5606fb7459001d614466fc440e0`

<!-- cua-release-backfill:v1:start evidence-sha256=89999afc5b79468b39845aad8ca1b67878143c47a5b1720e20c04b47bad57418 -->
## Summary

This release adds a Windows installer with native ARM64 support and makes installs on Linux and Windows safer through versioned, rollback-capable install directories, automatic cleanup of old versions, and protection against concurrent installs.

## Features

- Added a Windows PowerShell installer and native Windows ARM64 builds. ([#1540](https://github.com/trycua/cua/pull/1540))
- Switched Linux and Windows installs to a versioned directory layout with an atomic symlink or junction swap, so a previous version can be restored quickly if needed. ([#1540](https://github.com/trycua/cua/pull/1540))
- Added automatic cleanup of old installed versions, keeping the most recent versions by default and configurable by the user, plus a lockfile that prevents concurrent installs from corrupting the install directory. ([#1541](https://github.com/trycua/cua/pull/1541))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.0...cua-driver-rs-v0.2.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.0...cua-driver-rs-v0.2.1)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.2

- Tag: `cua-driver-rs-v0.2.2`
- Release ID: `323937932`
- Original body SHA256: `7b3c9c44b587c05031d4339478701ddb121b87fea1fec195ed88cfaa26af2efe`
- Proposed body SHA256: `ec62e0cdf856d1461881bbbcbd65734760a9c72e9a2772d0930a519ba116a2ea`

<!-- cua-release-backfill:v1:start evidence-sha256=4859f757b08e050941690f17952f35d379f1a9aad93cb0d6040e25ede030ebfc -->
## Summary

This release improves Windows reliability by fixing window enumeration for modern apps, adding support for launching packaged Windows apps, and adding diagnostics for common Windows session issues, while also unifying application listing across all platforms.

## Features

- Added support for launching packaged Windows Store apps such as the modern Notepad, Calculator, and Paint, fixing launches that previously returned a short-lived stub process with no window. ([#1544](https://github.com/trycua/cua/pull/1544))
- Added real diagnostics for Windows and Linux, including detection of non-interactive Windows sessions (such as over SSH) that cause window-driving tools to silently return empty results. ([#1543](https://github.com/trycua/cua/pull/1543))
- Unified the list of running and installed applications across macOS, Windows, and Linux, including the launch path and metadata needed to start an application that isn't currently running. ([#1545](https://github.com/trycua/cua/pull/1545))

## Fixes

- Fixed window listing on Windows returning empty results for modern apps such as WebView2-hosted Notepad and packaged apps, by enumerating windows through UI Automation in addition to the classic enumeration method. ([#1542](https://github.com/trycua/cua/pull/1542))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.1...cua-driver-rs-v0.2.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.1...cua-driver-rs-v0.2.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.3

- Tag: `cua-driver-rs-v0.2.3`
- Release ID: `324118461`
- Original body SHA256: `3ad37177e9d9a11ec881f3b6cb19ef3de9e54694b1f5bf1328ace42d2050d84d`
- Proposed body SHA256: `dc6fd6bf7cbb4d4c96094d063650b2731a2be6698c92e536998ce4da1aaa7a4a`

<!-- cua-release-backfill:v1:start evidence-sha256=636f3a71045ab5b96f88ba588a58b9cb1cab2889e846aaf04dace1b7e7d22783 -->
## Summary

This release adds a Windows autostart CLI verb and fixes several Windows-specific reliability issues, including UI Automation-based clicking on UWP apps and stability under non-interactive Session 0.

## Features

- Add a cua-driver autostart CLI verb (enable, disable, status, kick) that registers the Windows daemon as a Scheduled Task, replacing duplicated PowerShell logic in the install scripts. ([#1550](https://github.com/trycua/cua/pull/1550))

## Fixes

- Clicking by element index or by screen coordinates on Windows now uses UI Automation invocation, fixing clicks that silently failed on UWP apps such as Calculator, Notepad, Edge, and Settings. ([#1549](https://github.com/trycua/cua/pull/1549))
- Fixed crashes and hangs when running on Windows in non-interactive Session 0, added a Session 0 startup warning, restored proper foreground-window behavior after launching UWP apps in the background, and filtered out opaque system packages from the app list. ([#1548](https://github.com/trycua/cua/pull/1548))
- Installed Windows apps are now sorted with recently used apps first, with improved UWP display name resolution from app manifests. ([#1547](https://github.com/trycua/cua/pull/1547))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.2...cua-driver-rs-v0.2.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.2...cua-driver-rs-v0.2.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.4

- Tag: `cua-driver-rs-v0.2.4`
- Release ID: `324336314`
- Original body SHA256: `ffd60ed85a4c065005122289d9ddec7d3639e678f4a6eadd37682ba09b28c7b2`
- Proposed body SHA256: `97a0e312dfff192514f21fc1d6dc71284113624d9d561ccbe6cdef6e9ec98f58`

<!-- cua-release-backfill:v1:start evidence-sha256=e391e00e810beb2e304f34a7d12485cdaeae337bd75617b6ed455a22aa7194e9 -->
## Summary

This release converges macOS install identity with the Swift driver, consolidates installer scripts to a single canonical entry point per platform, ships Windows and Linux skill documentation, and further improves vision-mode clicking on Windows.

## Features

- The macOS app now installs as CuaDriver.app with the bundle identifier com.trycua.cuadriver, replacing the Swift driver's app at the same install path instead of installing alongside it. ([#1559](https://github.com/trycua/cua/pull/1559))
- Installer URLs are now consolidated to a single canonical install.sh and install.ps1 per platform, with old install URLs continuing to work via a redirect shim. ([#1556](https://github.com/trycua/cua/pull/1556), [#1557](https://github.com/trycua/cua/pull/1557))
- Added Windows and Linux skill documentation, deployed automatically at install time. ([#1553](https://github.com/trycua/cua/pull/1553))

## Fixes

- Fixed vision-mode clicks at screen coordinates that could hit the wrong window or fail on UWP apps when the target was occluded or hosted in a separate process. ([#1551](https://github.com/trycua/cua/pull/1551))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.3...cua-driver-rs-v0.2.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.3...cua-driver-rs-v0.2.4)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.5

- Tag: `cua-driver-rs-v0.2.5`
- Release ID: `324433906`
- Original body SHA256: `644d5f1319d782a0e036e95818e70faeea04161271c3e36a0b75041a7d00e754`
- Proposed body SHA256: `bc42ec7227769e050d0e9080bd7d3b302765bef9a03069433f2cd4568724bc00`

<!-- cua-release-backfill:v1:start evidence-sha256=e124c0e2931cf5dc3866780caa39516b1237cef920e99cf4a6f8479520cfd232 -->
## Summary

This release adds an opt-in native permissions onboarding panel on macOS and fixes a bug where the Screen Recording permission could be incorrectly reported as granted.

## Features

- Added a native macOS permissions onboarding panel, showing live status of required grants and guiding users to System Settings; enabled via an opt-in environment variable while the terminal-based prompt remains the default. ([#1565](https://github.com/trycua/cua/pull/1565), [#1566](https://github.com/trycua/cua/pull/1566))

## Fixes

- Fixed a bug on macOS where the Screen Recording permission check could report the grant as active even when it had been revoked, which could skip the permissions setup prompt and cause screenshot or recording failures. ([#1562](https://github.com/trycua/cua/pull/1562))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.4...cua-driver-rs-v0.2.5](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.4...cua-driver-rs-v0.2.5)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.6

- Tag: `cua-driver-rs-v0.2.6`
- Release ID: `324463616`
- Original body SHA256: `2d7a9c3aa2e472e0596af3ed52ffa8e6bba497970d9bc2a95162405036709d91`
- Proposed body SHA256: `4c13fdc5e7d8bcf5494f4e87c439c5e81d385f336eb12fad7a6e296782de20c9`

<!-- cua-release-backfill:v1:start evidence-sha256=19bf78434d4b993b5bbc29c9e2d72078c1a9a14354db52d3833f5d6aa3fb3f63 -->
## Summary

This release fixes a macOS hang where the daemon would keep waiting indefinitely after the user granted Accessibility and Screen Recording permissions.

## Fixes

- Fixed a hang on macOS where the daemon could remain stuck waiting for permissions even after the user granted Accessibility and Screen Recording access, by restarting itself to pick up the refreshed permission state. ([#1567](https://github.com/trycua/cua/pull/1567))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.5...cua-driver-rs-v0.2.6](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.5...cua-driver-rs-v0.2.6)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.7

- Tag: `cua-driver-rs-v0.2.7`
- Release ID: `325151100`
- Original body SHA256: `a85a08b4355cf93b3dad777de957b2f1ee015292d18ef84b3656fa7519ca603b`
- Proposed body SHA256: `b756c3578959a1a84ab8dfd7551cb8ffeeb6b2c92c7a5ba0800d352ec3684c11`

<!-- cua-release-backfill:v1:start evidence-sha256=2400acccd60fffa3d4ad7368be030a7a42d48e80c94db6eeecf4ad95c905a552 -->
## Summary

This release fixes an issue on Windows and Linux where the MCP server did not use a running daemon, causing tools to return empty results when launched from a non-interactive session.

## Fixes

- On Windows and Linux, the MCP server now proxies through an already-running daemon when one is available, fixing empty results for window and app tools when the MCP server was launched from a non-interactive session such as over SSH. ([#1580](https://github.com/trycua/cua/pull/1580))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.6...cua-driver-rs-v0.2.7](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.6...cua-driver-rs-v0.2.7)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.8

- Tag: `cua-driver-rs-v0.2.8`
- Release ID: `325976339`
- Original body SHA256: `ae06bccff26d90339160f2540afd33878f9370c1719d3984b8cc0e39503b809f`
- Proposed body SHA256: `83a5594c99054c5f508fc8aac2c568c3db84510c5721afd465e834cc4e8e65f2`

<!-- cua-release-backfill:v1:start evidence-sha256=01af0195d3fa4cf552bf1811aa9b8ad017a82e7726a71bba7b6d8ffa7da8dd8b -->
## Summary

This release advances Windows automation for modern UWP and XAML applications, adding a force-terminate tool for stuck processes, partially fixing all-black screenshots for XAML-based windows, and prototyping UI Automation-based text input and an elevated worker process.

## Features

- Added a kill_app tool that force-terminates a process by pid, using taskkill on Windows and SIGKILL on macOS and Linux, providing a way to terminate UWP and WinUI3 apps that ignore cooperative window close requests. ([#1596](https://github.com/trycua/cua/pull/1596))
- Typing text into modern XAML, WinUI3, and UWP application windows now routes through UI Automation's ValuePattern so keystrokes are correctly delivered instead of being silently dropped; added a debug_window_info diagnostic tool for inspecting window and automation properties. ([#1597](https://github.com/trycua/cua/pull/1597))
- Added a prototype Windows-only worker process that runs at UIAccess integrity to enable UWP automation, with tool calls automatically routed through it when present, falling back to previous behavior when the worker cannot elevate. ([#1604](https://github.com/trycua/cua/pull/1604))

## Fixes

- Most previously all-black XAML, WinUI3, and UWP window captures now return real on-screen pixels by copying the window's on-screen bounds from the desktop instead of using PrintWindow; background-collapsed UWP windows remain unavailable. ([#1599](https://github.com/trycua/cua/pull/1599))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.7...cua-driver-rs-v0.2.8](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.7...cua-driver-rs-v0.2.8)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.9

- Tag: `cua-driver-rs-v0.2.9`
- Release ID: `326882853`
- Original body SHA256: `3ca71320b96e0ae8a8a218cdc3e03236b5d91fffc5f5eb734f28caf4cc7979ad`
- Proposed body SHA256: `daff24d9dde02c63544ac97b1d573366bd13bf2743c5de3e63ab55be3747522b`

<!-- cua-release-backfill:v1:start evidence-sha256=4df74082a5aca9f5a69c582dd4c0219ba1654a2a061c5df9900035143a460165 -->
## Summary

This release fixes several Windows input-delivery bugs affecting modern UWP and XAML apps and Chromium-based browsers, including click precision, hotkey and keyboard modifier handling, and UI Automation tree discovery, and adds automatic anti-throttling flags for launched Chromium browsers.

## Features

- Launching Chromium-based browsers now automatically injects flags that disable occlusion-based renderer throttling, preventing launched browser tabs from having their content invisible to automation and screenshots. ([#1624](https://github.com/trycua/cua/pull/1624))
- Clicking at specific coordinates on Chromium-based browser windows now uses a synthesized input method when needed, fixing clicks on custom-drawn surfaces like canvases that were previously silently ignored by the browser. ([#1625](https://github.com/trycua/cua/pull/1625))

## Fixes

- Clicking at specific coordinates no longer incorrectly redirects to a UI Automation invoke at an element's center for container-like elements such as canvases or paint surfaces; clicks on non-button and non-menu elements now use the requested coordinates directly. ([#1622](https://github.com/trycua/cua/pull/1622))
- Hotkeys sent to modern XAML and UWP application windows now route through UI Automation accelerator discovery and pattern activation, so shortcuts like Ctrl+B actually toggle the target control instead of silently doing nothing, and unsupported combinations now return an actionable error. ([#1611](https://github.com/trycua/cua/pull/1611))
- Hotkeys with modifier keys sent to legacy Win32 applications now use a synthesized input method that properly sets system-wide modifier state, fixing shortcuts that previously were ignored or typed as literal characters; app launching also now correctly resolves the process window when the launched executable is a stub that re-executes into a different process. ([#1618](https://github.com/trycua/cua/pull/1618))
- Keyboard and mouse input sent to a target window is now detected and reported when it would be silently blocked by a mismatch in process integrity levels, and text typed into applications with an embedded editor child window is now correctly delivered to the focused child instead of being silently dropped. ([#1613](https://github.com/trycua/cua/pull/1613))
- Fixed automation tree discovery for CoreWindow-class Windows apps, which previously returned an empty automation tree; the driver now falls back to searching from the automation root for these apps. ([#1606](https://github.com/trycua/cua/pull/1606))
- Improved the error message returned when a hotkey has no matching UI Automation accelerator, clarifying what was searched and noting common causes such as menu-nested actions. ([#1612](https://github.com/trycua/cua/pull/1612))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.8...cua-driver-rs-v0.2.9](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.8...cua-driver-rs-v0.2.9)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.10

- Tag: `cua-driver-rs-v0.2.10`
- Release ID: `327102098`
- Original body SHA256: `70b8e18e43ac6b621f68511e9b77ea5a8057cd3461c46e4fb9d5dd7280474ccf`
- Proposed body SHA256: `165a7069c98e35a412247fedb7850b0dbe3dfa8111d3c961b3b17083eab6ddb0`

<!-- cua-release-backfill:v1:start evidence-sha256=7a26c8a3628ef024a8f0a91abee6bd271866793a2372a28ef8099531b3550ae3 -->
## Summary

This release changes the Windows autostart configuration to run the driver daemon with full administrator privileges, improving automation support for UWP applications that require elevated access.

## Features

- The Windows autostart scheduled task now registers the driver daemon to run at the highest available privilege level instead of a limited one, enabling successful automation of UWP applications such as Calculator and Settings that previously returned little to no automation data. ([#1630](https://github.com/trycua/cua/pull/1630))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.9...cua-driver-rs-v0.2.10](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.9...cua-driver-rs-v0.2.10)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.11

- Tag: `cua-driver-rs-v0.2.11`
- Release ID: `327109085`
- Original body SHA256: `7d7fc8732421ebcd4cb86f9ef524cc1c8e3a7ce89a3178ba4e274e8a51318f78`
- Proposed body SHA256: `0377c98fc92298afe36292abb093d31ffeccda2d182ff3204cd6768b3bb913be`

<!-- cua-release-backfill:v1:start evidence-sha256=74727ae479667860318df837350169cc59c4166ac9ea7f31a45a5cf30370989c -->
## Summary

This release fixes the Windows autostart enable command so it properly requests administrator elevation when needed instead of failing with an access-denied error.

## Fixes

- Running the autostart enable command from a non-elevated shell now automatically prompts for administrator elevation to register the scheduled task, instead of failing with an access-denied error. ([#1632](https://github.com/trycua/cua/pull/1632))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.10...cua-driver-rs-v0.2.11](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.10...cua-driver-rs-v0.2.11)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.12

- Tag: `cua-driver-rs-v0.2.12`
- Release ID: `327143688`
- Original body SHA256: `bce3af970c70cba004e81d86b14b6daf5f1d86222fea5db3d476c2a887ccb0b7`
- Proposed body SHA256: `f194eb17462918116ac65bc54bb1210bb91cc6aa6ed8e260776645f9c1138d19`

<!-- cua-release-backfill:v1:start evidence-sha256=492ce2bcb953c4d584a3a99ea8f9d037ef05547db935ab98e5c076cd2a788fc8 -->
## Summary

This release stops an unnecessary Windows error dialog from appearing when the driver daemon runs with administrator privileges.

## Fixes

- The driver no longer attempts to spawn the elevated UIAccess worker process when the main daemon is already running with administrator privileges, eliminating a spurious Windows error dialog that appeared on startup. ([#1634](https://github.com/trycua/cua/pull/1634))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.11...cua-driver-rs-v0.2.12](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.11...cua-driver-rs-v0.2.12)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.13

- Tag: `cua-driver-rs-v0.2.13`
- Release ID: `327177902`
- Original body SHA256: `78da6e1629c7a0704e1126fc14815c9ddeed5cbedc957140123f3bce422a1587`
- Proposed body SHA256: `ed9a81ef7fed344fa0322249a56ae27313b470c56bf02761a6decd890c32a15d`

<!-- cua-release-backfill:v1:start evidence-sha256=46e2a9377df0edf59aa64bc4eafe16c43136ff2e749c0d733c5e6a04d3970fcb -->
## Summary

This release improves the accuracy of Windows permission reporting and adds clearer error handling and diagnostics for CLI and app-launch operations.

## Fixes

- The CLI call command now reports JSON parse errors directly, including a hint for PowerShell 5.1 users whose shell strips quotes from multi-field JSON arguments, instead of silently falling back to stdin. ([#1642](https://github.com/trycua/cua/pull/1642))
- Windows launch_app error messages now list all supported application identifiers (bundle_id, name, aumid, path, launch_path, urls) instead of only mentioning bundle_id or name. ([#1642](https://github.com/trycua/cua/pull/1642))
- check_permissions on Windows now reports the process token's actual integrity level instead of relying on a legacy elevation flag that could misreport permission status for administrator and autostart-launched processes. ([#1641](https://github.com/trycua/cua/pull/1641))
- Added diagnostic logging for Windows app enumeration failures so gaps in list_apps output (such as missing UWP apps) are now visible instead of silent. ([#1643](https://github.com/trycua/cua/pull/1643))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.12...cua-driver-rs-v0.2.13](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.12...cua-driver-rs-v0.2.13)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.14

- Tag: `cua-driver-rs-v0.2.14`
- Release ID: `327193144`
- Original body SHA256: `cc4e0bd160574a1d38c16d5e85b3dd56216aa2b274e0fa0f4246b3ec239aee17`
- Proposed body SHA256: `d70118b14f8874b09b6287e4ea39c0f0244309f3b2c43aa9ab5ee7eb1bbf4e00`

<!-- cua-release-backfill:v1:start evidence-sha256=3426c7933511a21b5fa818b964c308eecc319f82571a4fd3a42d1db865c028dd -->
## Summary

This release fixes a case where check_permissions on Windows could still misreport integrity level due to thread impersonation.

## Fixes

- check_permissions on Windows now reverts thread impersonation and uses an explicit process handle before querying integrity level, fixing cases where it still misreported permission status. ([#1647](https://github.com/trycua/cua/pull/1647))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.13...cua-driver-rs-v0.2.14](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.13...cua-driver-rs-v0.2.14)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.16

- Tag: `cua-driver-rs-v0.2.16`
- Release ID: `327237516`
- Original body SHA256: `b6444b6e2538af9591cd505e57f0e531b1b24d9541060a8e29e8e397c8f3f21b`
- Proposed body SHA256: `ca65013c226d42c8a971b051d5bd4cc02ea2b7a0cd643602a409d050571da21e`

<!-- cua-release-backfill:v1:start evidence-sha256=7d03c4347e806d34a45110a513d67704dff279b003f35256c829f23b37844bd2 -->
## Summary

This release renames the local telemetry and configuration directory and migrates existing data automatically.

## Fixes

- The telemetry and configuration data directory was renamed from ~/.cua-driver-rs to ~/.cua-driver, with existing installations automatically migrated on first launch. ([#1650](https://github.com/trycua/cua/pull/1650))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.15...cua-driver-rs-v0.2.16](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.15...cua-driver-rs-v0.2.16)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.2.18

- Tag: `cua-driver-rs-v0.2.18`
- Release ID: `327295090`
- Original body SHA256: `c88f47cc80e73035ae904c1b81973d0009a64d982402f4f1fe7a7b00b7a40543`
- Proposed body SHA256: `8c47096e80bcd7251d4f0e47795e4480f4acd550338a001318268884cd69083c`

<!-- cua-release-backfill:v1:start evidence-sha256=2d047a07c4aaf33c28d39485043ae84ffd8e3c3c7025e286b2283ffe25b9f3fd -->
## Summary

This release fixes persistent misreporting of Windows permission status by changing how integrity level is queried.

## Fixes

- check_permissions on Windows now retrieves integrity level through a PowerShell-based check instead of an in-process query, fixing cases where permission status was still misreported. ([#1653](https://github.com/trycua/cua/pull/1653))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.2.17...cua-driver-rs-v0.2.18](https://github.com/trycua/cua/compare/cua-driver-rs-v0.2.17...cua-driver-rs-v0.2.18)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.3.2

- Tag: `cua-driver-rs-v0.3.2`
- Release ID: `330354385`
- Original body SHA256: `3c28ffde9bf057edf51b33087519d8f6d95a519719d4803aea036cb03a34db01`
- Proposed body SHA256: `caae9b174e9c774a46e831f2b9fb86e55d7b3a9dcacc45659578491e356ad7ce`

<!-- cua-release-backfill:v1:start evidence-sha256=1102cf84fceec1c842725b2ca8f74e89e017380fda22ca84032e5aee7a33d22e -->
## Summary

This release adds a way to check for newer cua-driver releases without installing them.

## Features

- Added a check-update CLI command and a check_for_update MCP tool that report whether a newer release is available, without installing it. ([#1734](https://github.com/trycua/cua/pull/1734))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.3.1...cua-driver-rs-v0.3.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.1...cua-driver-rs-v0.3.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.3.3

- Tag: `cua-driver-rs-v0.3.3`
- Release ID: `331875006`
- Original body SHA256: `63821496aad61b8ab1b1a18e70658c59cce9a0c65af72826c6154e390f9ddd72`
- Proposed body SHA256: `47c4bf017ab205562387cfcf7484da2b43842f0be89a34027c767df92e78fd65`

<!-- cua-release-backfill:v1:start evidence-sha256=f29d2f7fffa81168e6e500367b3ffa83fe326ffd3e246e28300f51d1abad53f9 -->
## Summary

This release makes the Rust implementation the primary cua-driver product with updated documentation, while addressing several macOS permission-handling and reliability issues, including install bundling, permission prompt attribution, and reliability fixes for window inspection and app launching.

## Features

- Documentation was reorganized to present the Rust implementation as the primary cua-driver product, with updated install, intro, and platform guides for macOS, Linux, and Windows. ([#1738](https://github.com/trycua/cua/pull/1738))
- Added a permissions status/grant CLI command on macOS, along with a live screen-recording capability check and clearer labeling of which process a reported permission actually belongs to. ([#1765](https://github.com/trycua/cua/pull/1765))
- Added an opt-in flag to the uninstall script to revoke Accessibility, Screen Recording, and Automation permission grants on macOS during uninstall. ([#1759](https://github.com/trycua/cua/pull/1759))

## Fixes

- The install script's default backend on macOS was reverted to the Swift implementation due to permission-system issues in the Rust implementation; the Rust implementation remains available via an explicit backend flag, and non-macOS installs still default to the Rust implementation. ([#1762](https://github.com/trycua/cua/pull/1762))
- The local development installer now produces a proper application bundle on macOS instead of a bare binary, so Accessibility and Screen Recording permission grants persist across rebuilds instead of resetting. ([#1758](https://github.com/trycua/cua/pull/1758))
- check_permissions run from a terminal on macOS no longer triggers a permission prompt misattributed to the terminal; it now reports status only and instructs the user to launch the background service for a correctly attributed prompt. ([#1760](https://github.com/trycua/cua/pull/1760))
- get_window_state now caps traversal at 2000 elements and times out after 30 seconds, preventing indefinite hangs when inspecting complex applications such as Arc, Safari with many tabs, or Electron apps. ([#1754](https://github.com/trycua/cua/pull/1754)) Thanks @hippoley, @obaid.
- launch_app on macOS now returns a structured error when an application or local file target cannot be found, instead of allowing macOS to show a persistent system dialog. ([#1719](https://github.com/trycua/cua/pull/1719)) Thanks @LexClaw, @RitwijParmar.
- The cursor position reported after a click is now correctly synced with the actual on-screen cursor, fixing cases where the reported position was null after clicking. ([#1769](https://github.com/trycua/cua/pull/1769))
- The uninstall.ps1 one-liner no longer fails to parse when piped through Invoke-Expression; force mode is now controlled with an environment variable instead of a script parameter. ([#1750](https://github.com/trycua/cua/pull/1750))

## Contributors

Thanks to @hippoley, @LexClaw, @obaid, @RitwijParmar for contributing to this release.

## Full changelog

[cua-driver-rs-v0.3.2...cua-driver-rs-v0.3.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.2...cua-driver-rs-v0.3.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.3.4

- Tag: `cua-driver-rs-v0.3.4`
- Release ID: `331875197`
- Original body SHA256: `29c3203eef3683b1ad3333a5d335281cc899379d442012b5c7a6bd59c2e44ae5`
- Proposed body SHA256: `27254cb44d75a7c1e28bbb2a71f0f81e48e3d2ba436d0b93b295c638b6ddd78e`

<!-- cua-release-backfill:v1:start evidence-sha256=a3f23f183c585132638dcfd7cf989910e9cec077f87a0acc191b4835c304e402 -->
## Summary

This release changes the default install script to install the Rust implementation on all platforms.

## Fixes

- The install script now defaults to installing the Rust implementation on all platforms; the Swift implementation remains available on macOS via an explicit backend flag. ([#1768](https://github.com/trycua/cua/pull/1768))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.3.3...cua-driver-rs-v0.3.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.3...cua-driver-rs-v0.3.4)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.3.5

- Tag: `cua-driver-rs-v0.3.5`
- Release ID: `332010241`
- Original body SHA256: `e566c8aaedd901e97bfe8b50da4bbf923fa3cc59da6de33373a8303679be3ec7`
- Proposed body SHA256: `242b36159752250fa79635b924d10f075248a1398c5b2ef847c46e2a2c0e0b6b`

<!-- cua-release-backfill:v1:start evidence-sha256=90e5e91fddbce24c122f6e39da9b4fdfcd1c39d033c7550a4a376ddc7d97b973 -->
## Summary

This release fixes a macOS startup issue where the background service could be unreachable for minutes when permissions had not yet been granted, and corrects the install script to fetch the latest release.

## Fixes

- On macOS, the background service now binds its communication socket before waiting on the permissions prompt, fixing cases where the service was unreachable for minutes on first launch until Accessibility and Screen Recording permissions were granted. ([#1773](https://github.com/trycua/cua/pull/1773))
- The default install script now correctly installs the latest release instead of remaining stuck on an older version due to a release automation failure. ([#1770](https://github.com/trycua/cua/pull/1770))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.3.4...cua-driver-rs-v0.3.5](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.4...cua-driver-rs-v0.3.5)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.3.6

- Tag: `cua-driver-rs-v0.3.6`
- Release ID: `332013359`
- Original body SHA256: `64dc50558684e64f14283555e40409f71f0dd248f4cc28aef1e0fc15ec0d7403`
- Proposed body SHA256: `6191b1e2c0171aeb05b4a3fc7b48a075724b2c265c9eec1ad3f36a46308191a0`

<!-- cua-release-backfill:v1:start evidence-sha256=4200a9b0cda55304973cc02568043add91b8aac6b4f995e8bd7e59d385af4773 -->
## Summary

This patch fixes a false positive in permissions status reporting on macOS.

## Fixes

- Fixed cua-driver permissions status incorrectly reporting Accessibility and Screen Recording as granted when run standalone from a terminal; it now reports an unknown status with guidance when the daemon is not running instead of falsely claiming permissions were granted. ([#1774](https://github.com/trycua/cua/pull/1774))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.3.5...cua-driver-rs-v0.3.6](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.5...cua-driver-rs-v0.3.6)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.4.0

- Tag: `cua-driver-rs-v0.4.0`
- Release ID: `332047095`
- Original body SHA256: `704651e0d2ec99047a42e9458cc3d455d6ea73926e1a55ea27acb5fcc25413ff`
- Proposed body SHA256: `d0e4e8b200f8dcf7f7092f00365fa52547c27f0479119b32835dafd94cb8bba1`

<!-- cua-release-backfill:v1:start evidence-sha256=dc12511b8b16e1ca10bafc2f11509c909e25e25a39e48a9f3891284a2c82b95c -->
## Summary

This release isolates recording and configuration state per daemon session so concurrent MCP sessions no longer interfere with each other.

## Features

- Added per-session identity to the daemon so that starting or stopping a recording, or changing configuration, in one session no longer clobbers another concurrent session's state. ([#1776](https://github.com/trycua/cua/pull/1776))
- Video recording now starts off by default for a recording session. ([#1776](https://github.com/trycua/cua/pull/1776))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.3.6...cua-driver-rs-v0.4.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.3.6...cua-driver-rs-v0.4.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.4.1

- Tag: `cua-driver-rs-v0.4.1`
- Release ID: `332073416`
- Original body SHA256: `82c82f713fadae1fe63725257df7d79312abdf47e292821a87019ee88492de81`
- Proposed body SHA256: `25642efb01b34eb8038cb8d0fa302173e367265b05d24c42729504687a8f8f5a`

<!-- cua-release-backfill:v1:start evidence-sha256=17053532cb0cbf356c653324f4d14bc36545db7016bdf5460e7af36cb3a9dd09 -->
## Summary

This release adds distinct per-session agent cursors on macOS and fixes a headless-environment crash and a missing application icon.

## Features

- Added per-session agent cursors on macOS so concurrent sessions each render their own distinctly colored cursor instead of sharing and overwriting one overlay. ([#1779](https://github.com/trycua/cua/pull/1779))

## Fixes

- Fixed cua-driver mcp crashing with SIGABRT when launched in a context without Window Server access, such as over SSH or as a headless daemon; it now degrades to running headless instead of aborting. ([#1781](https://github.com/trycua/cua/pull/1781)) Thanks @werchoaai.
- Fixed the macOS CuaDriver.app bundle showing a blank generic icon in Finder and Launchpad by shipping a proper application icon. ([#1780](https://github.com/trycua/cua/pull/1780)) Thanks @yhy2049.

## Contributors

Thanks to @werchoaai, @yhy2049 for contributing to this release.

## Full changelog

[cua-driver-rs-v0.4.0...cua-driver-rs-v0.4.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.4.0...cua-driver-rs-v0.4.1)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.4.2

- Tag: `cua-driver-rs-v0.4.2`
- Release ID: `332153252`
- Original body SHA256: `bd459c7862cf5826853aab86c84051132dd0c26e176632f6b08ed1cd543a0473`
- Proposed body SHA256: `4ae96a8127f80a07507bcc50a4e819191893b39dfcf67580fdf0441b2b18009c`

<!-- cua-release-backfill:v1:start evidence-sha256=b131b8b8e9bd83e962c2728e4e39c6b08a08341fcb9db163021bb9f8b24d9b76 -->
## Summary

This release improves accessibility support for Chromium and Electron apps and fixes a crash on macOS 14.

## Features

- Enabled accessibility trees for Chromium- and Electron-based applications so get_window_state returns the full window content instead of only the title bar on first access. ([#1756](https://github.com/trycua/cua/pull/1756))

## Fixes

- Fixed hotkey, press_key, and scroll crashing the daemon on macOS 14 (Sonoma) due to an unrecognized selector; the driver now checks API availability and falls back gracefully on older macOS versions. ([#1782](https://github.com/trycua/cua/pull/1782)) Thanks @hippoley, @saved-j.

## Contributors

Thanks to @hippoley, @saved-j for contributing to this release.

## Full changelog

[cua-driver-rs-v0.4.1...cua-driver-rs-v0.4.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.4.1...cua-driver-rs-v0.4.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.4.3

- Tag: `cua-driver-rs-v0.4.3`
- Release ID: `332231382`
- Original body SHA256: `be43c969ad7c1177ec6ffec2123fa6511674b05747799da8232e9ad75e5dfc76`
- Proposed body SHA256: `5c4171231cf1163b010bc95edf5a1a30d61d7bef1d6dcf61865556c83c7b77b4`

<!-- cua-release-backfill:v1:start evidence-sha256=c3cc93d1c537a9d0018d39db17fcf20daf221f4fd96fcf17f9a949b991e609ba -->
## Summary

This release fixes the agent cursor not appearing during real MCP sessions, stops a repeated permissions prompt, and ensures the cursor overlay renders when using the daemon.

## Fixes

- Fixed the agent cursor not appearing during MCP sessions by wiring per-session cursor identity through the real cua-driver mcp path and seeding the cursor on-screen so the first action glides into view instead of only snapping in place. ([#1787](https://github.com/trycua/cua/pull/1787))
- Fixed the macOS permissions grant flow repeatedly re-prompting for Accessibility and Screen Recording access roughly every 25 seconds instead of prompting once. ([#1791](https://github.com/trycua/cua/pull/1791))
- Fixed the agent cursor overlay not rendering when cua-driver ran through the daemon-proxy setup by running the overlay loop inside the serve daemon. ([#1790](https://github.com/trycua/cua/pull/1790))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.4.2...cua-driver-rs-v0.4.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.4.2...cua-driver-rs-v0.4.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.0

- Tag: `cua-driver-rs-v0.5.0`
- Release ID: `332641473`
- Original body SHA256: `e48d42d79616bbf390f9ae10fd48f136b86fad8f5ee3580dd2f4fde3c68502f6`
- Proposed body SHA256: `f0f0c1658fc35bc34ddcceb08e747b2e8061c8e903db80710c6b9ea2953f0a02`

<!-- cua-release-backfill:v1:start evidence-sha256=158938d6eb2fe2f569aefac5bd890b31fa3fdfa9add6e0ee624c64095a65bf74 -->
## Summary

This release adds caller-declared session identity with an HTTP transport for running multiple agents in parallel, brings per-session agent cursors to Windows, and fixes a daemon crash under concurrent sessions.

## Features

- Added explicit, caller-declared session identity and a Streamable-HTTP MCP transport, allowing multiple agents to run in true parallel with independent sessions, cursors, and automatic idle-timeout cleanup. ([#1798](https://github.com/trycua/cua/pull/1798))
- Added per-session agent cursors on Windows so concurrent sessions each render their own distinctly colored cursor instead of sharing one overlay. ([#1801](https://github.com/trycua/cua/pull/1801))

## Fixes

- Fixed a daemon crash on macOS caused by a use-after-free when concurrent sessions accessed the same window during accessibility actions like clicking, typing, and scrolling. ([#1796](https://github.com/trycua/cua/pull/1796))
- Fixed local macOS installs becoming stuck reporting permissions as not granted after a rebuild, even though System Settings showed them enabled, by resetting a permission grant that was pinned to a previous signing identity. ([#1795](https://github.com/trycua/cua/pull/1795))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.4.3...cua-driver-rs-v0.5.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.4.3...cua-driver-rs-v0.5.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.1

- Tag: `cua-driver-rs-v0.5.1`
- Release ID: `332905972`
- Original body SHA256: `2a3800163a944345c8eabcb9e781073ea0f476e8da4340b85f9e79169abde9cd`
- Proposed body SHA256: `55be1184f96fedb5511c881307d3fee1befffd7ed2036995ba44ac98c416638b`

<!-- cua-release-backfill:v1:start evidence-sha256=d04303bdbd12aecf6d4b7f6b9d56553e86bd284a75df0608bd014030132f1fef -->
## Summary

This release fixes the release installer to use a consistent installation directory and clean up conflicting local installs.

## Fixes

- Fixed the release installer using a different installation directory than the local installer and runtime, and added cleanup of a prior local install so the two no longer conflict. ([#1803](https://github.com/trycua/cua/pull/1803))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.5.0...cua-driver-rs-v0.5.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.0...cua-driver-rs-v0.5.1)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.2

- Tag: `cua-driver-rs-v0.5.2`
- Release ID: `334709141`
- Original body SHA256: `eb7f11a6ce4ad54b22543e8271624217596f759145cfe725e9a2a6bbd62d1d89`
- Proposed body SHA256: `23cb320da32585ad326cc569a447a5a6837dc5cb3d9c53b26581f1d08d5c2d20`

<!-- cua-release-backfill:v1:start evidence-sha256=25516e3a2b13c23e908bb788185d9daf6e55c992c5ec325e6640f6b7ccc0f5d7 -->
## Summary

This release adds Windows background computer-use input that avoids window activation, brings a focus-free background text-entry technique to GTK4 apps on Linux, and fixes a Windows coordinate offset bug at 125%, 150%, and 200% display scaling.

## Features

- Background input on Windows can now click and type into Chromium, WPF, WinForms, Electron, and GTK applications without raising the window or moving the visible cursor, with automatic fallback to touch injection when needed. ([#1809](https://github.com/trycua/cua/pull/1809))
- Added GTK4 support for truly focus-free background text entry using synthetic focus events, extending the technique already used for Qt5. ([#1814](https://github.com/trycua/cua/pull/1814))

## Fixes

- Added a DPI-awareness manifest to the Windows driver, fixing an issue where click and screenshot coordinates were offset when the display was scaled to 125%, 150%, or 200%. ([#1821](https://github.com/trycua/cua/pull/1821))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.5.1...cua-driver-rs-v0.5.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.1...cua-driver-rs-v0.5.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.3

- Tag: `cua-driver-rs-v0.5.3`
- Release ID: `338387841`
- Original body SHA256: `07864827697d051f088177044ea7f9f15011a7c945ae0739762cfc9e617805ac`
- Proposed body SHA256: `26e6bbad1b25de9013d0c2ba5512abe0662e352e944282fde66370b7baa441b9`

<!-- cua-release-backfill:v1:start evidence-sha256=aff0289a119b85b2b5ed9622e8394bda9ae4e2a6f5d5e97009cbfaa37563f439 -->
## Summary

This release adds Linux background drag and held-button cursor support and enables the driver to show the cursor and type text into background terminal windows.

## Features

- Added background drag and held-button (pressed) cursor support on Linux, along with a refactored overlay backend that supports multiple simultaneous cursors. ([#1871](https://github.com/trycua/cua/pull/1871))
- The Linux driver can now animate the cursor to on-screen coordinates and type text directly into background terminal windows. ([#1789](https://github.com/trycua/cua/pull/1789)) Thanks @hippoley.

## Contributors

Thanks to @hippoley for contributing to this release.

## Full changelog

[cua-driver-rs-v0.5.2...cua-driver-rs-v0.5.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.2...cua-driver-rs-v0.5.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.4

- Tag: `cua-driver-rs-v0.5.4`
- Release ID: `339940302`
- Original body SHA256: `60cd6afcdf0cee139a0375e5ef3ae75a6c8ad0a74c954ab0d6d1668578caeeaa`
- Proposed body SHA256: `f13347e8d37556311dac1db59775c722cc701065d6803b420b2e1964d8d69db1`

<!-- cua-release-backfill:v1:start evidence-sha256=66927d555fbdc5405238cb35387c28a1792c849ea13fa09fc3cb08bdf22f3adf -->
## Summary

This release fixes a macOS bug where screenshots and window captures returned blank, zero-sized images on macOS 15 and later.

## Fixes

- Restored the screen-capture entitlement on the signed macOS release binary and app bundle, fixing screenshots and window-state captures that were returning blank 0x0 images on macOS 15 and later even when Screen Recording permission was granted. ([#1906](https://github.com/trycua/cua/pull/1906))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.5.3...cua-driver-rs-v0.5.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.3...cua-driver-rs-v0.5.4)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.5

- Tag: `cua-driver-rs-v0.5.5`
- Release ID: `339982639`
- Original body SHA256: `7935b46a50e62940461552fa1f39d7b08aa610709db3ef50430e5b220e743c4a`
- Proposed body SHA256: `c8d1d3e66346fb770807154f28e97c1d539dd13d4660eb935f313d616b7d4104`

<!-- cua-release-backfill:v1:start evidence-sha256=75efeb028e0072d7f62a65bb9aea5a19d5fcfa38f7d8a306c4f8e87cb622428e -->
## Summary

This release fixes two Windows coordinate-accuracy issues: incorrect zoom crops when window downscaling is active, and screenshot and click misalignment on high-DPI and multi-monitor displays.

## Fixes

- The zoom tool now scales the requested crop region by the window's resize ratio before cropping, fixing zoom captures that showed the wrong region of the screen when window downscaling was active. ([#1884](https://github.com/trycua/cua/pull/1884)) Thanks @tobitege.
- Corrected the Windows DPI-awareness manifest so it is actually applied by the system loader, and removed the resulting double-scaling in screen and window capture, fixing screenshot and click misalignment on high-DPI and multi-monitor Windows setups. ([#1883](https://github.com/trycua/cua/pull/1883)) Thanks @tobitege.

## Contributors

Thanks to @tobitege for contributing to this release.

## Full changelog

[cua-driver-rs-v0.5.4...cua-driver-rs-v0.5.5](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.4...cua-driver-rs-v0.5.5)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.6

- Tag: `cua-driver-rs-v0.5.6`
- Release ID: `341116505`
- Original body SHA256: `4db8a7b4c0cd8fa103f1b6bba07ff527bcfe6d93a5f7f4eec52304c701ade68d`
- Proposed body SHA256: `77400011ddba935eea3e10f0a965269683c32f1c69c2cb6bf0a606b210b3de75`

<!-- cua-release-backfill:v1:start evidence-sha256=98f21e5827ff4e889fea75eb8a7e59e939a5ef2d4152555f93af94f8a66c1ffe -->
## Summary

This release adds native Wayland support and automatic accessibility enablement for Chromium and Electron apps on Linux, fixes incorrect element coordinates in GTK4 apps, and stops the macOS cursor overlay from consuming CPU while idle.

## Features

- Added native Wayland support for background input on wlroots-based compositors. ([#1910](https://github.com/trycua/cua/pull/1910))
- The Linux driver now enables accessibility support for Chromium and Electron-based applications, such as VS Code, Slack, Discord, and Obsidian, at startup, so these apps are no longer invisible to the driver's accessibility tree. ([#1930](https://github.com/trycua/cua/pull/1930))

## Fixes

- Fixed incorrect element screen coordinates reported for GTK4 applications on Linux, which had prevented element-targeted clicks and the cursor overlay from aiming correctly. ([#1931](https://github.com/trycua/cua/pull/1931))
- The macOS agent-cursor overlay no longer renders frames continuously while idle, eliminating unnecessary CPU usage when the cursor is not active. ([#1865](https://github.com/trycua/cua/pull/1865)) Thanks @redwine99, @zviratko.

## Contributors

Thanks to @redwine99, @zviratko for contributing to this release.

## Full changelog

[cua-driver-rs-v0.5.5...cua-driver-rs-v0.5.6](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.5...cua-driver-rs-v0.5.6)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.5.7

- Tag: `cua-driver-rs-v0.5.7`
- Release ID: `341163398`
- Original body SHA256: `9c1fca738451aebd6ce914cd32581a3ae04ca4e36558b83a85389ba4c8d4c109`
- Proposed body SHA256: `6cfc04bb0c71bf0dae2efe990d4c1ff8fe7c4f9a4056e916e08b40693016877c`

<!-- cua-release-backfill:v1:start evidence-sha256=465b1954fea40f6839c268f7581d940ddbe8a1e5070ca593c8fca257ec0316a5 -->
## Summary

This release fixes a Linux configuration bug where settings failed to persist, disables the experimental native Wayland backend by default until it is more complete, and fixes idle CPU usage and an orphaned process issue in the Windows cursor overlay.

## Fixes

- Fixed Linux configuration settings such as max_image_dimension and capture_mode silently failing to persist when set using the key/value format. ([#1928](https://github.com/trycua/cua/pull/1928))
- The experimental native Wayland backend is now disabled by default and must be explicitly enabled, rather than automatically engaging on any Wayland session. ([#1935](https://github.com/trycua/cua/pull/1935))
- Fixed the Windows cursor overlay consuming significant CPU while idle, and fixed the driver process lingering as an orphan with the overlay still running after an MCP client disconnected. ([#1933](https://github.com/trycua/cua/pull/1933)) Thanks @zviratko.

## Contributors

Thanks to @zviratko for contributing to this release.

## Full changelog

[cua-driver-rs-v0.5.6...cua-driver-rs-v0.5.7](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.6...cua-driver-rs-v0.5.7)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.0

- Tag: `cua-driver-rs-v0.6.0`
- Release ID: `342640085`
- Original body SHA256: `d8083b62845e5698b132b5ed71b8fd4f16cad29c78529c84dc5ea08d87363eaf`
- Proposed body SHA256: `0e8640fa3d1f202ff59d0b6a37ac9cab0bc3c7a41a8346c260ec531cc5b3fa2b`

<!-- cua-release-backfill:v1:start evidence-sha256=3a8e0ab96a8a7471f094ae631c92a027fbed4eb499c9bb379376871d2a231135 -->
## Summary

This release overhauls the cursor overlay with retina-aware rendering and selectable cursor shapes, fixes text input in terminal emulators, expands the MCP tool surface for third-party agent integrations, adds a unified health_report diagnostics tool, and adds Hermes agent skill autodetection.

## Features

- The agent cursor overlay now renders at native retina resolution and supports selecting between arrow and teardrop cursor shapes. ([#1961](https://github.com/trycua/cua/pull/1961))
- Expanded the MCP tool surface with per-tool capability lists, a machine-readable CLI manifest, explicit image response types, a click button enum, and opaque element tokens, letting third-party agent integrations decouple from internal driver details. ([#1961](https://github.com/trycua/cua/pull/1961))
- Added a health_report MCP tool that returns a single diagnostic report covering driver version, platform support, permissions, and capability status. ([#1908](https://github.com/trycua/cua/pull/1908))
- The skills installer now autodetects the Hermes agent and symlinks the cua-driver skill pack into its skills directory. ([#1963](https://github.com/trycua/cua/pull/1963))

## Fixes

- Typing text now falls back to direct key-event synthesis when the target window is a terminal emulator, fixing silent text-input failures in terminals such as Ghostty, iTerm2, Terminal.app, and Windows Terminal. ([#1961](https://github.com/trycua/cua/pull/1961))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.5.8...cua-driver-rs-v0.6.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.5.8...cua-driver-rs-v0.6.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.2

- Tag: `cua-driver-rs-v0.6.2`
- Release ID: `343016113`
- Original body SHA256: `8d6dfb346b9ff9b1ce1404e5254b279302c66f012412aa3e3e602245b790fbdc`
- Proposed body SHA256: `44dc25116045017f919584b168adb47900897871de05275adb314b061c8aac39`

<!-- cua-release-backfill:v1:start evidence-sha256=92d422fc1437837255fd33d151680c0113064b2489196fe26e42c72e111805e2 -->
## Summary

This patch makes the Linux release pipeline compatible with the Debian bullseye build container by feature-gating optional desktop-portal and libei input backends, allowing Linux release artifacts to build without raising the minimum required glibc version.

## Fixes

- Feature-gate the desktop-portal and libei input backends so Linux release builds no longer require a newer PipeWire/libei toolchain, preserving compatibility with Debian 11, Ubuntu 22.04, and RHEL/Rocky 9. ([#1967](https://github.com/trycua/cua/pull/1967))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.6.1...cua-driver-rs-v0.6.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.1...cua-driver-rs-v0.6.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.3

- Tag: `cua-driver-rs-v0.6.3`
- Release ID: `343115974`
- Original body SHA256: `958ce3847bdf24e6b6ae95bee2a0414a239cad6673e4f79574bd92673823baf3`
- Proposed body SHA256: `3fd9b5f023ab07362f1811aa75e0396b3e9672ea27425169b010cc898cb579ab`

<!-- cua-release-backfill:v1:start evidence-sha256=6251b3c0d5c875bd3e3e2a4052a9182e4e1606ab9277ba7db69ab9f9b73ab8c0 -->
## Summary

This patch fixes a black-screenshot bug when capturing minimized windows on Windows and completes uninstall cleanup for additional AI agent skill directories.

## Fixes

- Return a clear error instead of a blank black screenshot when capturing a minimized window on Windows. ([#1974](https://github.com/trycua/cua/pull/1974))
- Uninstall scripts now also remove skill links for the Antigravity and Hermes agents, preventing orphaned symlinks after uninstall. ([#1972](https://github.com/trycua/cua/pull/1972))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.6.2...cua-driver-rs-v0.6.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.2...cua-driver-rs-v0.6.3)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.4

- Tag: `cua-driver-rs-v0.6.4`
- Release ID: `343130778`
- Original body SHA256: `27b8fa6f0461305cce101d1d9837a61146ece2292f9ce8f330ed4bb9157293ab`
- Proposed body SHA256: `65485dd8fad130e234d49910315b13aaa2ca611591de586fad97c2c31e1c037c`

<!-- cua-release-backfill:v1:start evidence-sha256=3ca2bd5cc814c1f4f718a1abbe08e21bb8a23f5cd74164b8a02a1e88f621392a -->
## Summary

This patch surfaces screenshot capture failures instead of silently dropping them when reporting Windows window state.

## Fixes

- Windows window-state responses now include a clear error message when a screenshot cannot be captured, instead of silently omitting it. ([#1977](https://github.com/trycua/cua/pull/1977))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.6.3...cua-driver-rs-v0.6.4](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.3...cua-driver-rs-v0.6.4)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.5

- Tag: `cua-driver-rs-v0.6.5`
- Release ID: `343247819`
- Original body SHA256: `e96db92d341c68a8848a2b49c7decdca56ac692dd34ee31c6b566052067db99c`
- Proposed body SHA256: `85110b0d5527a3cc16c56eb1ee6cb01e2e641f6ba47f94a03a5fece1ecddbbda`

<!-- cua-release-backfill:v1:start evidence-sha256=0f5bd66079d0dc23ef415dd5b3e8dfc58220cad9b5bfa8e82a47c6f8bc9f4f43 -->
## Summary

This patch fixes incorrect click placement on multi-monitor Windows setups with negative-offset secondary monitors and hardens mouse coordinate packing against out-of-range values.

## Fixes

- Corrected a fence-post error in multi-monitor coordinate normalization on Windows that could send synthesized clicks to the wrong monitor near screen boundaries. ([#1980](https://github.com/trycua/cua/pull/1980)) Thanks @1mestre.
- Windows mouse input coordinate packing now clamps and logs out-of-range values instead of silently corrupting them. ([#1983](https://github.com/trycua/cua/pull/1983))

## Contributors

Thanks to @1mestre for contributing to this release.

## Full changelog

[cua-driver-rs-v0.6.4...cua-driver-rs-v0.6.5](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.4...cua-driver-rs-v0.6.5)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.6

- Tag: `cua-driver-rs-v0.6.6`
- Release ID: `343862036`
- Original body SHA256: `9506132864b41028c8eb9df12faae72c40c17ac29b4268956ce42f94f99ddde0`
- Proposed body SHA256: `a96b01cb993f2412d3a4e24fe823949d36ae9309bd38e6f271b5b468d33961c7`

<!-- cua-release-backfill:v1:start evidence-sha256=15ccbba8eb0a8f510ca4014349b4c209ff83ced930242dbdd5a93f69b3cc62cb -->
## Summary

This release fixes several Linux and Windows input reliability issues, including dead-input detection on GNOME/KDE Wayland, SSH-driven Wayland+Xwayland authentication, double and right-click delivery to Chromium apps, transient UI Automation cache failures, a Windows installer temp-path failure, and a stalled daemon proxy on slow requests.

## Fixes

- Double-click and right-click on Chromium and Electron apps on Windows, such as Obsidian, VS Code, and Slack, now actually deliver clicks instead of silently doing nothing. ([#1995](https://github.com/trycua/cua/pull/1995))
- On GNOME and KDE Wayland desktops without libei/portal input support, input actions now return a clear error instead of silently failing. ([#1992](https://github.com/trycua/cua/pull/1992))
- On Linux desktops running pure Wayland with no working input backend, input actions now fail with an actionable error instead of reporting false success. ([#1994](https://github.com/trycua/cua/pull/1994))
- Windows UI Automation tree queries now retry transient cache-build failures instead of returning an empty element tree. ([#1996](https://github.com/trycua/cua/pull/1996)) Thanks @tobitege.
- The daemon proxy no longer treats a socket read timeout as a fatal error, fixing failures on large or slow responses such as full-screen captures and slow accessibility scans. ([#1997](https://github.com/trycua/cua/pull/1997))
- Linux window-state queries now retry when a Qt6 application's accessibility tree is still empty right after a cold launch, instead of returning only the root window. ([#1998](https://github.com/trycua/cua/pull/1998))
- Driving a Wayland and Xwayland desktop over SSH now auto-discovers the X authentication cookie, fixing X11 tool failures such as list_windows returning empty results. ([#1999](https://github.com/trycua/cua/pull/1999))
- The Windows installer no longer fails when the system temp directory resolves to a missing 8.3 short path. ([#1990](https://github.com/trycua/cua/pull/1990)) Thanks @tobitege.

## Contributors

Thanks to @tobitege for contributing to this release.

## Full changelog

[cua-driver-rs-v0.6.5...cua-driver-rs-v0.6.6](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.5...cua-driver-rs-v0.6.6)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.7

- Tag: `cua-driver-rs-v0.6.7`
- Release ID: `343892996`
- Original body SHA256: `369d65c5a9695059cd1d0b0c71dcd6bc68288808764633d8038199787bae650a`
- Proposed body SHA256: `f4f03defc3a597f5634baa551dd9e259ae0a8236a0716d505707c264b43a112a`

<!-- cua-release-backfill:v1:start evidence-sha256=a61dc70b0c47e225194e2b65467924f8960d9eaf6d8d36c1a0466f32af7cf935 -->
## Summary

This patch returns a clear error instead of a zero-size or null screen dimension when no usable display is detected on Linux, such as under WSL without a display attached.

## Fixes

- Linux screen-size and screenshot calls now fail with an actionable error instead of returning 0x0 dimensions or a blank capture when no usable display, such as WSL without WSLg, is available. ([#2006](https://github.com/trycua/cua/pull/2006))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.6.6...cua-driver-rs-v0.6.7](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.6...cua-driver-rs-v0.6.7)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.6.8

- Tag: `cua-driver-rs-v0.6.8`
- Release ID: `344385110`
- Original body SHA256: `26442aec810e7d3409ef0cc334db8378e769fdc644834c47609dca49e53acbb4`
- Proposed body SHA256: `d649d760fed013f495f28e0230c2123407ec631860745e2d5e765d92980896b4`

<!-- cua-release-backfill:v1:start evidence-sha256=8fff061ae47535937918689690cdd4d87b868acb97f60cb0c3c7228c480c2ace -->
## Summary

This release applies a dependency security rollup addressing vulnerable dependencies used by the project, including the Rust driver.

## Fixes

- Applied a dependency security update rollup to remove known vulnerabilities in project dependencies. ([#2000](https://github.com/trycua/cua/pull/2000))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.6.7...cua-driver-rs-v0.6.8](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.7...cua-driver-rs-v0.6.8)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.7.0

- Tag: `cua-driver-rs-v0.7.0`
- Release ID: `347259948`
- Original body SHA256: `620357004bd9cf0654e7551c1ac6bfb9b32312f0e12a0433fa7ee9157901df5c`
- Proposed body SHA256: `5c9425d5fa8684bb3dd5db2e8866c5ce6996d450ba65c7808dd5188a9b92305f`

<!-- cua-release-backfill:v1:start evidence-sha256=f3fe236d800f2d0c8efc9550614f5d216627729811cd3a652f845012f5c73b21 -->
## Summary

cua-driver-rs 0.7.0 introduces action-time modality selection with honest verification of whether an action actually took effect, adds browser DOM control, extends screen-absolute desktop-scope actions to macOS and Linux, and retires the Swift implementation in favor of Rust.

## Features

- Actions now report whether they were verified, which input path was used, the observed effect, and an escalation suggestion when an action's effect cannot be confirmed (for example when a web or Electron app's accessibility tree falsely echoes success). ([#2077](https://github.com/trycua/cua/pull/2077))
- Added browser page control (reading text, querying the DOM, clicking elements, running JavaScript, and two text-entry methods) so actions can be verified and applied directly against web content. ([#2077](https://github.com/trycua/cua/pull/2077))
- Perception now always returns both the accessibility tree and a screenshot in one call; the separate capture-mode selection for perception is deprecated in favor of choosing addressing (element vs. pixel coordinates) at the time of each action. ([#2077](https://github.com/trycua/cua/pull/2077))
- Added a desktop-scope mode that captures the full screen and supports window-less, screen-absolute clicking and scrolling, first shipped for Windows. ([#2019](https://github.com/trycua/cua/pull/2019))
- Extended window-less, screen-absolute clicking to macOS and Linux, unifying desktop-scope support across all three platforms. ([#2056](https://github.com/trycua/cua/pull/2056))
- Added support for background accessibility clicks and cursor-accurate pixel clicks on Linux Wayland sessions without direct pointer injection. ([#2077](https://github.com/trycua/cua/pull/2077))
- Added Qwen Code, Factory Droid, and ZCode as supported clients for MCP configuration setup. ([#2077](https://github.com/trycua/cua/pull/2077))

## Fixes

- Configuration changes made with set_config now persist to disk on Windows and Linux, so settings survive across separate command invocations instead of reverting to defaults. ([#2034](https://github.com/trycua/cua/pull/2034)) Thanks @outdog-hwh.
- set_config on macOS now also accepts the key/value input shape already supported on Windows and Linux, fixing cases where configuration updates silently had no effect on macOS. ([#2059](https://github.com/trycua/cua/pull/2059))
- Fixed desktop state capture failing with a display-not-set error on pure Wayland Linux sessions. ([#2047](https://github.com/trycua/cua/pull/2047))
- Fixed key presses failing on Linux systems with a sparse or headless keyboard layout that lacked a mapping for the requested key. ([#2048](https://github.com/trycua/cua/pull/2048))
- The visible agent cursor now moves to the click location for window-less desktop-scope clicks on Linux, instead of remaining stationary while the click lands elsewhere. ([#2061](https://github.com/trycua/cua/pull/2061))
- Window listings on Linux now include bounds, application name, and on-screen status fields matching the format already returned on macOS and Windows. ([#2018](https://github.com/trycua/cua/pull/2018))

## Contributors

Thanks to @outdog-hwh for contributing to this release.

## Full changelog

[cua-driver-rs-v0.6.8...cua-driver-rs-v0.7.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.6.8...cua-driver-rs-v0.7.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.7.1

- Tag: `cua-driver-rs-v0.7.1`
- Release ID: `350399358`
- Original body SHA256: `25611ca4218c534da2552fb9c7e6bef393511fe549ea5d6d10fc4c3956017ec8`
- Proposed body SHA256: `dee35bcf1da7a25602bf8d282429cf12dd513ee628d319e7c9ea525df510c69f`

<!-- cua-release-backfill:v1:start evidence-sha256=ed2a270ffeeed9df6ccc37a20202b9c252546a4d2cdb1b248fa89068a412422b -->
## Summary

This release adds an embedded mode that lets a host macOS application share its own Accessibility and Screen Recording permissions with cua-driver, adds macOS page click support with improved Chromium window targeting, returns structured launch identity from Linux app launches, retries daemon socket writes instead of failing on temporary backpressure, and resets macOS permission grants by default on uninstall.

## Features

- Embedded mode lets a host application run cua-driver as a child process that inherits the host's Accessibility and Screen Recording grants, avoiding a second permission prompt for the driver. ([#2102](https://github.com/trycua/cua/pull/2102))
- Added macOS support for clicking page elements, along with a fix so browser JavaScript targets the correct Chrome window. ([#2082](https://github.com/trycua/cua/pull/2082)) Thanks @gabriel.

## Fixes

- Linux app launches now return the process ID and other launch identity as structured data instead of only in the text response. ([#2091](https://github.com/trycua/cua/pull/2091)) Thanks @cm2435-hcomp.
- Daemon socket writes now retry through temporary backpressure instead of failing with a resource-unavailable error when several tools run at once. ([#2036](https://github.com/trycua/cua/pull/2036)) Thanks @LaZzyMan.
- Uninstalling cua-driver now resets macOS Accessibility and Screen Recording permission grants by default, with a new option to preserve them if needed. ([#2108](https://github.com/trycua/cua/pull/2108))

## Contributors

Thanks to @cm2435-hcomp, @gabriel, @LaZzyMan for contributing to this release.

## Full changelog

[cua-driver-rs-v0.7.0...cua-driver-rs-v0.7.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.7.0...cua-driver-rs-v0.7.1)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.8.0

- Tag: `cua-driver-rs-v0.8.0`
- Release ID: `354083612`
- Original body SHA256: `f709baacb1f44a75cb37dca16430249984216c0f519918bf8d08eae15156cb73`
- Proposed body SHA256: `063b8dde4ab8d74759b040e862142abdd94d4b1b520a13d3fe3fefbb2dc4ad8d`

<!-- cua-release-backfill:v1:start evidence-sha256=390d97503f9c4f41e9edd3f521b9e34d43ecde91ce4df3a78ba51314eb038044 -->
## Summary

This release adds consent-aware telemetry to Cua Driver and Lume, fixes JavaScript execution so it targets the correct page in multi-window applications, moves installer and uninstaller downloads to cua.ai, and hardens delivery reliability on Windows, macOS, and Linux.

## Features

- Added default-on, consent-aware telemetry for Cua Driver and Lume, including commands to disable, reset, and purge collected identity data. ([#2211](https://github.com/trycua/cua/pull/2211))

## Fixes

- Page JavaScript execution now requires an explicit target and fails instead of silently running against the wrong window when one debugging port serves multiple pages. ([#2166](https://github.com/trycua/cua/pull/2166)) Thanks @hqhq1025.
- The install and uninstall scripts now fetch from cua.ai instead of the previous location. ([#2152](https://github.com/trycua/cua/pull/2152))
- Windows background actions no longer report success when input was not actually delivered, and window capture now falls back through GDI and Windows Graphics Capture with an honest unavailable result for minimized windows. ([#2180](https://github.com/trycua/cua/pull/2180))
- Corrected Linux Wayland window identity tracking, accessibility frame geometry, and WebKitGTK window bounds and scroll delivery so background actions target the right window position. ([#2182](https://github.com/trycua/cua/pull/2182))
- Windows UI Automation actions now scroll clipped controls into view before acting and background text entry appends to existing text instead of replacing it; macOS keyboard delivery now clears stale modifier keys before typing. ([#2193](https://github.com/trycua/cua/pull/2193))

## Contributors

Thanks to @hqhq1025 for contributing to this release.

## Full changelog

[cua-driver-rs-v0.7.1...cua-driver-rs-v0.8.0](https://github.com/trycua/cua/compare/cua-driver-rs-v0.7.1...cua-driver-rs-v0.8.0)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.8.1

- Tag: `cua-driver-rs-v0.8.1`
- Release ID: `354137944`
- Original body SHA256: `1645712dd2810802b781a4d1853fc5febdb1d7b073deedace8767e9d428ae16a`
- Proposed body SHA256: `c5b049fbcf557bf4635f2804640d47180e99a8931a4d965841b7c7a9baebfe9e`

<!-- cua-release-backfill:v1:start evidence-sha256=af06635de4084d1b0fa8b235c8f2e3691672e18ad30bf6d8483d3005efcfb478 -->
## Summary

This release hardens installer recovery paths and telemetry handling, keeping Windows recovery on the stable installer endpoint and ensuring location data collected by telemetry is limited to country level.

## Fixes

- Windows installer recovery now routes through the stable cua.ai installer endpoint instead of GitHub's releases/latest, and installer attribution hints are suppressed when telemetry is disabled. ([#2214](https://github.com/trycua/cua/pull/2214))
- Telemetry now retains only country-level location data derived from IP enrichment, discarding the raw client IP address. ([#2217](https://github.com/trycua/cua/pull/2217))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.8.0...cua-driver-rs-v0.8.1](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.0...cua-driver-rs-v0.8.1)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.8.2

- Tag: `cua-driver-rs-v0.8.2`
- Release ID: `354523858`
- Original body SHA256: `aa4f55f4f95c15522d783e2f6eae298d19f0ace1c62677c85d05774a34ef5228`
- Proposed body SHA256: `3f1a3762031e4778a04e58eff9e19a01299b2b9d51fc4ac8d8d6f904a103e2a6`

<!-- cua-release-backfill:v1:start evidence-sha256=a86cc9553d6c94f03947a2c788b9f0c5b19af1d0497755bbb29098d48e160969 -->
## Summary

This release adds comprehensive, privacy-bounded telemetry covering agent sessions, CLI and MCP tool usage, and the macOS permission gate, while ensuring tool-completion events are emitted only once.

## Features

- Added session-level telemetry that aggregates duration, tool, action, error, and transport usage in a bounded, privacy-preserving way across CLI, daemon, and MCP execution paths. ([#2227](https://github.com/trycua/cua/pull/2227))
- Added bounded telemetry for CLI operations, MCP client kinds, and cursor state, while isolating GUI test harnesses from live telemetry by default. ([#2228](https://github.com/trycua/cua/pull/2228))

## Fixes

- Corrected daemon lifecycle telemetry across macOS permission and responsibility re-execs, and added events for the macOS permission gate. ([#2224](https://github.com/trycua/cua/pull/2224))
- Unified tool-completion telemetry so only a single owner emits each event, preventing duplicate telemetry records between the MCP proxy and daemon. ([#2225](https://github.com/trycua/cua/pull/2225))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2)
<!-- cua-release-backfill:v1:end -->

# Review: Cua Driver 0.8.3

- Tag: `cua-driver-rs-v0.8.3`
- Release ID: `354790227`
- Original body SHA256: `23a1434b4c276763b247275864dc96bc537c8ba5b2b3b368a7cfa64e145b3c17`
- Proposed body SHA256: `8f022aa382d8536fda450fd2e49746f65db9f1f2d847c071b3aed531d67766d9`

<!-- cua-release-backfill:v1:start evidence-sha256=7c8b5c31acd9d6b3afe40ec50ed8cea6d9eb2ad36bf3fb0aa26c0aa6efd19e69 -->
## Summary

This release adds configurable permission policies, written in YAML or Rego, that let administrators restrict which tools the driver can execute.

## Features

- Added support for defining tool access permission policies using YAML or Rego, allowing administrators to restrict which tools the driver can execute. ([#2235](https://github.com/trycua/cua/pull/2235))

## Contributors

This release contains maintainer changes only.

## Full changelog

[cua-driver-rs-v0.8.2...cua-driver-rs-v0.8.3](https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.2...cua-driver-rs-v0.8.3)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.0

- Tag: `v0.1.0`
- Release ID: `197817285`
- Original body SHA256: `ba3788345feb14e23417852e33351c3ffd2e86aedfbf6aa725f02434e11e760a`
- Proposed body SHA256: `ab21e3cd74dd23ac45309a865d006380e2a07f2b2548ae49a6e633dae53fce15`

<!-- cua-release-backfill:v1:start evidence-sha256=844283a7f2a6f22e9f078ebf4f523f6f199a13d30108f57bbc766426218dc3b8 -->
## Summary

Initial public release of Lume, a command-line tool for creating, running, and managing lightweight macOS and Linux virtual machines.

## Features

- Initial public release of Lume, including commands to create, run, list, stop, delete, and manage virtual machines and container images. ([b92c9cd](https://github.com/trycua/cua/commit/b92c9cd5fd4f463c88be3132868cb6523762e1be))

## Contributors

This release contains maintainer changes only.

## Full changelog

[Initial release...v0.1.0](https://github.com/trycua/cua/releases/tag/v0.1.0)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.1

- Tag: `v0.1.1`
- Release ID: `197925395`
- Original body SHA256: `a4345649cc2b83e49d835108ab6ad74c9ef23e05a356e6dc7a96e499220f43d7`
- Proposed body SHA256: `62bcc34097ac91184b4b55d23f543548344fe3f5fb1fcba91d857419196bc7a3`

<!-- cua-release-backfill:v1:start evidence-sha256=4928a3d0ac7ca9b0595cd8ed530bd9e885ed96eec8607d9a5a3ddcc981e51a6d -->
## Summary

This release fixes handling of cached images that are in an unknown state.

## Fixes

- Correctly handle cached images with an unknown state instead of causing errors. ([5443076](https://github.com/trycua/cua/commit/5443076a9f78c252b6a23fe787d2599e61c83d92))

## Contributors

This release contains maintainer changes only.

## Full changelog

[History boundary...v0.1.1](https://github.com/trycua/cua/releases/tag/v0.1.1)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.5

- Tag: `v0.1.5`
- Release ID: `197998362`
- Original body SHA256: `1e67235e7ac0c757f9ba51af3a306b15b63e5ff32ae7cd09cf03cd94f51abd8a`
- Proposed body SHA256: `98df06c1d32110a31bd68737756d2e7442e43c9d9efadf6ca5120ae701e76af5`

<!-- cua-release-backfill:v1:start evidence-sha256=e2e1750d50c7c66709998484d6c2201a62f5feb6153e9f1f74279f972158166d -->
## Summary

This release fixes an issue with virtual machine name handling.

## Fixes

- Normalized virtual machine names to prevent issues caused by inconsistent naming when pulling or running VMs. ([#6](https://github.com/trycua/cua/pull/6))

## Contributors

This release contains maintainer changes only.

## Full changelog

[v0.1.4...v0.1.5](https://github.com/trycua/cua/compare/v0.1.4...v0.1.5)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.6

- Tag: `v0.1.6`
- Release ID: `198298798`
- Original body SHA256: `dee3b5f2d44a9ba299d6e7bcb664bd97d7acddae3d6879d4ad438d89ebfd3ec9`
- Proposed body SHA256: `eaee94cafa4ca37ce456e6d77d89b66fd34a91c118423c6a1414c5579c0af0ce`

<!-- cua-release-backfill:v1:start evidence-sha256=ab50cd30f48dfbc5abf84b762af960040f1cd2f80de398890da559915c61e8e6 -->
## Summary

This release adds a progress indicator when pulling image layers.

## Features

- Added a progress indicator while processing image layers during pull operations. ([#11](https://github.com/trycua/cua/pull/11))

## Contributors

This release contains maintainer changes only.

## Full changelog

[v0.1.5...v0.1.6](https://github.com/trycua/cua/compare/v0.1.5...v0.1.6)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.7

- Tag: `v0.1.7`
- Release ID: `198705167`
- Original body SHA256: `ba69c023446feedd23b5911f2df2f2e72b88d4bf7380dc99dec6bca5a6b3880c`
- Proposed body SHA256: `a0fbc3f1884c7f5bc51c81337c2f4c174f32fed221edb3eb68d4d6832043020b`

<!-- cua-release-backfill:v1:start evidence-sha256=ebf822d03663786ad8d2a237d7cc977fbb38ceaf11497106ca8bae00f57a950f -->
## Summary

This release fixes the message displayed when no virtual machines exist.

## Fixes

- Corrected the message displayed when no virtual machines exist. ([4c89d48](https://github.com/trycua/cua/commit/4c89d488a275c2ee4ef7900befe910d5afeb1082))

## Contributors

This release contains maintainer changes only.

## Full changelog

[v0.1.6...v0.1.7](https://github.com/trycua/cua/compare/v0.1.6...v0.1.7)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.8

- Tag: `v0.1.8`
- Release ID: `199243971`
- Original body SHA256: `6ddf2bcdd429462051dde5f1cfca0990586b2a3eb36c2d1e24ea3ddde0b71313`
- Proposed body SHA256: `503935bce86d590e021ec5477862677f787df2a68e53dffa777342b5547c0845`

<!-- cua-release-backfill:v1:start evidence-sha256=706a58331eba80936183148a56d890ffb3933fd3b63cbaf88e586134f367e01f -->
## Summary

This release adds the ability to set the VM display resolution.

## Features

- Added a command to set the VM display resolution. ([#25](https://github.com/trycua/cua/pull/25)) Thanks @agrieco.

## Contributors

Thanks to @agrieco for contributing to this release.

## Full changelog

[v0.1.7...v0.1.8](https://github.com/trycua/cua/compare/v0.1.7...v0.1.8)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.9

- Tag: `v0.1.9`
- Release ID: `200398866`
- Original body SHA256: `197577e239e53dd3e24c981c3cb88f80bc685faf140ea7c331c8c47f474c4d48`
- Proposed body SHA256: `9d80e93ff5384cbc102d7d0acf8f1bc0f479fc5ec49e049ffdcb2e3b393bceba`

<!-- cua-release-backfill:v1:start evidence-sha256=bcfdd05139387406aab4cd31ecba7a48a9042f9732980879f8c638707187ac72 -->
## Summary

This release adds JSON output support for the get and list commands and local network support for VNC.

## Features

- Added a --json flag to the get and list commands for machine readable output. ([#17](https://github.com/trycua/cua/pull/17)) Thanks @pepicrft.
- Added local network support for VNC connections. ([#31](https://github.com/trycua/cua/pull/31)) Thanks @yijiasu.

## Contributors

Thanks to @pepicrft, @yijiasu for contributing to this release.

## Full changelog

[v0.1.8...v0.1.9](https://github.com/trycua/cua/compare/v0.1.8...v0.1.9)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.10

- Tag: `v0.1.10`
- Release ID: `205709849`
- Original body SHA256: `629512beacad97da318e21aa1ebd088b1feff4806cb44c6474c8e80f5f695105`
- Proposed body SHA256: `5b9b8b138cf72c3ddc406167a3799d9e872c445ffb4f5236f07463a7142c792a`

<!-- cua-release-backfill:v1:start evidence-sha256=b7d622142b2314ebd6eedff9cfd686ebdf978a5cc1d5041c3f0fbf112920fa0a -->
## Summary

This release adds a recovery mode option and fixes an issue where serve failed if its port was already bound.

## Features

- Added an option to start a VM in recovery mode. ([#37](https://github.com/trycua/cua/pull/37)) Thanks @aktech.

## Fixes

- Fixed the serve command failing when its port was already bound by another process. ([#43](https://github.com/trycua/cua/pull/43))

## Contributors

Thanks to @aktech for contributing to this release.

## Full changelog

[v0.1.9...v0.1.10](https://github.com/trycua/cua/compare/v0.1.9...v0.1.10)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.11

- Tag: `v0.1.11`
- Release ID: `206108588`
- Original body SHA256: `5fb3944bfaa20bdae6535b2e066bd7e1fa8cb8cde27bba92628edd091074262f`
- Proposed body SHA256: `bc7a682422d38b74e95994358d66fc176a9ec4d5559f232423d9170e92947dd3`

<!-- cua-release-backfill:v1:start evidence-sha256=1c1a0f7761b2fcfa035d6e6925561cc41f6d75a2ddef924725a4dee40734fd89 -->
## Summary

This release restructures the project into a monorepo and introduces Computer and an Agent preview, along with a fix for a faulted directory after pulling images.

## Features

- Introduced Computer and an Agent preview as part of a restructured monorepo. ([#47](https://github.com/trycua/cua/pull/47))

## Fixes

- Fixed a faulted directory issue that occurred after pulling images. ([#51](https://github.com/trycua/cua/pull/51))

## Contributors

This release contains maintainer changes only.

## Full changelog

[v0.1.10...v0.1.11](https://github.com/trycua/cua/compare/v0.1.10...v0.1.11)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.12

- Tag: `v0.1.12`
- Release ID: `206214490`
- Original body SHA256: `e18c7ece5ac33bfeb07d5696e44e3608dc94c15fdd9a10ea600ef6a982f8cf4c`
- Proposed body SHA256: `f0fcc31acc799d5a4df75ae797f7c687f54e2c8671dd4f721844f956b7187c0f`

<!-- cua-release-backfill:v1:start evidence-sha256=ab22fdb76c4276296f2d60cc906f37d4a5c3fc046c72c83b3f8d1cc43b419cd8 -->
## Summary

This release adds support for installing Lume with ubi and mise.

## Features

- Added support for installing Lume using ubi and mise. ([#53](https://github.com/trycua/cua/pull/53))

## Contributors

This release contains maintainer changes only.

## Full changelog

[v0.1.11...v0.1.12](https://github.com/trycua/cua/compare/v0.1.11...v0.1.12)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.14

- Tag: `lume-v0.1.14`
- Release ID: `206283473`
- Original body SHA256: `91b2b9987f22eeab940e1827cc4f8a3019bd1c3f2118ac6612cfbdabe79dc98e`
- Proposed body SHA256: `a0e9f66701108cf90468c5aa23c24c2b7af0dedc37c6117decdfa51c1d81617c`

<!-- cua-release-backfill:v1:start evidence-sha256=931e6a1200851cc901636657e613da67a21d95239d56065641b3a614f3012cee -->
## Summary

This release fixes an incorrect clone endpoint URL documented in the API reference.

## Fixes

- Corrected the documented clone endpoint URL in the API reference, which previously caused clone requests to fail. ([#54](https://github.com/trycua/cua/pull/54)) Thanks @aktech.

## Contributors

Thanks to @aktech for contributing to this release.

## Full changelog

[v0.1.13...lume-v0.1.14](https://github.com/trycua/cua/compare/v0.1.13...lume-v0.1.14)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.16

- Tag: `lume-v0.1.16`
- Release ID: `206300556`
- Original body SHA256: `cc4eeaf59d11a4f3e2d8dcff10c9082bb619f0dcf9e46bc11cc8c00117f52571`
- Proposed body SHA256: `a0d1ecd93aed0d63b0d194a2d26163649fb0590705ff61bf78ae647317c26da8`

<!-- cua-release-backfill:v1:start evidence-sha256=fe9fd1eb409658ec7918b7f920286a629be0ac3e5b28c852cab90e65fea026d6 -->
## Summary

This release fixes an incorrect clone endpoint URL documented in the API reference.

## Fixes

- Corrected the documented clone endpoint URL in the API reference, which previously caused clone requests to fail. ([#54](https://github.com/trycua/cua/pull/54)) Thanks @aktech.

## Contributors

Thanks to @aktech for contributing to this release.

## Full changelog

[lume-v0.1.15...lume-v0.1.16](https://github.com/trycua/cua/compare/lume-v0.1.15...lume-v0.1.16)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.18

- Tag: `lume-v0.1.18`
- Release ID: `207017894`
- Original body SHA256: `10753f9e93023a0186f51e6dc3b14af3c2298b3953ca6cdc5ea1983d42df33b0`
- Proposed body SHA256: `ccd2134ff65fe646d9a6596f3f36a522ed10bfebdd6215885bb15f7920d300e9`

<!-- cua-release-backfill:v1:start evidence-sha256=f2e6352614bc32c1c9f92f3009951ada3bdd995cdb4370ea9c44ab395e3ec030 -->
## Summary

This release restores build scripts required to install and package Lume.

## Fixes

- Restored the build and install scripts used to package and install Lume. ([46fd3a0](https://github.com/trycua/cua/commit/46fd3a0c7ca5628bc02bef486de913fca7ebfa7e), [#62](https://github.com/trycua/cua/pull/62))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.17...lume-v0.1.18](https://github.com/trycua/cua/compare/lume-v0.1.17...lume-v0.1.18)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.19

- Tag: `lume-v0.1.19`
- Release ID: `207953094`
- Original body SHA256: `62653a5e2b2fb0c9760355104f1bc5abe1f15f4bfaf4ff11348f6e470a4a6250`
- Proposed body SHA256: `0f43ef47ae88ca99d43a7c8fe5008429973f68992a683c6fed7394a377bf86c9`

<!-- cua-release-backfill:v1:start evidence-sha256=290fff48e8219a7d1ca56d76a8631e1842a8ff8f0ae2b337941e1aefa21082df -->
## Summary

This release adds a --no-cache option to the pull command.

## Features

- Added a --no-cache option to the pull command to force re-downloading of images. ([#67](https://github.com/trycua/cua/pull/67)) Thanks @aktech.

## Contributors

Thanks to @aktech for contributing to this release.

## Full changelog

[lume-v0.1.18...lume-v0.1.19](https://github.com/trycua/cua/compare/lume-v0.1.18...lume-v0.1.19)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.20

- Tag: `lume-v0.1.20`
- Release ID: `209199991`
- Original body SHA256: `37d15595c97e1fb314642551984fe1bdbfa410e2035958ca08c637cd856c7b47`
- Proposed body SHA256: `aa90d463eb38e309df92e2f5b3fe8faf66f20e8353a0340921dd8adcb84ff73b`

<!-- cua-release-backfill:v1:start evidence-sha256=67a04c77458f09bb39a4b26f55efd1c355618b104081d93e1938b68b71626866 -->
## Summary

This release optimizes disk image reassembly to reduce memory usage and storage requirements.

## Performance

- Reduced storage and memory requirements during disk image reassembly, allowing images of 60GB or larger to be reassembled on systems with limited RAM and storage. ([#72](https://github.com/trycua/cua/pull/72))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.19...lume-v0.1.20](https://github.com/trycua/cua/compare/lume-v0.1.19...lume-v0.1.20)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.31

- Tag: `lume-v0.1.31`
- Release ID: `211760725`
- Original body SHA256: `5a111f161cba60b90fd2209194b07d8c5850576ee6f033f4cf6f75fe2dad57db`
- Proposed body SHA256: `e85e25880e939550c9ec80040372b3e0d295c9c535331da4673e582569b70245`

<!-- cua-release-backfill:v1:start evidence-sha256=fb18a92d063ab5329b8f7fe7d5104ee01793fc98baf9bebca4bc57f67180dc4a -->
## Summary

This patch fixes a crash during VM image reassembly on Macs with limited memory.

## Fixes

- Fixed the VM image reassemble process being killed by the system on Macs with less than 16GB of RAM due to memory constraints. ([#99](https://github.com/trycua/cua/pull/99))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.30...lume-v0.1.31](https://github.com/trycua/cua/compare/lume-v0.1.30...lume-v0.1.31)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.32

- Tag: `lume-v0.1.32`
- Release ID: `212026710`
- Original body SHA256: `8bc30d36a4d0fc570e0387f155f72faf2c2f591becd64a5ab9f3f45d911509af`
- Proposed body SHA256: `101b417667351d17e75ee5e1487be8e1fa7490deeacad13cb92d4a1946fbc9c1`

<!-- cua-release-backfill:v1:start evidence-sha256=19d5d10882732776d7a95039f3151c5ce3e70d4317130b7a81f5534d26725f39 -->
## Summary

This release reduces disk usage and improves download speed when assembling VM images, with better progress reporting.

## Features

- Added real-time download speed statistics with an ETA and a download summary. ([#102](https://github.com/trycua/cua/pull/102))

## Performance

- Reduced disk space usage during VM image assembly by about 50% using a sparse file technique. ([#102](https://github.com/trycua/cua/pull/102))
- Improved VM image download speed through adaptive concurrency and network optimizations. ([#102](https://github.com/trycua/cua/pull/102))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.31...lume-v0.1.32](https://github.com/trycua/cua/compare/lume-v0.1.31...lume-v0.1.32)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.1.34

- Tag: `lume-v0.1.34`
- Release ID: `212118551`
- Original body SHA256: `c4e6689e4349390f8d3e64535b0ce2fe124a3df36eecbbd1c1a95791d1bcc53a`
- Proposed body SHA256: `989c1031ef23e3e7849f6ee72f5ef1a22f303e16960345ed77f10426bbab99c7`

<!-- cua-release-backfill:v1:start evidence-sha256=3c243b9dea6aa0276e6e09ff71e143c9f06ac94eca19e0d96b3585d63b1775e8 -->
## Summary

This release adds support for managing VMs across multiple storage locations with configurable cache settings.

## Features

- Added support for creating and managing VMs in multiple storage locations, configurable via new lume config location commands and a --location parameter on create, pull, and run. ([#91](https://github.com/trycua/cua/pull/91))
- Added configurable cache directory management via lume config cache get and lume config cache set commands, replacing the --no-cache parameter. ([#91](https://github.com/trycua/cua/pull/91))
- Moved configuration storage to follow the XDG Base Directory specification, with automatic migration from legacy settings. ([#91](https://github.com/trycua/cua/pull/91))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.33...lume-v0.1.34](https://github.com/trycua/cua/compare/lume-v0.1.33...lume-v0.1.34)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.0

- Tag: `lume-v0.2.0`
- Release ID: `214054000`
- Original body SHA256: `e67e18a5f952629661498930a36de9c567c6633558e8892b2ed2833da8fe4e99`
- Proposed body SHA256: `6838cc095a68d5a40743ce2cba9f8b1dc103acc438173c05dd43e78676b938e3`

<!-- cua-release-backfill:v1:start evidence-sha256=8fb6d235b1c614aa704684554d14a3b1921e24940d7be9389dbdfd3f93349b72 -->
## Summary

This release adds a push command for sharing VMs via the registry and introduces sparse file support to reduce disk usage while improving VM bootability.

## Features

- Added a push command to share VMs via the registry, including support for additional tags. ([#122](https://github.com/trycua/cua/pull/122))

## Fixes

- Fixed VM boot issues by properly preserving partition tables during image operations. ([#122](https://github.com/trycua/cua/pull/122))

## Performance

- Added sparse file support for VM disk images, significantly reducing actual disk usage. ([#122](https://github.com/trycua/cua/pull/122))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.1.38...lume-v0.2.0](https://github.com/trycua/cua/compare/lume-v0.1.38...lume-v0.2.0)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.1

- Tag: `lume-v0.2.1`
- Release ID: `214063840`
- Original body SHA256: `e29e4f39645b71f0df13d802662707fdd3acdc40af3576dda1e6c2bda843414c`
- Proposed body SHA256: `d33a330593aabed19d66091a008571f574cdf33c3f5ebc946a120fe5eca6faab`

<!-- cua-release-backfill:v1:start evidence-sha256=c73c5abfcff50aef63a777c2d4f6530bbac9a8237fed12a1313d79592e9f77da -->
## Summary

This patch improves handling of sparse VM disk images.

## Fixes

- Fixed handling of sparse disk images for VMs. ([5653c86](https://github.com/trycua/cua/commit/5653c86670cd3c1de2c9f57044ce3b41997a32fc))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.0...lume-v0.2.1](https://github.com/trycua/cua/compare/lume-v0.2.0...lume-v0.2.1)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.3

- Tag: `lume-v0.2.3`
- Release ID: `214075670`
- Original body SHA256: `93f3073b9ebf33b425be36194760f1e64418d134725cec7a3b87f4bf868df9ec`
- Proposed body SHA256: `d36f94753df356f41aa0847dd52f14656ef71c64664b0b9980414bae9fb6e995`

<!-- cua-release-backfill:v1:start evidence-sha256=6b3a9e7bbdc2f1195857fb1a107f05082cccc3cad7a5c3d32d109b11ec1483b3 -->
## Summary

This patch fixes VM image name normalization.

## Fixes

- Fixed normalization of VM image names. ([f5121a6](https://github.com/trycua/cua/commit/f5121a6f4df500d68dad73a9b150e0a4843ce11f))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.2...lume-v0.2.3](https://github.com/trycua/cua/compare/lume-v0.2.2...lume-v0.2.3)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.4

- Tag: `lume-v0.2.4`
- Release ID: `214655453`
- Original body SHA256: `08bb3a24287bda48122520bd58ead310a082ad900cda2127d5b4a0530aef0c03`
- Proposed body SHA256: `7bf6fa6ad6a6199418fe73b5aa0d92967147c1dc57c028a9c3b1ae3ab7b9ece3`

<!-- cua-release-backfill:v1:start evidence-sha256=9f1775f11d355248d0816ea1171396949ddfdb2ccb6b9214fbac3ec96fc646fd -->
## Summary

This release fixes VM cloning so that cloned and original VMs run independently instead of interfering with each other.

## Fixes

- Fixed cloned VMs incorrectly appearing as running when the original VM was started, by generating a new machine identifier for cloned images. ([#130](https://github.com/trycua/cua/pull/130)) Thanks @dp221125.
- Fixed an issue where cloning a VM while it was running caused problems. ([182e8c6](https://github.com/trycua/cua/commit/182e8c69ad75042b5ab8d48bdf665f8f9d71f94d))

## Contributors

Thanks to @dp221125 for contributing to this release.

## Full changelog

[lume-v0.2.3...lume-v0.2.4](https://github.com/trycua/cua/compare/lume-v0.2.3...lume-v0.2.4)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.5

- Tag: `lume-v0.2.5`
- Release ID: `214705602`
- Original body SHA256: `5377c14d5af55ecdfab65b8c2145b597604c9311800ebb445521fb2fdcaebc11`
- Proposed body SHA256: `b607156c3be6e7b1efa45307fb6997a6ac41bf77f1be08adac6131f02e05198e`

<!-- cua-release-backfill:v1:start evidence-sha256=74ea028c99ea237a6e29536439b9e1a2df08767ea3c0b34c3d14376f3c9ca9e2 -->
## Summary

This patch fixes authentication and permission scopes used when pushing and pulling VM images from the registry.

## Fixes

- Fixed registry push authorization to include all required scopes. ([003879c](https://github.com/trycua/cua/commit/003879c46eadc3a28b5447cd1c421fb8093b5a31))
- Fixed incorrect push and pull scopes that could cause registry authentication failures. ([35e24a9](https://github.com/trycua/cua/commit/35e24a9d637c588bb7a3d6452ae1b37c750c198f))
- Restored previous pull authentication behavior after a regression. ([b7c96f6](https://github.com/trycua/cua/commit/b7c96f63794dfb89097bcf29e1979322ff3878e3))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.4...lume-v0.2.5](https://github.com/trycua/cua/compare/lume-v0.2.4...lume-v0.2.5)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.6

- Tag: `lume-v0.2.6`
- Release ID: `215616303`
- Original body SHA256: `9c24d811057198818eaa01d9001c329b6974e81651150822d7ab81fe701ac776`
- Proposed body SHA256: `8b324199f0dcb17a8797764e2ad58eace25f68e0dc9573c624f1ddfb6520b051`

<!-- cua-release-backfill:v1:start evidence-sha256=4a653dc36fd546ac58e6f883cda5256bb7001b7497b003e5db336bdf8996212f -->
## Summary

This release introduces Lumier, a Docker-based interface for running macOS VMs in containers, and makes the install script sudo-free by default.

## Features

- Introduced Lumier, a Docker-based interface for running lume macOS VMs in containers with browser-based VNC access and configurable CPU, RAM, and display resolution. ([#144](https://github.com/trycua/cua/pull/144))

## Fixes

- Changed the default installation directory to a user-owned location, removing the need for sudo, with an optional install directory parameter for custom locations. ([#137](https://github.com/trycua/cua/pull/137))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.5...lume-v0.2.6](https://github.com/trycua/cua/compare/lume-v0.2.5...lume-v0.2.6)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.7

- Tag: `lume-v0.2.7`
- Release ID: `215620264`
- Original body SHA256: `a8c87f011fb37ecb05a1d6c26b01f846bbda81282415f934516f136ff8a68302`
- Proposed body SHA256: `9d533b8fce96a426029cae6486c2693709ad98d8480680ab6dd3d01f4e7b7b0d`

<!-- cua-release-backfill:v1:start evidence-sha256=dce4ea9bd801af6f233350fdbdfb6c7332d25fb98bf97ba4f2b94300fc9a75d7 -->
## Summary

This patch fixes an issue where running a VM could fail if its storage location was not found.

## Fixes

- Handle the case where a VM's storage location is not found when running lume run. ([9b78a40](https://github.com/trycua/cua/commit/9b78a40cb556ee67e159bb47a40b1af4d31fdf26))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.6...lume-v0.2.7](https://github.com/trycua/cua/compare/lume-v0.2.6...lume-v0.2.7)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.8

- Tag: `lume-v0.2.8`
- Release ID: `216086496`
- Original body SHA256: `faa266eaeca789759dad762023a431370fe437b1135d177c236c731db6c2d765`
- Proposed body SHA256: `2dc5736db77152f77c628ec77fe974d2e4ce172efcf7ca13cc1a198028e4e105`

<!-- cua-release-backfill:v1:start evidence-sha256=9e2185e1871b913684ae5d93f1304368d193192c636f28fa700b72b5bc94dbbf -->
## Summary

This release adds an option to start Lume without its background service and fixes a stop-handling bug.

## Features

- Add a --no-background-service option to the install script. ([a5ec926](https://github.com/trycua/cua/commit/a5ec926922f55c03303261e90aa4805ce4fc146d))

## Fixes

- Fix a bug in the stop handler. ([e9a9c03](https://github.com/trycua/cua/commit/e9a9c03b637916eb307af0e7e3c8a7113f723449))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.7...lume-v0.2.8](https://github.com/trycua/cua/compare/lume-v0.2.7...lume-v0.2.8)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.9

- Tag: `lume-v0.2.9`
- Release ID: `216610271`
- Original body SHA256: `78459d17ff817db6d71c6ab52c0869c2ff1c6bbf08f77dcf09952f60a61651d3`
- Proposed body SHA256: `00ce6be59b2709189ffb956b482cf93e49ac466b5207f0163079517f5be4414d`

<!-- cua-release-backfill:v1:start evidence-sha256=5f0fea7343e09ccd98691b55e1ea0d1bf942b53c3ab87dd419b63e082fd5c0c2 -->
## Summary

This release introduces Lumier, a Docker-first interface for Lume.

## Features

- Add Lumier, a Docker-first interface. ([591a456](https://github.com/trycua/cua/commit/591a4561e5712648cfede5409d91792475550a8b))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.8...lume-v0.2.9](https://github.com/trycua/cua/compare/lume-v0.2.8...lume-v0.2.9)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.10

- Tag: `lume-v0.2.10`
- Release ID: `216650281`
- Original body SHA256: `99c8d80b635380f67abcc7e2f510a11b30f1954f583b8e1a01a2c7880f325e1e`
- Proposed body SHA256: `9a664e99f6385f24c220f67ec588e439a07a5aed8aa8e92f587606502370544e`

<!-- cua-release-backfill:v1:start evidence-sha256=ebfab848133f36d521559d4d6fa43a73b12deab64ebd7f610ae7d967aa070c2b -->
## Summary

This patch fixes an issue with PATCH requests used to update VM settings.

## Fixes

- Fix handling of PATCH requests for setting values. ([f0016d3](https://github.com/trycua/cua/commit/f0016d3e24e6a7c451c04762e31ae499101f8ba4))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.9...lume-v0.2.10](https://github.com/trycua/cua/compare/lume-v0.2.9...lume-v0.2.10)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.11

- Tag: `lume-v0.2.11`
- Release ID: `216886959`
- Original body SHA256: `0c7c44db1f46d34fcffcd0788e668e0342417e0b75809689824c0374da8a9af5`
- Proposed body SHA256: `ceee7df93bd61ff738baa353992a11ad1bc35bb78b7d43a4c93747738040db17`

<!-- cua-release-backfill:v1:start evidence-sha256=8d4b42de4949b7d8c8673d61eadd595495e79ddbeb0fa91417fce18907288d6b -->
## Summary

This patch fixes an issue with deleting VMs.

## Fixes

- Fix lume delete. ([0ab712d](https://github.com/trycua/cua/commit/0ab712d5ef97354323cc011aa94189d7387ab7f7))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.10...lume-v0.2.11](https://github.com/trycua/cua/compare/lume-v0.2.10...lume-v0.2.11)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.12

- Tag: `lume-v0.2.12`
- Release ID: `217193230`
- Original body SHA256: `a76785f17f4987839c0c71cd4ea3aa69809a5bc1207103025f1fba8a1e1c3228`
- Proposed body SHA256: `0144c2c440d0696bbcaf724fe2898af387a69b6468e8e96c352e0451a27022d1`

<!-- cua-release-backfill:v1:start evidence-sha256=724b4f8fce6d31cb0de247eebe0c58208ab37374d8b18cf4c6e916bd5f074b83 -->
## Summary

This release improves the install script to include the correct port and prevent installing with sudo, and adds support for ephemeral storage.

## Features

- Add support for ephemeral storage. ([c88ef77](https://github.com/trycua/cua/commit/c88ef772cc3f319de0f93643e20424dc69e30d34))

## Fixes

- Include the correct port in the Lume install script. ([a1890ad](https://github.com/trycua/cua/commit/a1890ad9cc2214ebe559506367e6266b2bafad04))
- Prevent installing Lume when run with sudo. ([#161](https://github.com/trycua/cua/pull/161))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.11...lume-v0.2.12](https://github.com/trycua/cua/compare/lume-v0.2.11...lume-v0.2.12)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.13

- Tag: `lume-v0.2.13`
- Release ID: `217817955`
- Original body SHA256: `928c2cd7060f35082a3f838dc9711a274907fd1c6936ee43dc186c3c3cafa787`
- Proposed body SHA256: `9cd77c39f58cda352c527ee346517ae0a85be5480bdd411cb2fb85d8fa151e31`

<!-- cua-release-backfill:v1:start evidence-sha256=4787ffd87825aab881a93550fd99d9e0a25d39edfcd7249741d73c4b8990005e -->
## Summary

This release adds clipboard and audio device support to VMs.

## Features

- Add clipboard and audio device support. ([#170](https://github.com/trycua/cua/pull/170))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.12...lume-v0.2.13](https://github.com/trycua/cua/compare/lume-v0.2.12...lume-v0.2.13)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.14

- Tag: `lume-v0.2.14`
- Release ID: `218455316`
- Original body SHA256: `6cf307a3bb0d01a37e28067edd18b14ccc0be23a21147beca669ffc09c4928a1`
- Proposed body SHA256: `6f5ed01054013a0156e0a15eb89ee65d1bd55a8a316734f4527b04d13a9479df`

<!-- cua-release-backfill:v1:start evidence-sha256=a3ccaac167ce702a56b0a9c7c670e371a166f0e371ac79f13519e8a4bdf0cb67 -->
## Summary

This release adds an initial VM provider abstraction to the Computer interface, changes the Lume daemon's default port, and speeds up disk image caching.

## Features

- Add an initial VM provider to support multiple provider types including Lume, Lumier, and Cloud. ([#162](https://github.com/trycua/cua/pull/162))

## Fixes

- Change the Lume daemon service port from 3000 to 7777 to avoid conflicts with common frontend development ports. ([#162](https://github.com/trycua/cua/pull/162))

## Performance

- Cache the VM disk image as a fully reassembled file instead of in parts, enabling faster cache hits. ([#162](https://github.com/trycua/cua/pull/162))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.13...lume-v0.2.14](https://github.com/trycua/cua/compare/lume-v0.2.13...lume-v0.2.14)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.15

- Tag: `lume-v0.2.15`
- Release ID: `220529322`
- Original body SHA256: `dbc7ad5a3bfcb1d7daa29a0b9d7d0c3abe6636dcdf90b8557964e325e4b9e80d`
- Proposed body SHA256: `2e7c3088ffed5c3103f2d90eb5898a86eceb6a3e46e1785a8ccbfa6d2f8aad72`

<!-- cua-release-backfill:v1:start evidence-sha256=7e890b8b265a816c39565f7ec7476e0685f8e63ddddaf1d7ba4edd01080321e7 -->
## Summary

This patch fixes a bug when cloning a VM from a direct storage path.

## Fixes

- Fix cloning a VM from a direct path, such as a Lumier location, into a standard Lume storage location. ([#177](https://github.com/trycua/cua/pull/177)) Thanks @jklapacz.

## Contributors

Thanks to @jklapacz for contributing to this release.

## Full changelog

[lume-v0.2.14...lume-v0.2.15](https://github.com/trycua/cua/compare/lume-v0.2.14...lume-v0.2.15)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.23

- Tag: `lume-v0.2.23`
- Release ID: `272554111`
- Original body SHA256: `51ff5a945c864bf8282a9bd2f86f52f1c7fb8e123aa62381d456aa70e80112d7`
- Proposed body SHA256: `8130a6a6d94d788873e94883cd1af08655c0af5b7e6be1ca7ab0ca1049edd0b6`

<!-- cua-release-backfill:v1:start evidence-sha256=00cb84cd620eb6b415b057a98bf7c93a4d82602e3699682894e93935a55a112d -->
## Summary

This release fixes a server endpoint used for pushing VMs and corrects the install script so it downloads the correct Lume release instead of an unrelated component release.

## Fixes

- Update the server endpoint for pushing VMs from /vms/push to /lume/vms/push. ([#420](https://github.com/trycua/cua/pull/420)) Thanks @JagjeevanAK.
- Fix the install script so it downloads from the correct Lume release instead of resolving to an unrelated component's release. ([#655](https://github.com/trycua/cua/pull/655)) Thanks @synacktraa.

## Contributors

Thanks to @JagjeevanAK, @synacktraa for contributing to this release.

## Full changelog

[lume-v0.2.22...lume-v0.2.23](https://github.com/trycua/cua/compare/lume-v0.2.22...lume-v0.2.23)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.27

- Tag: `lume-v0.2.27`
- Release ID: `275846336`
- Original body SHA256: `242c536b6266ddc83acd7135a7dcc854fb153d5e9790f95cfdb81d80b1be2637`
- Proposed body SHA256: `156a3562894daf1c1c1cba367d1aabde99f32897841069620f37d6dcb84afd8c`

<!-- cua-release-backfill:v1:start evidence-sha256=ea70d26689f3e633b2978c4438e76764062bf6b3a8bb02fb4765317f70c39a7e -->
## Summary

This release fixes macOS Login Items behavior for the Lume daemon, removing an unidentified developer warning and reducing duplicate notifications.

## Fixes

- Run the signed lume binary directly in the LaunchAgent instead of a wrapper script, fixing an unidentified developer warning and correcting the Login Item display name. ([#772](https://github.com/trycua/cua/pull/772))
- Consolidate the daemon and updater into a single Login Item to reduce duplicate background activity notifications. ([#771](https://github.com/trycua/cua/pull/771))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.26...lume-v0.2.27](https://github.com/trycua/cua/compare/lume-v0.2.26...lume-v0.2.27)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.28

- Tag: `lume-v0.2.28`
- Release ID: `277634064`
- Original body SHA256: `ad2969dd552e98787a63d0c5e8221c07e6fdf4b870b7b17a941e5731e51df94b`
- Proposed body SHA256: `76095a49709b1bf47defc7834728e2fb203cd657f37f3f4c7907d37fb7abb710`

<!-- cua-release-backfill:v1:start evidence-sha256=6e30d2cd1ad7717e17f0292bdba98444d774e6749e995aa81cac03645290d419 -->
## Summary

This release adds opt-out telemetry, CLI usability improvements, and a rewritten documentation site.

## Features

- Add opt-out telemetry for CLI commands and HTTP API endpoints, configurable via a new lume config telemetry command or an environment variable. ([#820](https://github.com/trycua/cua/pull/820))
- Display an ASCII art banner when running lume or lume --help. ([#820](https://github.com/trycua/cua/pull/820))
- Add an uninstall script with purge and force options. ([#820](https://github.com/trycua/cua/pull/820))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.27...lume-v0.2.28](https://github.com/trycua/cua/compare/lume-v0.2.27...lume-v0.2.28)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.29

- Tag: `lume-v0.2.29`
- Release ID: `277635833`
- Original body SHA256: `7b39527cb032f37b6f6b87aafa07f9c2096b59861be58092e948c6f7adda49c0`
- Proposed body SHA256: `698bdd5677f2519218caae116014dec09a9b29bff2d5702b0583f0262cddf023`

<!-- cua-release-backfill:v1:start evidence-sha256=7a8778d5fc2a088f97cfee7b298910326590d94e77ce89d3de72714de8faea35 -->
## Summary

This release fixes a fatal error when creating VMs with the unattended flag.

## Fixes

- Include the required resource bundle in installation, fixing a fatal error when using the unattended setup flag. ([#821](https://github.com/trycua/cua/pull/821))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.28...lume-v0.2.29](https://github.com/trycua/cua/compare/lume-v0.2.28...lume-v0.2.29)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.30

- Tag: `lume-v0.2.30`
- Release ID: `277641460`
- Original body SHA256: `0b0562f0412e03c075281e23cdb8a93c2f6f924bb51c8d6186de269edab3748c`
- Proposed body SHA256: `f4df847dc59af6abc2efcf44fdad298056469440bba08faabe8b3b9e38706e59`

<!-- cua-release-backfill:v1:start evidence-sha256=59d40973a5f0cdab507eb046735ff090a59ae879c11ded26324e2c0a13657bfd -->
## Summary

This release improves the reliability of unattended macOS VM setup.

## Fixes

- Improve reliability of the unattended Setup Assistant automation by using a more direct field interaction and longer delays. ([#824](https://github.com/trycua/cua/pull/824))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.29...lume-v0.2.30](https://github.com/trycua/cua/compare/lume-v0.2.29...lume-v0.2.30)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.31

- Tag: `lume-v0.2.31`
- Release ID: `277641895`
- Original body SHA256: `5a8cd6c8d85d1099f2fdd769c01a152d6caebc2b840f155b486d44ebd89e879e`
- Proposed body SHA256: `d5d55af88d01aaaba0c720cbc2410eb6472815b9e38dcc9ac504b7f6e3add2a5`

<!-- cua-release-backfill:v1:start evidence-sha256=59ae444184676b490c6f7d69ab749cd3d3257236e48dea644e55faec28b2d704 -->
## Summary

This release fixes an unattended setup failure in System Settings navigation.

## Fixes

- Fix an unattended setup failure where System Settings search could not be reached due to lost keyboard focus. ([#825](https://github.com/trycua/cua/pull/825))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.30...lume-v0.2.31](https://github.com/trycua/cua/compare/lume-v0.2.30...lume-v0.2.31)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.32

- Tag: `lume-v0.2.32`
- Release ID: `277674593`
- Original body SHA256: `bb6405ad5e148319a2c1bf46b481e2c1959a5c10b2a685b1b1447582e82bbe65`
- Proposed body SHA256: `987683392e2978b4a9d9060905988140ae1f58e494d928a1593a1d3722c9a9fc`

<!-- cua-release-backfill:v1:start evidence-sha256=27cb1db9e237cacaedc5b8669e1532ed5a4d5ffb1c56bfe318dff5fc4c4b37cd -->
## Summary

This release fixes VM status detection across processes and improves installation documentation.

## Fixes

- Fix lume ls incorrectly showing VMs as stopped when run in a different process than lume run. ([#834](https://github.com/trycua/cua/pull/834))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.31...lume-v0.2.32](https://github.com/trycua/cua/compare/lume-v0.2.31...lume-v0.2.32)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.33

- Tag: `lume-v0.2.33`
- Release ID: `277675142`
- Original body SHA256: `e65c1dfd40826b5bdb2ad006fe75971df82ce79048daa727636dc45d152dd437`
- Proposed body SHA256: `1d9f1709dcb0ad0185229059b1c4274e740a0c8edd2998eff892c84bebad77fb`

<!-- cua-release-backfill:v1:start evidence-sha256=8dee1a8c8f1511091693a478fa5109e5a8f746fb8e4afabdf0b069989f7cd7bb -->
## Summary

This release fixes incorrect SSH availability detection in lume ls.

## Fixes

- Fix lume ls incorrectly reporting SSH as unavailable due to unreliable connection timeout detection. ([#835](https://github.com/trycua/cua/pull/835))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.32...lume-v0.2.33](https://github.com/trycua/cua/compare/lume-v0.2.32...lume-v0.2.33)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.34

- Tag: `lume-v0.2.34`
- Release ID: `277686975`
- Original body SHA256: `eb1b2ff63f74d3194ba0615cd83cfce27e6ca406b97517f91e0f4244804efdab`
- Proposed body SHA256: `7bf3cb2330deff74b2b91cdf78fd10709198ed1bfd0566f5d7966e677fd87780`

<!-- cua-release-backfill:v1:start evidence-sha256=a8429fb9dc0151e9078dd5a1b09d26fd5bdd21df06b3c2da4e22ac5c444f9659 -->
## Summary

This release adds a Model Context Protocol server for AI agent integration and makes VM creation asynchronous with provisioning status tracking.

## Features

- Add a native MCP server via lume serve --mcp with tools, resources, and prompts for AI agent VM management. ([#836](https://github.com/trycua/cua/pull/836))
- VM creation now returns immediately while provisioning runs in the background, with a new provisioning status visible in CLI, HTTP API, and MCP. ([#836](https://github.com/trycua/cua/pull/836))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.33...lume-v0.2.34](https://github.com/trycua/cua/compare/lume-v0.2.33...lume-v0.2.34)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.36

- Tag: `lume-v0.2.36`
- Release ID: `277688413`
- Original body SHA256: `269d43d60298fba6cae9ac1e9fd2b6ff6962733395e8b6b923cbd65c72170228`
- Proposed body SHA256: `b65aa192185b69a00a7e8ad509d1b24b23f1fb28df07db0a63e60dac5c300805`

<!-- cua-release-backfill:v1:start evidence-sha256=dbe605ba869be7acb2324c54135bcde812bdf8a619c23cf7a74c9fb8877ba0b7 -->
## Summary

This release adds MCP resources and prompts to help AI agents follow Lume workflows and best practices.

## Features

- Add MCP resources providing a usage guide and default credentials, plus guided prompts for creating, running, and resetting sandboxes. ([#838](https://github.com/trycua/cua/pull/838))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.35...lume-v0.2.36](https://github.com/trycua/cua/compare/lume-v0.2.35...lume-v0.2.36)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.37

- Tag: `lume-v0.2.37`
- Release ID: `277690084`
- Original body SHA256: `bb1d41a09e513203d3debbecd739bdd45bc2f2f4a8a4f544619e6fd06085d9f4`
- Proposed body SHA256: `8367a39ac4ed09b5542c782ae43ddf22cca88381b23271c3d9d299889da24d84`

<!-- cua-release-backfill:v1:start evidence-sha256=02cddd4bad5bd7abe8d64c33293263b26a768aa8e23fb3305962013fe38abf49 -->
## Summary

This patch ensures lume ls and other CLI commands reflect a VM's provisioning state accurately and prevents unsafe operations on VMs that are still being created.

## Fixes

- Show newly created VMs with a provisioning status in lume ls immediately after lume create starts, instead of only after creation finishes. ([#839](https://github.com/trycua/cua/pull/839))
- Prevent running, stopping, or cloning a VM while it is still being provisioned, returning a clear error instead. ([#839](https://github.com/trycua/cua/pull/839))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.36...lume-v0.2.37](https://github.com/trycua/cua/compare/lume-v0.2.36...lume-v0.2.37)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.39

- Tag: `lume-v0.2.39`
- Release ID: `277731965`
- Original body SHA256: `50767f9d6b2804504cef45693103b8d2367b21ee521f31dce2016faeb77e5ca1`
- Proposed body SHA256: `8dd6c127ab5ee328bc3e7cda3532873d224ce2bb6943a43d5cddcb09c628e23e`

<!-- cua-release-backfill:v1:start evidence-sha256=40404032df0d4be0b55de7795f3ec858f55e966084a44db0e59fb2c37a75cdbe -->
## Summary

This patch fixes lume ls hanging when a previous lume process crashed and left behind stale session data.

## Fixes

- Detect and clean up stale session files left by crashed or killed lume processes so lume ls no longer hangs and correctly reports the VM as stopped. ([#846](https://github.com/trycua/cua/pull/846))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.38...lume-v0.2.39](https://github.com/trycua/cua/compare/lume-v0.2.38...lume-v0.2.39)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.40

- Tag: `lume-v0.2.40`
- Release ID: `277733864`
- Original body SHA256: `81358ca1c901944123232faa81b1d5d25b7cb398bb50d613c4e3717a04974a03`
- Proposed body SHA256: `94db809d2e71dc2ee023f54162486949e8f802e5af5295156315b1d899fb9d74`

<!-- cua-release-backfill:v1:start evidence-sha256=b2080fc545ac29cc58c8ec0093e94df7d0268c116ddb05b1776f021a28edbaf1 -->
## Summary

This patch refines how VM provisioning status is tracked and renames the default storage location.

## Features

- Rename the default storage location to "home" for new installations to better reflect the ~/.lume directory. ([#847](https://github.com/trycua/cua/pull/847))

## Fixes

- Show VMs as running during unattended setup instead of provisioning, and automatically clean up leftover provisioning markers on completed VMs while warning about VMs stuck provisioning for more than 8 hours. ([#847](https://github.com/trycua/cua/pull/847))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.39...lume-v0.2.40](https://github.com/trycua/cua/compare/lume-v0.2.39...lume-v0.2.40)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.41

- Tag: `lume-v0.2.41`
- Release ID: `277836020`
- Original body SHA256: `7b696472b4c7c1cd687b05fbc4a7331a59fc6ac608774e6c0706ac28e8a688f7`
- Proposed body SHA256: `0db185d29806208d2d8e370f1a5c0914a7a509e6a28a6303577dadcab794b776`

<!-- cua-release-backfill:v1:start evidence-sha256=a2afa28071d9f6b9e9dcdcabe542a71d5024afafd0c5ab086ac8515fe9d8d65e -->
## Summary

This patch fixes lume get to accurately report provisioning status and simplifies MCP server setup for Claude Desktop.

## Fixes

- Fix lume get to show accurate provisioning status for VMs that are still being created, matching the behavior of lume ls. ([#853](https://github.com/trycua/cua/pull/853))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.40...lume-v0.2.41](https://github.com/trycua/cua/compare/lume-v0.2.40...lume-v0.2.41)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.42

- Tag: `lume-v0.2.42`
- Release ID: `277971151`
- Original body SHA256: `d74974c9fa2cafc3e7603922fd141d90dbeefdf16e29f1c21bd715a2232ac478`
- Proposed body SHA256: `ee6fa0cb6b21e85cc70cdb10b90d0e4074fcdfbd3bcbdc41248a5084010e2be9`

<!-- cua-release-backfill:v1:start evidence-sha256=db1fea3e0075d89545ec23077742956a39e9bc90e3b2b54fca52b12be92c0912 -->
## Summary

This patch fixes the MCP server's run VM command, which previously blocked indefinitely and caused MCP clients to time out.

## Fixes

- Fix the MCP server's run VM command to return immediately after starting the VM instead of blocking forever, preventing MCP client timeouts. ([#854](https://github.com/trycua/cua/pull/854))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.41...lume-v0.2.42](https://github.com/trycua/cua/compare/lume-v0.2.41...lume-v0.2.42)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.44

- Tag: `lume-v0.2.44`
- Release ID: `277977496`
- Original body SHA256: `3a2b75460d4a0802cb908bdd9d64ce160f4e57b33db0d554b50165b49a2230df`
- Proposed body SHA256: `251e6e026403c94e58d90016e2d3dba507a2f2b78f1b7cc260f0ffef382817d5`

<!-- cua-release-backfill:v1:start evidence-sha256=9482fe75dbdcea3d003ef490d35e1cf1be512515d69de2ed7ec111aae742449d -->
## Summary

This patch fixes the MCP server's get VM command, which incorrectly reported provisioning VMs as stopped.

## Fixes

- Fix the MCP server's get VM command to correctly show provisioning status instead of incorrectly reporting stopped for VMs still being created. ([#855](https://github.com/trycua/cua/pull/855))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.43...lume-v0.2.44](https://github.com/trycua/cua/compare/lume-v0.2.43...lume-v0.2.44)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.45

- Tag: `lume-v0.2.45`
- Release ID: `277981206`
- Original body SHA256: `24325353e294b174a6c1fadce6cf0bc45ae0d9c67ee3ba11de014ac549374cc1`
- Proposed body SHA256: `274011c1681288e6063c6e12faf6b1ed1dc601cda30896d37fff08a5d907d561`

<!-- cua-release-backfill:v1:start evidence-sha256=dc043aedaef39998841cfa5ade53135c624347e9931daf64e491356a2f7441fe -->
## Summary

This patch fixes several issues where the MCP and HTTP API servers reported incorrect VM status or blocked on VM creation and startup.

## Fixes

- Fix the MCP server's run VM command to return immediately in the background instead of blocking forever. ([#856](https://github.com/trycua/cua/pull/856))
- Fix the MCP server's get VM command to correctly report provisioning status. ([#856](https://github.com/trycua/cua/pull/856))
- Fix the HTTP API's get VM endpoint to correctly report provisioning status. ([#856](https://github.com/trycua/cua/pull/856))
- Fix a race condition where a newly created VM could briefly show as stopped immediately after creation instead of provisioning. ([#856](https://github.com/trycua/cua/pull/856))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.44...lume-v0.2.45](https://github.com/trycua/cua/compare/lume-v0.2.44...lume-v0.2.45)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.49

- Tag: `lume-v0.2.49`
- Release ID: `278507630`
- Original body SHA256: `fb3805e8cca41a62fa25b03aab77e49c27316178a5952113bbe7265afc8cde4f`
- Proposed body SHA256: `975565228ebdb12a40ea1059b3b334e46b44bfcfceaecd7bd33d69c74d1ecb5d`

<!-- cua-release-backfill:v1:start evidence-sha256=6d98a8bf8d53a61874cb8dcdab296e739d2b6f18e52bfc9e45ee5b0085f8e5ca -->
## Summary

This patch adds a script for building and installing lume from local source code for local development and debugging.

## Features

- Add an install-local.sh script that builds lume from local source, codesigns it, installs it, and sets up the background service without downloading from GitHub releases. ([#874](https://github.com/trycua/cua/pull/874))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.48...lume-v0.2.49](https://github.com/trycua/cua/compare/lume-v0.2.48...lume-v0.2.49)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.54

- Tag: `lume-v0.2.54`
- Release ID: `281866055`
- Original body SHA256: `48f05a92007c38e74ff46d2daffcd4d4451e2b607f3a3a9c3c7e0b4faa77be81`
- Proposed body SHA256: `c9c9ea621f3d84626dfd8938ea881e72dcc103f647c8cd20a77b8fbc2208a694`

<!-- cua-release-backfill:v1:start evidence-sha256=e577b4f4ebee5d029e47ca0b64f099acbaadb6abc21a568503f7856738d16a52 -->
## Summary

This patch fixes a build failure in the previous release by updating the CI toolchain used to compile lume.

## Fixes

- Update the CI build toolchain to resolve a build failure caused by stricter concurrency checks in the prior Swift compiler version. ([#948](https://github.com/trycua/cua/pull/948))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.53...lume-v0.2.54](https://github.com/trycua/cua/compare/lume-v0.2.53...lume-v0.2.54)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.55

- Tag: `lume-v0.2.55`
- Release ID: `281878821`
- Original body SHA256: `9bc543c852b2291deedba4f9ac7c75739da6e562f6901a43ca77f4345a2b6b62`
- Proposed body SHA256: `152896f06e099a410f988411ebc4361fb733e86a4e1cc572896edf4bbcfb666d`

<!-- cua-release-backfill:v1:start evidence-sha256=a295d55cb3d921fb273568e68e0543e22eaf502515edcf3734509ed2f2fc7430 -->
## Summary

This patch adds automatic bidirectional clipboard syncing between the macOS host and running Lume VMs.

## Features

- Automatically sync clipboard content in both directions between the host Mac and a running VM over SSH, allowing copy and paste to work seamlessly between them. ([#951](https://github.com/trycua/cua/pull/951))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.54...lume-v0.2.55](https://github.com/trycua/cua/compare/lume-v0.2.54...lume-v0.2.55)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.56

- Tag: `lume-v0.2.56`
- Release ID: `281879123`
- Original body SHA256: `122dbc1aa79ec999d22549d999658ac3c9470a7a684516056456d0f4d0b9ab63`
- Proposed body SHA256: `173ba8ca4c1419976413a9ddb782b4292f243fe5b494076b452914e8ea1868db`

<!-- cua-release-backfill:v1:start evidence-sha256=39addb64ca55bbf56b1b7fb993c7766fe12954511120833d01d7c7ca2682e8cd -->
## Summary

This patch fixes VM disk sizing so that sizes given without a unit are treated as gigabytes instead of megabytes, preventing accidentally undersized disks.

## Fixes

- Disk size values without an explicit unit now default to gigabytes instead of megabytes, and a minimum size check rejects disks that are too small to use (30GB for macOS, 10GB for Linux). ([#952](https://github.com/trycua/cua/pull/952)) Thanks @thomasquinlan.

## Contributors

Thanks to @thomasquinlan for contributing to this release.

## Full changelog

[lume-v0.2.55...lume-v0.2.56](https://github.com/trycua/cua/compare/lume-v0.2.55...lume-v0.2.56)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.57

- Tag: `lume-v0.2.57`
- Release ID: `281880119`
- Original body SHA256: `c3a25a7d84a852d067a4a626be600bb7c8a6266fa8c0478cd16dbe0cc099ee71`
- Proposed body SHA256: `cbc9d92cfdf0e4dc7130dd3261ded5d4578ee60889ac2821e5eaf197644e76b7`

<!-- cua-release-backfill:v1:start evidence-sha256=02611260f7c8c6ea9c3d757c08c431e0f43031c7e10f0c40bec6664adf163572 -->
## Summary

This patch fixes clipboard synchronization for large amounts of copied content.

## Fixes

- Clipboard sync now uses a heredoc instead of a command line argument for content larger than 64KB, avoiding shell argument length limits, and the transfer timeout for large content was increased from 5 to 10 seconds. ([#953](https://github.com/trycua/cua/pull/953))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.56...lume-v0.2.57](https://github.com/trycua/cua/compare/lume-v0.2.56...lume-v0.2.57)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.58

- Tag: `lume-v0.2.58`
- Release ID: `281881527`
- Original body SHA256: `73ee33302385f738f3950ffa26399ab45dc7a116f878bbe50ae256894ccc23cb`
- Proposed body SHA256: `bf79cb7fbb61672a5ea4a21a699771ed4d82d6879371e9f48b02f2c2d05ba6ca`

<!-- cua-release-backfill:v1:start evidence-sha256=b544f456b3535c931d56745b15be8dee27c8d02c14c220394cba3754087d16e4 -->
## Summary

This patch fixes a clipboard synchronization race condition between the host and the VM.

## Fixes

- Fixed a race condition where clipboard content copied on the host could be overwritten by stale VM clipboard content immediately after syncing. ([#955](https://github.com/trycua/cua/pull/955))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.57...lume-v0.2.58](https://github.com/trycua/cua/compare/lume-v0.2.57...lume-v0.2.58)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.59

- Tag: `lume-v0.2.59`
- Release ID: `281990957`
- Original body SHA256: `d790d303d8de6e556f7883d610c25ab47942bf08707dff2a3b1f184e90458c57`
- Proposed body SHA256: `bc46d2722ae71fa123c99d445bfd77e3b534d32d63f8fc101d351144b8685d45`

<!-- cua-release-backfill:v1:start evidence-sha256=3c40f0b945711d05de4ce7ee0947e93fab28d1cd58d354944a065165f005cad2 -->
## Summary

This patch fixes unattended macOS setup presets that had not been receiving previous fixes due to being bundled from the wrong location.

## Fixes

- Synced the bundled unattended setup presets with prior System Settings navigation fixes that had been applied to an unused copy of the preset files, and removed the duplicate unused files. ([#958](https://github.com/trycua/cua/pull/958))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.58...lume-v0.2.59](https://github.com/trycua/cua/compare/lume-v0.2.58...lume-v0.2.59)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.60

- Tag: `lume-v0.2.60`
- Release ID: `281998739`
- Original body SHA256: `e42e0b78dabad432e65c1924ecab83243266a98c21f8922d1be1cd73c53655cd`
- Proposed body SHA256: `f247b38ef9d95a5aaca979e4db456d304e1a9b3a9a5c4f0b9e66b3ecb8e4e741`

<!-- cua-release-backfill:v1:start evidence-sha256=acffca1fbd9c1f2128872b2d247c535ffbf109e75fee2ef7b428a49aff68f08f -->
## Summary

This patch simplifies Siri setup during unattended macOS installation.

## Fixes

- Unattended setup now clicks Enable Ask Siri directly, skipping the Select a Siri Voice and Improve Siri and Dictation screens, making setup faster and more reliable. ([#959](https://github.com/trycua/cua/pull/959))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.59...lume-v0.2.60](https://github.com/trycua/cua/compare/lume-v0.2.59...lume-v0.2.60)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.61

- Tag: `lume-v0.2.61`
- Release ID: `282005388`
- Original body SHA256: `e6ed75b670a53cfbebc33d44e53df943679a964d4e74cd12b8f5f78bcc8b42bb`
- Proposed body SHA256: `9661bca9408c45458e8460dc557e6a9651d10c0b57f65d4bd4747d35593d93f4`

<!-- cua-release-backfill:v1:start evidence-sha256=aca8f269e7deaa5d0f4a464bdd1fbcfec374c0f731abf3bd0da500e7ee3fe1e2 -->
## Summary

This patch fixes language selection during unattended macOS setup on European hosts.

## Fixes

- Unattended setup now clicks directly on the English option instead of pressing enter, preventing English (UK) from being selected instead of English on European hosts. ([#961](https://github.com/trycua/cua/pull/961))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.60...lume-v0.2.61](https://github.com/trycua/cua/compare/lume-v0.2.60...lume-v0.2.61)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.62

- Tag: `lume-v0.2.62`
- Release ID: `282007209`
- Original body SHA256: `b4c9a5dbad7056502b1a353a964758102b4cbebcb2219ff5d457a45ef0c4ea05`
- Proposed body SHA256: `677831e2538ef2d01413e00e9bd9c742a9b91f8b41daf23f86f97b8a98f57f5a`

<!-- cua-release-backfill:v1:start evidence-sha256=987a8070ca16e0019b01f8434b921589276867f083f7550d19e3a276f85141eb -->
## Summary

This patch fixes an unattended setup issue where language selection did not proceed to the next screen.

## Fixes

- Unattended setup now presses enter after clicking English to confirm the selection and proceed, after the previous fix left the selection unconfirmed. ([#963](https://github.com/trycua/cua/pull/963))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.61...lume-v0.2.62](https://github.com/trycua/cua/compare/lume-v0.2.61...lume-v0.2.62)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.63

- Tag: `lume-v0.2.63`
- Release ID: `282010046`
- Original body SHA256: `17c3b971e925d77c0b9493b2158508c8aeb7cf9568b0285fbc1b79c1bf4beebf`
- Proposed body SHA256: `227e08b0cd261d30602133bd7612071da5d1d841c77aa4efebe67cbfd3fb911e`

<!-- cua-release-backfill:v1:start evidence-sha256=6f2897e6f16b558ca5f4e6db04c60bddbe4f532f4501093f6e8b3bebe6df984c -->
## Summary

This patch fixes unreliable password entry during unattended account creation.

## Fixes

- Added one-second delays between keyboard input steps during unattended account creation, fixing an issue where rapid input caused the password field to receive incomplete characters. ([#965](https://github.com/trycua/cua/pull/965))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.62...lume-v0.2.63](https://github.com/trycua/cua/compare/lume-v0.2.62...lume-v0.2.63)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.65

- Tag: `lume-v0.2.65`
- Release ID: `282012344`
- Original body SHA256: `1632beb161ac6b99858e16c8976800a52bf83adb7690dfc61895f07e9662a135`
- Proposed body SHA256: `05776679ef47eb00e08b35e0e6949f6bcca0012da0a882032b411957e1d47162`

<!-- cua-release-backfill:v1:start evidence-sha256=29c3f35ab1c87546a8be26f095268afe16855272e133d62a8803954520b2b999 -->
## Summary

This patch fixes an unattended setup issue where Remote Login settings could not be reliably located.

## Fixes

- Unattended setup now searches for Remote Logi instead of Remote Login in System Settings to avoid ambiguous search result matches. ([#966](https://github.com/trycua/cua/pull/966))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.64...lume-v0.2.65](https://github.com/trycua/cua/compare/lume-v0.2.64...lume-v0.2.65)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.66

- Tag: `lume-v0.2.66`
- Release ID: `282034760`
- Original body SHA256: `1a62997c5822e5a31e5f4c493bae74bccdf0ff561f1a78fc130ae6ebba773a48`
- Proposed body SHA256: `8496b4e2a60273457ac96521982857b8ad5d9012e68d47bd4fad4437e2a74e44`

<!-- cua-release-backfill:v1:start evidence-sha256=55b7d88d584d4ed131929d66d8a7924107932cfafd67f339ef54af4e8798ba7f -->
## Summary

This patch further improves the reliability of locating Remote Login settings during unattended setup.

## Fixes

- Unattended setup now searches for login instead of Remote Logi and waits up to 10 seconds after clicking for the settings pane to fully load. ([#967](https://github.com/trycua/cua/pull/967))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.65...lume-v0.2.66](https://github.com/trycua/cua/compare/lume-v0.2.65...lume-v0.2.66)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.67

- Tag: `lume-v0.2.67`
- Release ID: `282035837`
- Original body SHA256: `c74a2c2e76d6b9ab6474178b1c8ff08fcf47bce5cd145b3f8742e7285c5d2db8`
- Proposed body SHA256: `64e88d65644d5b87d443c31cbc26cea1c4dac9e65519597848d3ecf9bab38fd8`

<!-- cua-release-backfill:v1:start evidence-sha256=86e659cfe84ad77a2cedc983ed1d78972d87706d3de41e503c72332889ec205d -->
## Summary

This release removes automatic name conversion during image pulls, so images are pulled exactly as specified without an added suffix.

## Fixes

- Stop automatically converting image names to a suffixed variant during pull; images are now pulled exactly as specified. This is a breaking change for existing images that relied on the old naming convention. ([#964](https://github.com/trycua/cua/pull/964))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.66...lume-v0.2.67](https://github.com/trycua/cua/compare/lume-v0.2.66...lume-v0.2.67)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.68

- Tag: `lume-v0.2.68`
- Release ID: `282045478`
- Original body SHA256: `b10d796953001b21edb30ff23466376fc6857f657a00867bf7f1e9a70e7961dc`
- Proposed body SHA256: `345c932050d0e687324b4c8a79bf6bc0450c8bf48b48c2548b649c67e01933c4`

<!-- cua-release-backfill:v1:start evidence-sha256=eab4e1bdfde9807d23e34c5aeb01d94beb99e5f5b066b7d76d8bc5102acfb6c6 -->
## Summary

This release prevents VM screensaver activation and re-login prompts during unattended setup.

## Features

- Disable the screensaver, screen lock, display sleep, system sleep, and auto-logout in unattended VM setup presets to prevent re-login prompts that broke automated workflows. ([#968](https://github.com/trycua/cua/pull/968))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.67...lume-v0.2.68](https://github.com/trycua/cua/compare/lume-v0.2.67...lume-v0.2.68)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.69

- Tag: `lume-v0.2.69`
- Release ID: `282073507`
- Original body SHA256: `f32ae07c210d924296fb43a6d577510f0662d9364eee2f156aedcfb89e9d783c`
- Proposed body SHA256: `22caa8c7c81463aa1a513af03f327b1e8671d66cc09364cc2299913a3f2eb02d`

<!-- cua-release-backfill:v1:start evidence-sha256=bfd7c091f5c64b121449272a904e5dc5ed0c257e9084b00151952fa51fe95dbd -->
## Summary

This release adds a more reliable way to run post-setup commands in unattended VM configuration.

## Features

- Add a post_ssh_commands option to unattended configuration that runs commands over SSH after the health check passes, replacing less reliable VNC keyboard typing for setup steps like disabling the screensaver and sleep. ([#971](https://github.com/trycua/cua/pull/971))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.68...lume-v0.2.69](https://github.com/trycua/cua/compare/lume-v0.2.68...lume-v0.2.69)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.70

- Tag: `lume-v0.2.70`
- Release ID: `282371185`
- Original body SHA256: `65c6b45258b710f3a2a9f3e07a0f8792121abfa9e646b4f4e8961222a8605e1a`
- Proposed body SHA256: `5ab5ba6f020604b28bb6c1ef219f2f044d13a0a486365c839359c9165e5ff59d`

<!-- cua-release-backfill:v1:start evidence-sha256=1238c83ea649ccf507bfe04b6588fc8f291d2d1d93c30a063c5ca1101be7d79c -->
## Summary

This release improves the reliability of unattended VM setup by changing how remote login is enabled and slowing down automated keyboard input.

## Fixes

- Enable Remote Login through the System Settings UI toggle instead of typing Terminal commands, avoiding issues caused by fast keyboard input. ([#980](https://github.com/trycua/cua/pull/980))
- Slow down automated keyboard typing during unattended setup to prevent missed characters and typos. ([#981](https://github.com/trycua/cua/pull/981))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.69...lume-v0.2.70](https://github.com/trycua/cua/compare/lume-v0.2.69...lume-v0.2.70)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.71

- Tag: `lume-v0.2.71`
- Release ID: `282378877`
- Original body SHA256: `2577b57c924a5efd0c2b38a1dda09f1464fa15bc8512512f480aa0a8ca121af8`
- Proposed body SHA256: `5aee5fdfdc8b7ce4be2a30db9b177a38d2ebdf784654d3f54a41a82cd63ead96`

<!-- cua-release-backfill:v1:start evidence-sha256=a7a1847900ce256b1a12beb40005b1a4addddd762def7df16fa434f5871cfb3a -->
## Summary

This release lets unattended setup configurations control typing speed per command.

## Features

- Add an optional delay parameter to the type command in unattended configuration files, allowing per-command control over typing speed. ([#982](https://github.com/trycua/cua/pull/982))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.70...lume-v0.2.71](https://github.com/trycua/cua/compare/lume-v0.2.70...lume-v0.2.71)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.72

- Tag: `lume-v0.2.72`
- Release ID: `282385027`
- Original body SHA256: `d0de9cb792cdff9a918eff5056e138aee610b27dd2e81e9ec5853df1aedf9afe`
- Proposed body SHA256: `d76708fdd4f9bbc554ce99ba7574541a6f058779d584248e9ef14e594bb71db3`

<!-- cua-release-backfill:v1:start evidence-sha256=90fa8881553d66b78c99beb2022bca53919be737f57fc8094875ceafbd29c026 -->
## Summary

This release improves how unattended setup opens the Terminal application via Spotlight.

## Fixes

- Open Terminal during unattended setup by typing a partial name and clicking the Spotlight search result instead of typing the full name and pressing enter. ([#983](https://github.com/trycua/cua/pull/983))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.71...lume-v0.2.72](https://github.com/trycua/cua/compare/lume-v0.2.71...lume-v0.2.72)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.73

- Tag: `lume-v0.2.73`
- Release ID: `282407313`
- Original body SHA256: `7861e65e72ce4ff573271fa111853b14956b51d95357c881420fefa22ccfd403`
- Proposed body SHA256: `2da0b041308b2564934b9e9fb98ed605d00a397dcb6474f9e6ff562db921e455`

<!-- cua-release-backfill:v1:start evidence-sha256=49693d29cfb71b1111ee27c1f340e87c9176938eec19b28fc2462520d070fbb8 -->
## Summary

This release reverts the previous change to how Terminal is opened during unattended setup.

## Reverts

- Revert to opening Terminal during unattended setup by typing a partial name and pressing enter, instead of clicking the Spotlight search result. ([#984](https://github.com/trycua/cua/pull/984))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.72...lume-v0.2.73](https://github.com/trycua/cua/compare/lume-v0.2.72...lume-v0.2.73)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.74

- Tag: `lume-v0.2.74`
- Release ID: `282467092`
- Original body SHA256: `b31de4eaaa7300d19765d5a22fd4a89f37fdb7990d82002d2eed399c33b923ee`
- Proposed body SHA256: `c6a6c5bb20a90fd3f1aae2110990bdbf07c9d93ecdd929b0d71d8775d15c6337`

<!-- cua-release-backfill:v1:start evidence-sha256=4b8177f567dbbc450e091a183024d1f74d9b02346429f3f065ae37dfd98872f9 -->
## Summary

This release fixes repeated characters appearing when typing commands during unattended setup.

## Fixes

- Fix repeated key presses in Terminal during unattended setup by removing the key hold delay; the configured delay now only affects the pause between characters. ([#985](https://github.com/trycua/cua/pull/985))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.73...lume-v0.2.74](https://github.com/trycua/cua/compare/lume-v0.2.73...lume-v0.2.74)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.75

- Tag: `lume-v0.2.75`
- Release ID: `282760996`
- Original body SHA256: `a4076e013574f04c84028de8db359097e49f4ef63ec4c602c09f92b33254a9fb`
- Proposed body SHA256: `5fd9e74efedb8a44102f2dd0327105ff11fd56b034f0573e725f6c81b5fbfa3b`

<!-- cua-release-backfill:v1:start evidence-sha256=e2a997e6f0e6c029360a45dadcd9d0343b8ec78b68935ad80ccdf70710bfce90 -->
## Summary

This release improves the reliability of enabling Remote Login during unattended VM setup.

## Fixes

- Simplify and increase delays in the Remote Login toggle sequence during unattended setup, improving reliability on both supported macOS versions. ([#989](https://github.com/trycua/cua/pull/989))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.74...lume-v0.2.75](https://github.com/trycua/cua/compare/lume-v0.2.74...lume-v0.2.75)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.76

- Tag: `lume-v0.2.76`
- Release ID: `282774117`
- Original body SHA256: `ef6dbbfb4b2ebeaa6a8d3d4b5ea4fd362459522add6e23cf5c38301202c78ccc`
- Proposed body SHA256: `7e27caba7250ce8d4fc8e3c12445c2e7b01d38d5990d9e809a47283b44150754`

<!-- cua-release-backfill:v1:start evidence-sha256=6af9461e810042769cad14feb072bdcb654fc1825bb118524d76c9313c61005e -->
## Summary

This release fixes unattended setup health checks failing when sshpass is not installed.

## Fixes

- Replace the external sshpass dependency with a built-in SSH client for health checks and post-setup commands, fixing failures when sshpass was not installed on the host. ([#990](https://github.com/trycua/cua/pull/990))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.75...lume-v0.2.76](https://github.com/trycua/cua/compare/lume-v0.2.75...lume-v0.2.76)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.77

- Tag: `lume-v0.2.77`
- Release ID: `284659115`
- Original body SHA256: `fe14c65b9aa086eb726ebefb8d093494de8df47be51e7443decfc8523b50ae76`
- Proposed body SHA256: `c8757969df2438b6a399c606bd31445fb585292340b6bf9ba26c1ceec5dedaf2`

<!-- cua-release-backfill:v1:start evidence-sha256=0a12c80f608b074c5cacc750e1c0b25bfe51cb25177e1b1e4a66b17ca975d7e3 -->
## Summary

This release adds a VM provisioning script and improves the reliability of unattended macOS setup, including auto-login and sudo command handling.

## Features

- Added a script that provisions OpenClaw and its dependencies on a Lume macOS VM, with an unattended mode for fully automated installs. ([#994](https://github.com/trycua/cua/pull/994))

## Fixes

- Improved unattended macOS setup reliability by extending the Remote Login wait time, confirming the dialog more reliably, and ensuring auto-login persists after a VM restart. ([#991](https://github.com/trycua/cua/pull/991))
- Fixed sudo command handling during unattended setup and prevented the VM from showing a lock screen after being idle. ([#992](https://github.com/trycua/cua/pull/992))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.76...lume-v0.2.77](https://github.com/trycua/cua/compare/lume-v0.2.76...lume-v0.2.77)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.78

- Tag: `lume-v0.2.78`
- Release ID: `285100194`
- Original body SHA256: `0af47e44f496672728c7ca3c1ff7400bab9c099458046731bdc0771eb020c865`
- Proposed body SHA256: `b327aef714e8810a5ba954b9ae5463ab6e1119c6a62e604d39bad12f9048c251`

<!-- cua-release-backfill:v1:start evidence-sha256=fd9602532172e3280b7b89dbb611b69112d03e6a9a593ad4395652ecbb6054aa -->
## Summary

This release fixes networking configuration regressions and installer signing issues introduced by bridged networking support.

## Fixes

- Fixed VM run and creation commands so they respect a VM's persisted network mode instead of reverting to NAT, and the API now rejects invalid network values instead of silently falling back to NAT. ([#1063](https://github.com/trycua/cua/pull/1063)) Thanks @festen.
- Fixed local ad-hoc installer signing to avoid a networking entitlement that could cause macOS to kill the binary at launch. ([#1063](https://github.com/trycua/cua/pull/1063)) Thanks @festen.

## Contributors

Thanks to @festen for contributing to this release.

## Full changelog

[lume-v0.2.77...lume-v0.2.78](https://github.com/trycua/cua/compare/lume-v0.2.77...lume-v0.2.78)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.79

- Tag: `lume-v0.2.79`
- Release ID: `285107254`
- Original body SHA256: `08bd899d94856601a28aec95521a8473618de3a1266c537bec9b004d91e8fb22`
- Proposed body SHA256: `35b78a245a193349fba3554f9d8f230036b3e17b1b744f8ff732c1a739ee4d11`

<!-- cua-release-backfill:v1:start evidence-sha256=e36e16a29ff10ddb23234f2bf8f2c54823816e93e3b56bb29f094f01678b4f10 -->
## Summary

This release fixes a critical bug that caused the previous release to be unusable on macOS.

## Fixes

- Removed a restricted networking entitlement from release builds that was causing macOS to immediately kill the binary on launch for all commands, including basic ones like listing VMs. ([#1075](https://github.com/trycua/cua/pull/1075))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.78...lume-v0.2.79](https://github.com/trycua/cua/compare/lume-v0.2.78...lume-v0.2.79)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.80

- Tag: `lume-v0.2.80`
- Release ID: `285108746`
- Original body SHA256: `bf63fb8a75afa651b3796a7f49bc0fa63a28543d94ee1b8e20c03d5372b0e81c`
- Proposed body SHA256: `5bdf5aebf34c2daf6b0b49c2562b26042f0dd11bc26ee27e702222c88e45448c`

<!-- cua-release-backfill:v1:start evidence-sha256=ebea551dd8c9e67e0869894c6100e255796a44cd3a59110a74fb6756ab6117ee -->
## Summary

This release fixes noisy clipboard sync error logging when SSH connectivity to a VM is disrupted.

## Fixes

- Fixed clipboard sync so repeated SSH connection failures no longer flood the console with identical error messages, added a backoff on the poll interval, reused the SSH connection across polls, and logged when sync recovers. ([#1077](https://github.com/trycua/cua/pull/1077)) Thanks @gbertb.

## Contributors

Thanks to @gbertb for contributing to this release.

## Full changelog

[lume-v0.2.79...lume-v0.2.80](https://github.com/trycua/cua/compare/lume-v0.2.79...lume-v0.2.80)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.82

- Tag: `lume-v0.2.82`
- Release ID: `290861661`
- Original body SHA256: `1fdd7ea49736014e854371d0774e2c7409823ac77fbe5d94852b227afecf5912`
- Proposed body SHA256: `048c86e132623517d41d85458160743662e1697b35783f477026d23fd9518b30`

<!-- cua-release-backfill:v1:start evidence-sha256=40c900722971764f848a5b37e7aa31bf5909d6b41ad879f78577e055f40bde21 -->
## Summary

This release ships Lume as a signed macOS .app bundle with a provisioning profile to support bridged networking, re-landing the packaging change after an earlier revert.

## Features

- Re-landed the release packaging as a signed macOS .app bundle with a provisioning profile to support bridged networking after an earlier revert. ([#1122](https://github.com/trycua/cua/pull/1122))

## Reverts

- Reverted an earlier .app bundle packaging attempt while waiting for the required Apple entitlement. ([#1084](https://github.com/trycua/cua/pull/1084))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.81...lume-v0.2.82](https://github.com/trycua/cua/compare/lume-v0.2.81...lume-v0.2.82)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.84

- Tag: `lume-v0.2.84`
- Release ID: `291013808`
- Original body SHA256: `8b8b32a206e892caa2012636e0696251e4c9e15e83380cfa0e4e012d708a9c69`
- Proposed body SHA256: `f5dc9e138bcfb837424020555479c7311874f2aa48460d2c4ec9e8331ce210ea`

<!-- cua-release-backfill:v1:start evidence-sha256=c695d0b44db61a9b553fd9af711c3323a1c0aac195f21cda6eea6ca8adc2b04d -->
## Summary

This release fixes VM listing hangs, adds an SSH fallback for restricted environments, and corrects IP address resolution for bridged networking.

## Fixes

- Prevented the VM listing command from hanging indefinitely when subprocesses got stuck during concurrent VM state transitions. ([#1125](https://github.com/trycua/cua/pull/1125))
- Added a fallback to system SSH so remote sessions still work in sandboxed or restricted environments that block direct connections. ([#1125](https://github.com/trycua/cua/pull/1125))
- Fixed VM listing and info commands to show the correct IP address for VMs using bridged networking instead of a stale NAT address. ([#1125](https://github.com/trycua/cua/pull/1125))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.83...lume-v0.2.84](https://github.com/trycua/cua/compare/lume-v0.2.83...lume-v0.2.84)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.85

- Tag: `lume-v0.2.85`
- Release ID: `291638475`
- Original body SHA256: `3ac155080981c6f7d7d4c214c998c215530d53ed6cde9f1a67386ab79863409d`
- Proposed body SHA256: `e5e449f6113c82410688798720e88293cfdff9595a5940990d741cd5406a4eee`

<!-- cua-release-backfill:v1:start evidence-sha256=b6823b3a3442218e24fe4f81048cebe041f1e62ee5f23278f716fee4e9c78ebe -->
## Summary

This release fixes VM startup screen sharing, pause/resume behavior, and filesystem corruption in Linux VMs.

## Fixes

- Fixed Screen Sharing windows sometimes appearing black on VM startup by waiting for visible framebuffer content before opening the viewer. ([#1132](https://github.com/trycua/cua/pull/1132))
- Fixed a bug where pausing or resuming a VM would instead restart it. ([#1130](https://github.com/trycua/cua/pull/1130))
- Fixed filesystem corruption in Linux VMs by using cached disk mode for virtual disk attachments; macOS VMs are unaffected. ([#1100](https://github.com/trycua/cua/pull/1100)) Thanks @zpv.

## Contributors

Thanks to @zpv for contributing to this release.

## Full changelog

[lume-v0.2.84...lume-v0.2.85](https://github.com/trycua/cua/compare/lume-v0.2.84...lume-v0.2.85)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.2.86

- Tag: `lume-v0.2.86`
- Release ID: `297728745`
- Original body SHA256: `1b415f8a64fb184e01553e463c14c026a74b54fc2243f39f9f4228f5a81a0ed7`
- Proposed body SHA256: `c15d011bdf677a385bb4e1dffd3a5c227efce424845a7b48ea974ddd44c2cda9`

<!-- cua-release-backfill:v1:start evidence-sha256=3f189cf8ac16067109e919f26a0cb46e2279c28d15e1b6297a603542a2834b0a -->
## Summary

This release fixes inconsistent cleanup behavior when VM creation fails.

## Fixes

- Fixed cleanup after a failed VM creation so it behaves consistently across synchronous and asynchronous creation paths, and partially created VMs remain discoverable if cleanup itself fails. ([#1167](https://github.com/trycua/cua/pull/1167)) Thanks @AdityaSinghh7, @laogao.

## Contributors

Thanks to @AdityaSinghh7, @laogao for contributing to this release.

## Full changelog

[lume-v0.2.85...lume-v0.2.86](https://github.com/trycua/cua/compare/lume-v0.2.85...lume-v0.2.86)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.0

- Tag: `lume-v0.3.0`
- Release ID: `299804512`
- Original body SHA256: `cb52d85f3a52426912ad23e1fa9b3717376b3792a876ee795fb04e5f0cbff1b1`
- Proposed body SHA256: `01e6691f9d3fd7b4d12af776cce0d188329e835af05b9180dccdc7250721eeaa`

<!-- cua-release-backfill:v1:start evidence-sha256=7e15c7383e41372bec69c9b06f3ffda8cda2518ee679677a229b3ff5daf0c552 -->
## Summary

This release adds a cross-platform VNC automation backend for the computer server and overhauls the VM image push/pull format to a standard OCI-compliant structure.

## Features

- Added a VNC-based automation backend for the computer server that connects to any VNC server for screenshots, mouse, keyboard, and scroll operations, selectable via a backend flag or environment variables. ([#1196](https://github.com/trycua/cua/pull/1196))
- Overhauled the VM image push and pull format to produce standard OCI-compliant, multi-layer compressed manifests by default, while still accepting older image formats for backward compatibility. ([#1144](https://github.com/trycua/cua/pull/1144))

## Fixes

- Fixed a crash in the reassemble flag and a division-by-zero crash in the download progress bar, and changed the auto-update check from daily to weekly to reduce interruptions. ([#1144](https://github.com/trycua/cua/pull/1144))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.2.86...lume-v0.3.0](https://github.com/trycua/cua/compare/lume-v0.2.86...lume-v0.3.0)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.1

- Tag: `lume-v0.3.1`
- Release ID: `300435528`
- Original body SHA256: `9c7f0393ebc8f5282c5031c5bb12cd10818f802a86d60ce20d79abe6e4ae5c57`
- Proposed body SHA256: `21f52e1610d71f248770da4f3c7a3c5a6c33cd2340e3732ef7cdf90d37143e66`

<!-- cua-release-backfill:v1:start evidence-sha256=9efb586293c29ffd9beb0f01623bcad8e5e755c29a0a40a970cee3f1f0a73d0b -->
## Summary

This release rewrites the VNC automation backend for reliability and adds dynamic VNC configuration discovery for VMs.

## Features

- VMs now share dynamic VNC port and password configuration with the guest at boot instead of relying on hardcoded values. ([#1205](https://github.com/trycua/cua/pull/1205))

## Fixes

- Rewrote the VNC backend to avoid deadlocks and fixed drag operations and screen access issues in VNC-based automation. ([#1205](https://github.com/trycua/cua/pull/1205))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.0...lume-v0.3.1](https://github.com/trycua/cua/compare/lume-v0.3.0...lume-v0.3.1)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.2

- Tag: `lume-v0.3.2`
- Release ID: `300449977`
- Original body SHA256: `d6c457d97dad269912f579f826ec401706ba417c5c5d7059bf95ef7d00b854bc`
- Proposed body SHA256: `5dc999893e1c04aa5e94c5204566ce2a7f2ff8b53c65f5a1ae322b4dc0a65a8f`

<!-- cua-release-backfill:v1:start evidence-sha256=63c92eabe6344677e19db3f65a2c348372df880ff07a83cfd8e02d7d54af840d -->
## Summary

This release replaces the shared, hardcoded VNC password with per-VM auto-generated credentials delivered securely after boot, and fixes several VNC input bugs.

## Features

- Each VM now gets a randomly generated VNC port and passphrase instead of a shared hardcoded password. ([#1207](https://github.com/trycua/cua/pull/1207))

## Fixes

- VNC connection configuration is now delivered to the guest VM over SSH after boot, avoiding failures caused by macOS restrictions on reading shared files from background processes. ([#1207](https://github.com/trycua/cua/pull/1207))
- Fixed VNC coordinate corruption, incorrect scroll direction, and a connection leak in the VNC backend. ([#1207](https://github.com/trycua/cua/pull/1207))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.1...lume-v0.3.2](https://github.com/trycua/cua/compare/lume-v0.3.1...lume-v0.3.2)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.3

- Tag: `lume-v0.3.3`
- Release ID: `300492924`
- Original body SHA256: `66b20cb1df889475ea22e2aca2b3a8acc95c66e98149a013f2f73e6e73947fcf`
- Proposed body SHA256: `436c79660cde59b1775ab1258161a683583dc103a90dc7b3dae460b8faf1205c`

<!-- cua-release-backfill:v1:start evidence-sha256=eb9f369d3b01889783d7f164776d0362d7cd0d06127db901856cd6b1370c57fb -->
## Summary

This patch fixes an image pull failure that occurred when resuming an interrupted download.

## Fixes

- Fixed image pulls failing with a file collision error when retrying a download that was previously interrupted partway through. ([#1209](https://github.com/trycua/cua/pull/1209))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.2...lume-v0.3.3](https://github.com/trycua/cua/compare/lume-v0.3.2...lume-v0.3.3)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.4

- Tag: `lume-v0.3.4`
- Release ID: `300822746`
- Original body SHA256: `53b2b6376c8d968ff495adfd7101ebf456d5a4db9d3dc879226ab1ff97a44f14`
- Proposed body SHA256: `7c3b0776aad89f1e094b270810d43777d9a4bcd80ad052f298c2db9c3def6517`

<!-- cua-release-backfill:v1:start evidence-sha256=0abddd6a8a542b596b7fd54e1f3f941bd3600f03b39710704bb412f6ed03f1a6 -->
## Summary

This release standardizes the OCI image format used by Lume, which breaks compatibility with images published under the old format.

## Features

- Standardized the OCI image format, renaming the auxiliary image layer to a dedicated NVRAM layer; images previously published under the old format are no longer recognized and must be re-published. ([#1213](https://github.com/trycua/cua/pull/1213))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.3...lume-v0.3.4](https://github.com/trycua/cua/compare/lume-v0.3.3...lume-v0.3.4)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.7

- Tag: `lume-v0.3.7`
- Release ID: `302197803`
- Original body SHA256: `b5dff302151ea8c7b51ae3963494531e5cc1e3e668d97e5b1b2a20247fdc5505`
- Proposed body SHA256: `8b4cbae74b08faeb032d46b83435802b1d6be455dfee9820627008f632008430`

<!-- cua-release-backfill:v1:start evidence-sha256=6d9755f42dc15eb33c8326119c2baa6b5f81ec350a72e3984fe444824abec380 -->
## Summary

This patch fixes a build failure that was breaking the release process for Lume.

## Fixes

- Fixed a build error caused by a mismatched progress-reporting parameter in the image pull code path, which had been breaking the release build. ([#1232](https://github.com/trycua/cua/pull/1232))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.6...lume-v0.3.7](https://github.com/trycua/cua/compare/lume-v0.3.6...lume-v0.3.7)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.8

- Tag: `lume-v0.3.8`
- Release ID: `302220235`
- Original body SHA256: `4d91b900ddc98f9ed712f47a85b07a3fc83b3e3796032e739b8d859d63287a69`
- Proposed body SHA256: `420540070b8b52bcd453ab261e9f12c29a81945c837100ff8d29331c6916e709`

<!-- cua-release-backfill:v1:start evidence-sha256=a06e55b39461fc0d62f8c3f348075b7a2e4c6ba93e6a74830ba25272570d10ec -->
## Summary

This patch speeds up repeated image pulls by caching downloaded layers locally.

## Performance

- Image layers are now cached locally after download, so pulling the same image again skips the network download and reuses cached files. ([#1233](https://github.com/trycua/cua/pull/1233))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.7...lume-v0.3.8](https://github.com/trycua/cua/compare/lume-v0.3.7...lume-v0.3.8)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.9

- Tag: `lume-v0.3.9`
- Release ID: `302544321`
- Original body SHA256: `7982c190ccc0337f8b4858b9138f099f3c0c37151da1b4b9f24a853df644b9bf`
- Proposed body SHA256: `c47d6fd3b521a200302201376a9d50301cc94a4f7b5f4ff9fc1e7f2457290275`

<!-- cua-release-backfill:v1:start evidence-sha256=0b96cd00f1ac0d2f13d713a9527455ef724b0d6ac24a7def01fa8adbeb9373a5 -->
## Summary

This patch fixes intermittent request failures in Lume's HTTP server that occurred on fresh connections.

## Fixes

- Fixed requests such as image pull and VM clone intermittently failing with an invalid request body error when headers and body arrived in separate network packets. ([#1236](https://github.com/trycua/cua/pull/1236))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.8...lume-v0.3.9](https://github.com/trycua/cua/compare/lume-v0.3.8...lume-v0.3.9)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.10

- Tag: `lume-v0.3.10`
- Release ID: `335741524`
- Original body SHA256: `bb0cb46d13a4b48123f3bb79b58e148b887ca5a75deeafd4661fbca5911c917b`
- Proposed body SHA256: `ba346682c07b2254510b30c85014e4a413a62c1dd3b02ca8e46dc19aaa14cbff`

<!-- cua-release-backfill:v1:start evidence-sha256=2aac2fbd7db7f7ffe793b23de1e2033b0a3fa643f77838e2411043dd7a5bedcf -->
## Summary

This patch fixes image pulls for macOS Sequoia images that would stall and loop endlessly with checksum failures.

## Fixes

- Fixed pulls of macOS Sequoia images stalling and repeatedly retrying due to corrupted downloads being silently cached; downloaded layers are now verified against their checksum and corrupted data is discarded instead of being reused. ([#1855](https://github.com/trycua/cua/pull/1855)) Thanks @pa4uslf, @RomneyDa.

## Contributors

Thanks to @pa4uslf, @RomneyDa for contributing to this release.

## Full changelog

[lume-v0.3.9...lume-v0.3.10](https://github.com/trycua/cua/compare/lume-v0.3.9...lume-v0.3.10)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.11

- Tag: `lume-v0.3.11`
- Release ID: `351030315`
- Original body SHA256: `efc340de94ae38ae50072c505140cb5092eb0b9dbc9f27427c6e25bd7ba2ac53`
- Proposed body SHA256: `a7f7601a30ec108e9d073b162fce30080cdfb229118475f308c7f83b38382e37`

<!-- cua-release-backfill:v1:start evidence-sha256=1f2efc5a0ba8ce521c0fd08e8d0a41d67bc4420cf7e92b350e693d8e8ebb292c -->
## Summary

This release adds an explicit update flow for Lume and removes the automatic background updater.

## Features

- Added explicit check-update and update commands, and stopped installing a scheduled background auto-updater. ([#2098](https://github.com/trycua/cua/pull/2098))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.10...lume-v0.3.11](https://github.com/trycua/cua/compare/lume-v0.3.10...lume-v0.3.11)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.12

- Tag: `lume-v0.3.12`
- Release ID: `352286846`
- Original body SHA256: `ede7d75b6bba032d3d6fa2df81271a3020f8e162ed0ca491ebbf5feac8216b4b`
- Proposed body SHA256: `1226c2a1200d0a418515c34254868403b87f639bfb4901cd92809da1a9c06993`

<!-- cua-release-backfill:v1:start evidence-sha256=dea95999521b999a7a3611c348ffbb7e142e93e4c281355d775ec749ce25a328 -->
## Summary

This release replaces the GUI-driven unattended macOS setup with an offline disk-patching approach for more reliable VM provisioning.

## Features

- Unattended macOS setup now patches the disk offline instead of relying on a GUI or agent-driven process, and creates a dedicated user with SSH access and autologin enabled. ([#2156](https://github.com/trycua/cua/pull/2156))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.11...lume-v0.3.12](https://github.com/trycua/cua/compare/lume-v0.3.11...lume-v0.3.12)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.13

- Tag: `lume-v0.3.13`
- Release ID: `352613776`
- Original body SHA256: `e4fc938763df56f16018ac4c633ac2fccd6bab045e4350ec194280093a4c8fbc`
- Proposed body SHA256: `75ef5ab4be55ff75edbddf6ed44972f9b5298bd9a6faf4b450946be19383b88e`

<!-- cua-release-backfill:v1:start evidence-sha256=bd1a618781e3580d0ce4b45895930acab8fd781c448bc1146b2c2746ff25b986 -->
## Summary

This release adds automated control of System Integrity Protection on macOS VMs and moves installer downloads to a new domain.

## Features

- Added a command to enable or disable System Integrity Protection on stopped macOS VMs by automating the process through macOS Recovery. ([#2165](https://github.com/trycua/cua/pull/2165))

## Fixes

- Installer and update downloads now come from cua.ai instead of the previous GitHub-hosted URLs. ([#2152](https://github.com/trycua/cua/pull/2152))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.12...lume-v0.3.13](https://github.com/trycua/cua/compare/lume-v0.3.12...lume-v0.3.13)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.14

- Tag: `lume-v0.3.14`
- Release ID: `352628930`
- Original body SHA256: `92b2ed03d383acb18955db237e3d696eb0dede512559327d26b2bd457fa00e19`
- Proposed body SHA256: `242304f1d9eaeaf1c7ddaecb813b30771a86da0cc01e1ffb0535433232bd142d`

<!-- cua-release-backfill:v1:start evidence-sha256=d41261163f85295c9392ee2c6c12923492537ad97dc3c80e89107276fd1f2b34 -->
## Summary

This release makes freshly created unattended macOS VMs Recovery-ready by fixing an authorization issue that blocked disabling System Integrity Protection, and improves installer reliability in non-interactive shells.

## Fixes

- Refresh APFS Preboot metadata after unattended guest finalization so the offline-created administrator account is authorized in paired Recovery, fixing failures when running lume sip off. ([#2167](https://github.com/trycua/cua/pull/2167))
- Make the installer's color setup tolerate missing terminal capabilities instead of exiting early in non-TTY shells. ([#2167](https://github.com/trycua/cua/pull/2167))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.13...lume-v0.3.14](https://github.com/trycua/cua/compare/lume-v0.3.13...lume-v0.3.14)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.15

- Tag: `lume-v0.3.15`
- Release ID: `352807299`
- Original body SHA256: `89a068be9528693fbe8b04629034250338d3fe45029c00bdc0026e506bd548b3`
- Proposed body SHA256: `f972894795fd828a1330cb191a4a5bb5cbad28594dfa2ee2fe8bb12303001739`

<!-- cua-release-backfill:v1:start evidence-sha256=60792cead4d1a4d86c3cdf2ecd8214d43f54da0fac997ea483ab6160a7db4f34 -->
## Summary

This release adds the ability to safely expand macOS VM disks, relocating the paired Recovery partition and growing the APFS container while preserving VM identity and boot state.

## Features

- Add support for expanding macOS VM disks with recovery-preserving offline resizing, exposed through the CLI, HTTP API, and MCP server, including transactional rollback and validation safeguards. ([#2174](https://github.com/trycua/cua/pull/2174))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.14...lume-v0.3.15](https://github.com/trycua/cua/compare/lume-v0.3.14...lume-v0.3.15)
<!-- cua-release-backfill:v1:end -->

# Review: Lume 0.3.16

- Tag: `lume-v0.3.16`
- Release ID: `354128492`
- Original body SHA256: `67e95c5fc10db06c263cd065c8a9c17e1a6510ec72aa9edc5d04e9b7db54ce47`
- Proposed body SHA256: `83d55039936a521d91fc023ba262e4914c7d7aab7a8e0f651762c234d1792fe8`

<!-- cua-release-backfill:v1:start evidence-sha256=8b8538db74f2aacd5f345657f693aa570ee67e4a643c9aac5d8aa237d3e62ad5 -->
## Summary

This release increases the default macOS VM disk size to 100GB, fixes SIP automation for macOS 26 guests, and adds consent-aware telemetry with persistent disable controls.

## Features

- Default new macOS VM disks to 100GB in the CLI and MCP create tool, while preserving the existing 50GB Linux default and explicit disk-size overrides. ([#2191](https://github.com/trycua/cua/pull/2191))
- Add default-on, consent-aware telemetry with persistent disable, reset, and purge controls, sending content-free usage events while excluding prompts, screenshots, file paths, and other sensitive data. ([#2211](https://github.com/trycua/cua/pull/2211))

## Fixes

- Handle the new macOS 26 authorized-user prompt during SIP automation, so disabling or enabling System Integrity Protection completes correctly on macOS 26 guests while remaining compatible with older guests. ([#2203](https://github.com/trycua/cua/pull/2203))

## Contributors

This release contains maintainer changes only.

## Full changelog

[lume-v0.3.15...lume-v0.3.16](https://github.com/trycua/cua/compare/lume-v0.3.15...lume-v0.3.16)
<!-- cua-release-backfill:v1:end -->
