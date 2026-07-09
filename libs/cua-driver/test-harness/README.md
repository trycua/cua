# cua-driver test harness

Deterministic host apps that present a fixed scenario set for cua-driver
to drive. One layout, three buckets: cross-OS reusable assets in
`shared/`, OS-specific (or cross-platform) target apps in `apps/`,
host-OS build scripts in `build/`, host-OS CLI smoke runners in
`smoke/`.

## Layout

```text
test-harness/
├── shared/                  # cross-OS reusable
│   ├── scenarios.json       # single SoT for AutomationIds / window titles
│   └── web/index.html       # WebView2 + Electron load this same file
├── apps/                    # the test-target host apps
│   ├── cross-platform/
│   │   └── electron/        # CuaTestHarness.Electron (CDP host, runs on any OS)
│   ├── macos/
│   │   ├── appkit/          # CuaTestHarness.AppKit  (single-file Swift)
│   │   └── swiftui/         # CuaTestHarness.SwiftUI (single-file Swift)
│   └── windows/
│       ├── wpf/             # CuaTestHarness.Wpf      (.NET 8 WPF)
│       ├── winui3/          # CuaTestHarness.WinUI3   (.NET 8 WinUI3 unpackaged)
│       └── webview2/        # CuaTestHarness.WebView  (WPF + Microsoft.Edge.WebView2)
├── build/                   # host-OS build scripts (publish into ../rust/test-apps/)
│   ├── macos.sh             # builds appkit + swiftui + wkwebview + electron
│   └── windows.ps1          # builds wpf + winui3 + webview2 + electron
└── smoke/                   # host-OS per-tool CLI smoke runners
    ├── macos.sh             # spawns appkit harness, iterates every tool
    └── results/macos.txt    # checked-in baseline for diff-based regression detection
```

`shared/scenarios.json` is the single source of truth — both the host
apps and the Rust integration tests read it so AutomationIds, AX
identifiers, and window titles never drift between the two halves.

Published build outputs land in `../rust/test-apps/harness-{wpf,winui3,webview,electron,appkit,swiftui}/`
so the existing sandbox runner and Rust integration tests pick them up
without further wiring.

## What's exercised

WPF (`apps/windows/wpf`):
- Plain XAML controls + AutomationIds (`counter`, `text_body`)
- TextBox + mirrored label (`text_input`)
- Right/double-click target (`click_target`)
- Scroll body with VerticalOffset label + WM_VSCROLL hook (`scroll_target`)
- Modal `MessageBox` (`message_box`)
- Save/Cancel bottom strip — regression guard for PR #1696 (`bottom_strip`)
- Native Win32 child HWNDs via `HwndHost` (`child_hwnd`)
- Owned secondary `Window` (`owned_popup`)
- Layered transparent window (`layered_popup`)
- Keyboard accelerators — F5 + Ctrl+Shift+H (`accelerator`)

WinUI3 (`apps/windows/winui3`):
- `counter` / `text_body` / `exit` (parity with WPF)
- TextBox + mirror (`text_input` — UIA `ValuePattern`)
- `CommandBarFlyout` — popup in same HWND, DComp-rendered
- XAML `Popup` — primitive, not a separate HWND

WebView2 (`apps/windows/webview2`):
- Loads `shared/web/index.html` — same page as the Electron harness.
- Exposes CDP on `default_cdp_port=9222` so cua-driver's `page` tool can drive it.

Electron (`apps/cross-platform/electron`):
- Loads the same `shared/web/index.html`. CDP on `default_cdp_port=9223`.

AppKit (`apps/macos/appkit`):
- Plain NSButton + NSTextField counter (`counter`)
- NSTextField label with shared marker (`text_body`)
- Editable NSTextField + mirror label (`text_input`)
- Custom NSView click-target (`click_target`)
- NSScrollView with tall NSTextView body + offset label (`scroll_target`)
- Top NSMenu item with known title (`ns_menubar`)
- Exit button

SwiftUI (`apps/macos/swiftui`):
- Counter / text_body / text_input parity with AppKit
- SwiftUI `.popover()` — Mac analogue of WinUI3 CommandBarFlyout / XAML Popup
- Exit button

## Coverage matrices (Rust integration tests)

Windows:

| Capability                          | WPF             | WinUI3       |
|-------------------------------------|-----------------|--------------|
| Smoke (UIA tree + AutomationIds)    | yes             | yes          |
| UIA Invoke click                    | yes             | (implicit)   |
| `right_click`                       | yes             | —            |
| `double_click`                      | yes             | —            |
| `type_text` (PostMessage / UIA)     | yes             | yes          |
| `set_value` (UIA ValuePattern)      | yes             | (covered)    |
| `scroll` (WM_VSCROLL via hook)      | yes             | —            |
| `press_key` accelerator (F5)        | yes             | —            |
| Modal `MessageBox` open + dismiss   | yes             | —            |
| Owned popup open + parse            | yes             | —            |
| Layered popup capture               | yes             | —            |
| XAML Popup open + parse             | —               | yes          |

macOS:

| Capability                          | AppKit          | SwiftUI      |
|-------------------------------------|-----------------|--------------|
| Smoke (AX tree + identifiers)       | yes             | yes          |
| Counter click via element_index     | yes             | (implicit)   |
| `set_value` (AXSetAttribute)        | yes             | (covered)    |
| Popover open + body assert          | —               | yes          |
| Per-tool CLI smoke                  | `smoke/macos.sh`| —            |

## Build

macOS (requires Xcode CLT for `xcrun swiftc`; Node.js + npm for Electron):

```bash
libs/cua-driver/test-harness/build/macos.sh                 # all macOS-runnable harnesses
libs/cua-driver/test-harness/build/macos.sh --skip swiftui  # skip one target
libs/cua-driver/test-harness/build/macos.sh --skip electron # skip Electron / npm
libs/cua-driver/test-harness/build/macos.sh --clean        # nuke stage dirs first
```

Windows (requires .NET 8 SDK on PATH; Node.js + npm for the Electron app):

```powershell
cd libs\cua-driver\test-harness
.\build\windows.ps1                  # all four apps
.\build\windows.ps1 -Skip winui3     # skip a specific one
```

Linux (requires Node.js + npm for the Electron app):

```bash
cd libs/cua-driver/test-harness/apps/cross-platform/electron
./build.sh
```

## Run the integration tests

```bash
# macOS
cd libs/cua-driver/rust
cargo test --release --test harness_appkit_test    -- --ignored --nocapture
cargo test --release --test harness_swiftui_test   -- --ignored --nocapture

# Windows (locally)
cd libs\cua-driver\rust
cargo test --test harness_wpf_test     -- --ignored --nocapture
cargo test --test harness_winui3_test  -- --ignored --nocapture

# Windows Sandbox (matches CI)
.\rust\sandbox\run-tests-in-sandbox.ps1 harness_wpf
.\rust\sandbox\run-tests-in-sandbox.ps1 harness_winui3
```

Tests are `#[ignore]` so they don't run in plain `cargo test` (they
require the harness apps to be built and, on macOS, TCC Accessibility
granted to the test runner).

## Per-tool CLI smoke

Companion to the MCP integration tests. Spawns the AppKit harness as a
victim, then iterates every cua-driver tool with sensible JSON args and
reports PASS / FAIL / SKIP in a sorted table. Runs in ~25 seconds.

```bash
libs/cua-driver/test-harness/smoke/macos.sh
```

A baseline result file (`smoke/results/macos.txt`) is checked in for the
current driver version — diff against it to catch regressions in tool
coverage.

## AppKit / SwiftUI AX quirks captured in the tests

- `accessibilityIdentifier` on AXStaticText doesn't propagate.
  NSTextField label-mode + SwiftUI `Text` views render as AXStaticText
  leaves and the OS drops the identifier slot. Tests assert AX-id only
  on actionable controls and assert on text-content for labels.
- SwiftUI popovers may live in a separate AXWindow. The popover test
  walks `list_windows` for any new window owned by the harness pid if
  the trigger window doesn't contain the marker.
- AX element budget caps tree depth + width. The scroll-body is capped
  at 30 lines so later scenarios don't get truncated.
