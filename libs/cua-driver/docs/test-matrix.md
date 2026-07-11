# CUA Driver Test Matrix

This is the source map for the Rust test suites in `libs/cua-driver`. It uses
two top-level test classes:

1. **Unit and deterministic protocol tests.** These do not depend on a repo-local
   application or an interactive desktop.
2. **Harness E2E tests.** These build and launch a repo-local application, drive
   it through the Rust driver, and verify an external application or desktop
   oracle.

A test is E2E because it crosses the driver, OS, window system, and application
boundary. The Rust harness catalog and its external oracles are the source of
truth.

## Matrix Dimensions

Every harness E2E result should identify these dimensions:

| Dimension | Values |
| --- | --- |
| OS | `windows`, `macos`, `linux` |
| Window system | Win32/UIA, AppKit/AX, X11/AT-SPI, Wayland/AT-SPI, WebView/CDP |
| Harness | Electron, Tauri, WPF, WinUI3, WebView2, AppKit, SwiftUI, WKWebView, GTK3 |
| Action targeting | `ax`, `px`, `page`, `not_applicable` |
| Delivery | `background`, `foreground`, `N/A` |
| Scope | `window`, `desktop`, `N/A` |
| Oracle | App state, accessibility state, focus state, pixel state, protocol state |
| Test status | `pass`, `fail`, `skip`, `environment_error` |
| Observed behavior | `delivered`, `refused`, `no_effect`, `error`, `not_run` |

Focus preservation is a cross-cutting oracle, not a separate matrix family.
When a background action is tested, the row should attach the platform focus
observer in addition to checking the target application's external state.

`AX` and `PX` describe how an action addresses a target. They are not capture
modes. `get_window_state` returns the tree and screenshot together; the action
uses an element index or coordinates.

## Unit And Deterministic Tests

These run without the repo-local GUI applications:

| Area | Location | Coverage |
| --- | --- | --- |
| Core driver logic | `rust/crates/*/src/**` | Protocol values, sessions, schemas, image helpers, input helpers, configuration, telemetry, CLI behavior |
| MCP and CLI boundary | `rust/crates/cua-driver/tests/protocol_*` | Handshake, tool registration, tool calls, media, sessions, and errors |
| Schema gate | `schema_consistency_test.rs` | Shared tool schema parity across OS backends |
| Configuration transport | `transport_config_persistence_test.rs` | CLI and MCP configuration persistence |
| Token and protocol surfaces | `protocol_element_token_test.rs`, related tests | JSON-RPC-visible contract behavior |

Some protocol tests spawn the driver process. They remain deterministic because
they do not launch a real target application or require a desktop. They should
be reported with the unit gate, separately from Harness E2E.

The ordinary unit gate must not run `#[ignore]` GUI tests and must not turn
missing desktop fixtures into silent skips.

## Harness E2E: Shared Web Applications

Electron and Tauri are separate repo-local applications that load the shared
web harness. The calculator fixture was removed because it added a synthetic
task without adding useful application diversity.

Source:

- `tests/fixtures/shared/web/index.html`
- `tests/fixtures/apps/cross-platform/electron/`
- `tests/fixtures/apps/cross-platform/tauri/`
- `rust/crates/cua-driver/tests/cross_platform_behavior_test.rs`

The shared harness exposes deterministic external markers for these actions:

| Action family | Actions | Addressing | Delivery |
| --- | --- | --- | --- |
| Pointer | Left click, right click, double click | AX and PX | Background and foreground |
| Keyboard and text | Type text, Return, hotkey | AX and PX where supported | Background and foreground |
| Scroll | Scroll | AX and PX where supported | Background and foreground |
| Child windows | Open child window | AX and PX | Background and foreground |
| Drag | Drag source to drop target | PX | Background and foreground |
| State controls | Checkbox, radio, combo, slider | AX or PX by control | Mode declared per action |
| Editor | Type, save, saved-state readback | AX and PX where supported | Mode declared per action |

The Rust shared catalog declares 36 evidence-bearing cells per host: the full
AX/PX and foreground/background cross-product for click, text, keyboard,
scroll, and child-window actions, plus both delivery modes for PX drag and AX
editor-save. A
background capability refusal is a valid result only when the test verifies the
declared structured refusal and the no-focus/no-z-order/no-input-leak side
effect. A refusal fails a cell whose contract requires delivery.

## Harness E2E: Native Windows

Windows native harnesses are repo-local applications built from source:

| Harness | Source test | Coverage |
| --- | --- | --- |
| WPF | `harness_wpf_test.rs` | UIA controls, text, keys, pointer actions, scroll, drag, popups, menus, modal windows |
| WinUI3 | `harness_winui3_test.rs` | XAML controls, text, checkbox/radio, slider, combo, popup |
| WebView2 | `harness_web_test.rs` | Window discovery, CDP page access, JavaScript, DOM click path |
| Desktop invariants | Testkit `DesktopObserver` plus typed launch/capture/cursor owners | Cross-cutting focus, z-order, minimized-launch, screenshot, cursor, and desktop checks |

Native controls use AX/UIA state as their oracle. Pointer actions also use PX
where the tool contract requires coordinates. Current `set_value` rows declare
background delivery and attach desktop-side-effect oracles.

## Harness E2E: macOS

| Harness | Source test | Coverage |
| --- | --- | --- |
| AppKit | `harness_appkit_test.rs` | Native AX controls, text, pointer actions, menus, popups, scroll, drag |
| SwiftUI | `harness_swiftui_test.rs` | SwiftUI controls, popovers, toggles, sliders, context menus, scroll |
| WKWebView | `tests/fixtures/apps/macos/wkwebview/` | Fixture is built from the shared web page; a dedicated Rust E2E target is still a coverage gap |
| Installed-app launch/focus | `installed_app_launch_macos_test.rs` | Real Calculator/TextEdit launch and focus behavior, when explicitly enabled |
| Installed-app text | `installed_app_textedit_macos_test.rs` | Real TextEdit AX background write and verification, when explicitly enabled |

macOS uses the installed ScreenCaptureKit/AX permissions for GUI runs. The
repo-local harnesses are canonical; Calculator and TextEdit are supporting
real-app checks.

## Harness E2E: Linux

| Harness | Source test | Window system | Coverage |
| --- | --- | --- | --- |
| Electron | `cross_platform_behavior_test.rs` | X11 and supported AT-SPI sessions | Shared web action matrix |
| Tauri | `cross_platform_behavior_test.rs` | X11 and native Wayland sessions | Shared web action matrix |
| GTK3 | `harness_gtk3_test.rs` | X11/AT-SPI and Wayland/AT-SPI where configured | Native GTK controls and input |
| Desktop scope | `desktop_scope_linux_test.rs` | X11/Wayland | Desktop versus window scope |

Nix provides the Linux build and desktop environment. X11 and Wayland are
separate matrix dimensions because their capture and input contracts differ.
Linux runs do not require GIF output. They may retain MP4, screenshots, AX
trees, structured results, and driver logs for E2E evidence.

## Action Delivery Matrix

For each OS and harness where the action is supported, the canonical E2E suite
should cover both delivery modes:

| Action | Background | Foreground | Addressing |
| --- | --- | --- | --- |
| Left click | Required delivery or a declared refusal contract | Required | AX, PX |
| Right click | Required delivery or a declared refusal contract | Required | AX, PX |
| Double click | Required delivery or a declared refusal contract | Required | AX, PX |
| Drag | Required delivery or a declared refusal contract | Required | PX, with AX discovery |
| Scroll | Required delivery or a declared refusal contract | Required | AX, PX where supported |
| Type text | Required delivery or a declared refusal contract | Required | AX, PX where supported |
| Press key | Required delivery or a declared refusal contract | Required | AX, PX where supported |
| Hotkey | Required delivery or a declared refusal contract | Required | AX, PX where supported |
| Set value | Required in current native rows | Not separately declared | AX/UIA/AXValue |
| Screenshot | N/A | N/A | Window or desktop capture |
| Page/CDP | N/A | N/A | Page selector/JavaScript |

`N/A` means the API has no delivery mode for that operation. A refusal is a
passing observation only when the Rust case expects refusal, the exact code is
allowed, and the desktop-side-effect oracles pass.

## CI And Local Entry Points

| Gate | Environment | Entry point |
| --- | --- | --- |
| Linux unit/source | Nix Linux CI | `nix build .#checks.x86_64-linux.cua-driver-build .#checks.x86_64-linux.cua-driver-linux-rust-unit` |
| Windows unit/compile | `windows-latest` | Package-scoped `cargo test --all-targets --no-run --locked` |
| Windows Harness E2E | Active Windows user session | `scripts/ci/windows/run-rust-e2e.ps1 -RequireGui` |
| Linux Harness E2E | Nix X11 or Wayland session | `scripts/ci/linux/run-rust-e2e.sh` |
| macOS Harness E2E | Logged-in macOS session with permissions | `scripts/ci/macos/run-rust-e2e.sh` |

Workflows select private execution lanes. Rust source owns scenario definitions,
fixture oracles, and result records. OS runners only build the driver, stage the
local harnesses, establish the desktop session, collect evidence, and publish
the shared report.

## Evidence And Ownership

Every Harness E2E cell should produce:

```text
recordings/<cell-label>-pid<pid>-<sequence>/recording.mp4
recordings/<cell-label>-pid<pid>-<sequence>/trajectory.json
results.jsonl
<rust-target>.log
```

The GitHub summary links the exact recording path from the corresponding row to
its lane archive. Target logs remain lane-level diagnostics. Unit tests need
logs and test-result output, but do not need desktop video.

When a new action or modality is added, update this document, the shared fixture
oracle, the Rust test, and the OS-specific runner selection together. This is
the cross-OS checklist that prevents a Windows-only test from being mistaken
for cross-platform coverage.
