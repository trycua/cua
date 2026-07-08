# test-harness build scripts

Run `macos.sh` or `windows.ps1` to build every harness app for that host
OS. Outputs are staged into `../../rust/test-apps/harness-<name>/` so
the Rust integration tests and the Windows Sandbox runner pick them up
unchanged.

```bash
# macOS — builds appkit + swiftui via xcrun swiftc into .app bundles
./macos.sh

# Windows — dotnet publish for wpf/winui3/webview2 + Electron dist staging
.\windows.ps1
```

Both scripts accept a `--skip` / `-Skip` argument to build one app at a
time during local iteration; see the file headers for the full surface.

Cross-platform Electron is built by `windows.ps1` today (which runs
`apps/cross-platform/electron/build.ps1`). When a non-Windows test path
needs the Electron host, lift the staging step into `macos.sh` (or
`linux.sh`).
