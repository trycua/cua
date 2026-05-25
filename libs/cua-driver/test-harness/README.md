# cua-driver Windows test harness

Two minimal .NET 8 host apps (WPF + WinUI3 unpackaged) that present a
deterministic set of UI scenarios for `cua-driver` to drive. They cover
the Windows hosting patterns that matter for background automation:

- Plain XAML controls + AutomationIds (`counter`, `text_body`)
- Modal `MessageBox` (`message_box`)
- Save/Cancel bottom strip — the layout that previously clipped under
  `GetClientRect` (`bottom_strip` — regression guard for PR #1696)
- Native Win32 child HWNDs via `HwndHost` (`child_hwnd`)
- Owned secondary `Window` (`owned_popup`)
- Layered transparent window (`layered_popup`)
- Keyboard accelerators (`accelerator`)
- `CommandBarFlyout` (`command_bar_flyout`, WinUI3 only)
- XAML `Popup` (`xaml_popup`, WinUI3 only) — in-window, not a separate HWND

`scenarios/scenarios.json` is the single source of truth — both the C#
host apps and the Rust integration tests read it so AutomationIds /
window titles never drift between the two halves.

## Layout

```
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
