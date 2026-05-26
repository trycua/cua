# cua-driver test harness

Deterministic host apps that present a fixed scenario set for cua-driver
to drive. The apps cover the hosting patterns that matter for background
automation on each OS:

- **Windows**: two minimal .NET 8 host apps — `CuaTestHarness.Wpf` and
  `CuaTestHarness.WinUI3`
- **macOS**: two single-file Swift host apps — `CuaTestHarness.AppKit`
  and `CuaTestHarness.SwiftUI`

## What's exercised

WPF scenarios (`CuaTestHarness.Wpf`):
- Plain XAML controls + AutomationIds (`counter`, `text_body`)
- TextBox + mirrored label (`text_input` — drives `type_text` and `set_value`)
- Right/double-click target (`click_target` — drives `right_click` /
  `double_click` via `MouseRightButtonDown` / `MouseDoubleClick`)
- Scroll body with VerticalOffset label + WM_VSCROLL hook (`scroll_target`)
- Modal `MessageBox` (`message_box`)
- Save/Cancel bottom strip — the layout that previously clipped under
  `GetClientRect` (`bottom_strip` — regression guard for PR #1696)
- Native Win32 child HWNDs via `HwndHost` (`child_hwnd`)
- Owned secondary `Window` (`owned_popup`)
- Layered transparent window (`layered_popup`)
- Keyboard accelerators — F5 (no modifier) + Ctrl+Shift+H (`accelerator`)

WinUI3 scenarios (`CuaTestHarness.WinUI3`):
- `counter` / `text_body` / `exit` (parity with WPF)
- TextBox + mirror (`text_input` — drives `type_text` via UIA `ValuePattern`)
- `CommandBarFlyout` — popup in same HWND, DComp-rendered
- XAML `Popup` — primitive, not a separate HWND

`scenarios/scenarios.json` is the single source of truth — both the C#
host apps and the Rust integration tests read it so AutomationIds /
window titles never drift between the two halves.

## Coverage matrix (Rust integration tests)

| Capability                       | WPF             | WinUI3          |
|----------------------------------|-----------------|-----------------|
| Smoke (UIA tree + AutomationIds) | ✅              | ✅              |
| UIA Invoke click                 | ✅              | (implicit)      |
| `right_click` (PostMessage/SendInput) | ✅         | —               |
| `double_click`                   | ✅              | —               |
| `type_text` (PostMessage WM_CHAR / UIA ValuePattern) | ✅ | ✅           |
| `set_value` (UIA ValuePattern)   | ✅              | (covered)       |
| `scroll` (WM_VSCROLL via hook)   | ✅              | —               |
| `press_key` accelerator (F5)     | ✅              | —               |
| Modal `MessageBox` open + parse + dismiss | ✅     | —               |
| Owned popup open + parse         | ✅              | —               |
| Layered popup capture (non-black assertion) | ✅   | —               |
| XAML Popup open + parse          | —               | ✅              |

## Layout

```text
test-harness/
├── CuaTestHarness.sln
├── build.ps1                         # publishes both projects
├── scenarios/scenarios.json          # shared SoT
├── CuaTestHarness.Wpf/               # .NET 8, WPF, x64
└── CuaTestHarness.WinUI3/            # .NET 8, WinUI3 unpackaged, x64
```

Publish output is staged into `../rust/test-apps/harness-{wpf,winui3}/`
so the existing sandbox runner picks them up via its mapped-folder route
(same path convention as `desktop-test-app-electron`).

## Build

Requires the **.NET 8 SDK** on `PATH`:
```powershell
winget install Microsoft.DotNet.SDK.8
```

Then:
```powershell
cd libs\cua-driver\test-harness
.\build.ps1                 # both projects
.\build.ps1 -Skip winui3    # WPF only (fast path for local iteration)
```

## Run the tests

Locally:
```powershell
cd libs\cua-driver\rust
cargo test --test harness_wpf_test     -- --ignored --nocapture
cargo test --test harness_winui3_test  -- --ignored --nocapture
```

In Windows Sandbox (matches CI):
```powershell
.\rust\sandbox\run-tests-in-sandbox.ps1 harness_wpf
.\rust\sandbox\run-tests-in-sandbox.ps1 harness_winui3
```

The sandbox runner auto-detects `dotnet` on the host and rebuilds the
harness before launching. If `dotnet` isn't installed the harness tests
silently skip (the rest of the suite still runs).

---

## macOS — AppKit + SwiftUI harness

### What's exercised

AppKit (`CuaTestHarness.AppKit`):
- Plain NSButton + NSTextField counter (`counter` — AXPress on a Cocoa NSButton)
- NSTextField label with shared marker (`text_body`)
- Editable NSTextField + mirror label (`text_input` — drives `type_text` /
  `set_value` via AXValue)
- Custom NSView click-target (`click_target` — distinguishes
  `right_click`, `double_click`, plain `click`)
- NSScrollView with tall NSTextView body + offset label (`scroll_target`)
- Top NSMenu item with known title (`ns_menubar` — Mac-specific scenario
  that has no Windows analogue)
- Exit button (`exit`)

SwiftUI (`CuaTestHarness.SwiftUI`):
- Counter / text_body / text_input parity with AppKit
- SwiftUI `.popover()` — Mac analogue of WinUI3 `CommandBarFlyout` /
  XAML `Popup` (`popover`)
- Exit button

### Coverage matrix (Rust integration tests)

| Capability                       | AppKit       | SwiftUI       |
|----------------------------------|--------------|---------------|
| Smoke (AX tree + identifiers)    | ✅           | ✅            |
| Counter click via element_index  | ✅           | (implicit)    |
| `set_value` (AXSetAttribute)     | ✅           | (covered)     |
| Popover open + body assert       | —            | ✅            |
| `right_click` / `double_click`   | scenario in fixture, manual today | — |
| Per-tool CLI smoke               | covered by `scripts/mac-smoke.sh` | — |

### Layout (macOS half)

```text
test-harness/
├── CuaTestHarness.AppKit/
│   └── main.swift                # single-file AppKit host
├── CuaTestHarness.SwiftUI/
│   └── main.swift                # single-file SwiftUI host
├── scenarios/scenarios.json      # shared SoT — `appkit` + `swiftui` sections
└── build.sh                      # macOS build script (parallels build.ps1)
```

Built outputs are staged into `../rust/test-apps/harness-{appkit,swiftui}/`
as `.app` bundles — same convention as the Windows side.

### Build (macOS)

Requires Xcode command line tools on `PATH` (provides `xcrun swiftc`):

```bash
cd libs/cua-driver/test-harness
./build.sh                  # both apps
./build.sh --skip swiftui   # AppKit only (fast iteration)
./build.sh --clean          # nuke stage dirs first
```

The build script compiles each `main.swift` with `xcrun swiftc -O
-target arm64-apple-macos13.0`, then writes a minimal `Info.plist`
into a manually-constructed `.app` bundle. No Xcode project files, no
Swift Package Manager — keeps the bar to "have Xcode CLT installed."

### Run the tests (macOS)

```bash
cd libs/cua-driver/rust
cargo test --release --test harness_appkit_test    -- --ignored --nocapture
cargo test --release --test harness_swiftui_test   -- --ignored --nocapture
```

Tests are `#[ignore]` so they don't run in plain `cargo test` (they
require the harness to be built and TCC Accessibility permission). On a
fresh Mac, when TCC is NOT granted to the test runner, the smoke tests
print a TCC hint and exit cleanly rather than misreporting as a hard
failure.

To grant TCC: System Settings → Privacy & Security → Accessibility →
add the test binary (`target/release/cua-driver`) or your terminal app.

### Per-tool CLI smoke (`scripts/mac-smoke.sh`)

Companion to the MCP integration tests. Spawns the AppKit harness as a
victim, then iterates every cua-driver tool with sensible JSON args and
reports PASS / FAIL / SKIP in a sorted table. Runs in ~25 seconds.

```bash
scripts/mac-smoke.sh
```

A baseline result file (`scripts/mac-smoke-RESULTS.txt`) is checked in
for the current driver version (0.2.18) — diff against it to catch
regressions in tool coverage.

### AppKit / SwiftUI AX quirks captured in the tests

- **`accessibilityIdentifier` on AXStaticText doesn't propagate.**
  NSTextField label-mode + SwiftUI `Text` views render as AXStaticText
  leaves and the OS drops the identifier slot. Tests assert AX-id only
  on actionable controls (Buttons, TextFields, menu items) and assert
  on text-content for labels. Same quirk as WPF's TextBlock.
- **SwiftUI popovers may live in a separate AXWindow.** The popover
  test walks `list_windows` for any new window owned by the harness pid
  if the trigger window doesn't contain the marker.
- **AX element budget caps tree depth + width.** The harness keeps its
  scroll-body to 30 lines so later scenarios don't get truncated.
