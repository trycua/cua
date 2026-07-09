# test-harness apps

Each subdirectory is one deterministic host app cua-driver can drive. Apps
share fixtures through `../shared/scenarios.json` and, for webview hosts,
`../shared/web/index.html`.

| Path | Host OS | What it tests |
| --- | --- | --- |
| `cross-platform/electron` | any | Electron/Chromium accessibility and CDP |
| `cross-platform/tauri` | any | Tauri/native-webview accessibility |
| `linux/gtk3` | Linux | GTK3/AT-SPI widgets |
| `macos/appkit` | macOS | AppKit controls and menus |
| `macos/swiftui` | macOS | SwiftUI controls and popovers |
| `macos/wkwebview` | macOS | WKWebView loading the shared DOM |
| `windows/wpf` | Windows | WPF/UIA controls, popups, child HWNDs |
| `windows/winui3` | Windows | WinUI3/XAML controls and popups |
| `windows/webview2` | Windows | WebView2 loading the shared DOM |

Built outputs land in `../../rust/test-apps/harness-<name>/` via
`../build/<host>`.

Adding a new app:

1. Put native source under `apps/<platform>/<toolkit>/`, or
   `apps/cross-platform/<toolkit>/` when the same source runs on multiple OSes.
2. Reuse `shared/scenarios.json` and `shared/web/index.html` instead of copying
   private ids.
3. Stage into `rust/test-apps/harness-<toolkit>/`.
4. Add or update the matching Rust/Python tests and this table.
