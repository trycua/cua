# cua-driver test fixtures

Deterministic host apps and shared fixtures used to verify cua-driver across
Windows, macOS, and Linux. The fixtures are source-first: build scripts stage
local outputs into `libs/cua-driver/rust/test-apps/harness-<name>/`; binaries are not
committed.

## Layout

```text
tests/fixtures/
├── shared/
│   ├── scenarios.json       # scenario ids, titles, and expected controls
│   └── web/index.html       # shared DOM for webview-style harnesses
├── apps/
│   ├── cross-platform/
│   │   ├── electron/        # Chromium/Electron host, CDP port 9223
│   │   └── tauri/           # native webview/Tauri host
│   ├── linux/
│   │   └── gtk3/            # PyGObject GTK3 app
│   ├── macos/
│   │   ├── appkit/          # single-file Swift AppKit app
│   │   ├── swiftui/         # single-file SwiftUI app
│   │   └── wkwebview/       # native WKWebView host for shared DOM
│   └── windows/
│       ├── wpf/             # .NET WPF app
│       ├── winui3/          # unpackaged WinUI3 app
│       └── webview2/        # WPF + WebView2 host for shared DOM
├── build/                   # host build scripts
├── smoke/                   # lightweight local smoke runners
├── linux-container/         # Azure/XFCE Linux verification helpers
└── modality-recordings/     # recorder scripts and dashboards
```

`shared/scenarios.json` is the source of truth for AutomationIds, AX
identifiers, expected window titles, and fixture names. Webview-style harnesses
load `shared/web/index.html`, either directly or through a build-staged copy.

## Build

Build scripts stage outputs into `libs/cua-driver/rust/test-apps/`.

```bash
# macOS: AppKit, SwiftUI, WKWebView, Electron, Tauri
libs/cua-driver/tests/fixtures/build/macos.sh
libs/cua-driver/tests/fixtures/build/macos.sh --skip electron

# Linux: GTK3, Electron, Tauri
libs/cua-driver/tests/fixtures/build/linux.sh
libs/cua-driver/tests/fixtures/build/linux.sh --skip tauri
```

```powershell
# Windows: WPF, WinUI3, WebView2, Electron, Tauri
cd libs\cua-driver\tests\fixtures
.\build\windows.ps1
.\build\windows.ps1 -Skip winui3
```

Host requirements:

- macOS: Xcode command line tools; Node.js/npm for Electron; Rust for Tauri.
- Linux: GTK3/PyGObject/AT-SPI; Node.js/npm for Electron; Rust plus WebKitGTK
  build/runtime deps for Tauri.
- Windows: .NET 8 SDK; Node.js/npm for Electron; Rust for Tauri.

## Test Mapping

Rust tests under `libs/cua-driver/rust/crates/cua-driver/tests/` consume the staged
`rust/test-apps/harness-<name>/` outputs:

- `harness_<toolkit>_test.rs`: toolkit-specific app coverage, usually ignored.
- `modality_<area>[_<os>]_test.rs`: background input, capture, and desktop
  modality coverage, ignored/manual or VM-backed.
- `protocol_*_test.rs` and schema tests: headless protocol coverage, default.

Rust integration tests under `libs/cua-driver/rust/crates/cua-driver/tests/` drive
the shared Electron/Tauri harnesses through the public MCP interface.

## App Coverage

| Harness | Source | Staged output | Primary surface |
| --- | --- | --- | --- |
| AppKit | `apps/macos/appkit` | `harness-appkit` | AX/AppKit controls |
| SwiftUI | `apps/macos/swiftui` | `harness-swiftui` | AX/SwiftUI controls |
| WKWebView | `apps/macos/wkwebview` | `harness-wkwebview` | WebKit AX tree |
| WPF | `apps/windows/wpf` | `harness-wpf` | UIA/WPF controls |
| WinUI3 | `apps/windows/winui3` | `harness-winui3` | UIA/XAML controls |
| WebView2 | `apps/windows/webview2` | `harness-webview` | Chromium web UIA |
| GTK3 | `apps/linux/gtk3` | `harness-gtk3` | AT-SPI/GTK controls |
| Electron | `apps/cross-platform/electron` | `harness-electron` | Chromium web AX/UIA/AT-SPI |
| Tauri | `apps/cross-platform/tauri` | `harness-tauri` | native webview AX/UIA/AT-SPI |

## Maintenance Rules

- Do not commit staged binaries, `node_modules`, copied `web/` folders, or Tauri
  `target/` output.
- Add new shared ids to `shared/scenarios.json` before asserting on them in
  tests.
- Keep platform-specific quirks in the relevant Rust test or app source, not in
  the shared fixture.
- If a recorder finding graduates into an invariant, move it into a Rust test
  and summarize it here.
