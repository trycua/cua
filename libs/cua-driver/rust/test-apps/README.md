# test-apps

Local staging directory for cua-driver test fixture applications.

Build scripts under `../tests/fixtures/build/` write `harness-<name>/`
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
../../tests/fixtures/build/macos.sh
../../tests/fixtures/build/linux.sh
```

```powershell
..\..\tests\fixtures\build\windows.ps1
```
