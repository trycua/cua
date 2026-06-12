# test-harness apps

Each subdirectory is one self-contained host app cua-driver drives. They
share fixtures via `../shared/scenarios.json` and (where relevant)
`../shared/web/index.html` so AutomationIds, AX identifiers, and DOM
ids never drift between the apps and the integration tests.

| Path                          | Host OS         | What it tests                                                   |
|-------------------------------|-----------------|-----------------------------------------------------------------|
| `cross-platform/electron`     | any             | Electron + CDP — exercises cua-driver's `page` tool             |
| `macos/appkit`                | macOS 13+       | AppKit / Cocoa hosting — NSButton, NSScrollView, NSMenu, etc.   |
| `macos/swiftui`               | macOS 13+       | SwiftUI hosting — `.popover()`, declarative state               |
| `windows/wpf`                 | Windows         | WPF / UIA — popups, layered windows, modal MessageBox, hwndhost |
| `windows/winui3`              | Windows         | WinUI3 unpackaged — DComp popups, XAML Popup primitive          |
| `windows/webview2`            | Windows         | Microsoft.Edge.WebView2 hosting the same DOM as Electron        |

Built outputs land in `../../rust/test-apps/harness-{appkit,swiftui,wpf,winui3,webview,electron}/`
via `../build/macos.sh` or `..\build\windows.ps1`. The Rust integration
tests under `../../rust/crates/cua-driver/tests/harness_*.rs` consume
those staged outputs.

Adding a new app:

1. Pick the right OS bucket (or `cross-platform/` if Electron-equivalent).
2. Mirror an existing app's layout — Swift `main.swift` for macOS,
   `.csproj` + XAML for Windows.
3. Add a section to `../shared/scenarios.json` with the AutomationIds /
   AX identifiers / window title your tests will assert on.
4. Wire it into `../build/{macos.sh,windows.ps1}` so it stages into
   `rust/test-apps/harness-<name>/`.
