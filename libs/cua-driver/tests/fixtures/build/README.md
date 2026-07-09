# test fixture build scripts

These host-specific scripts stage source-built harness apps into
`libs/cua-driver/rust/test-apps/harness-<name>/`.

| Script | Host | Builds |
| --- | --- | --- |
| `macos.sh` | macOS | AppKit, SwiftUI, WKWebView, Electron, Tauri |
| `linux.sh` | Linux | GTK3, Electron, Tauri |
| `windows.ps1` | Windows | WPF, WinUI3, WebView2, Electron, Tauri |

Each script supports skipping one target while iterating locally:

```bash
./macos.sh --skip electron
./linux.sh --skip tauri
```

```powershell
.\windows.ps1 -Skip webview
```

Build products are local artifacts. Rebuild them instead of committing them.
