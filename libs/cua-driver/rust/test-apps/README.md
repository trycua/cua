# test-apps

Local staging directory for cua-driver test harness applications.

Build scripts under `../test-harness/build/` write `harness-<name>/`
directories here. The staged outputs are intentionally git-ignored; rebuild
them from source instead of committing binaries.

Common outputs:

- `harness-wpf/`
- `harness-winui3/`
- `harness-webview/`
- `harness-electron/`
- `harness-tauri/`
- `harness-appkit/`
- `harness-swiftui/`
- `harness-wkwebview/`
- `harness-gtk3/`

Run the host build script for your platform:

```bash
../../test-harness/build/macos.sh
../../test-harness/build/linux.sh
```

```powershell
..\..\test-harness\build\windows.ps1
```
