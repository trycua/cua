# Windows Rust Runner

Canonical Windows Rust harness runner for an interactive user desktop.

Run from `libs/cua-driver` in an RDP or console session:

```powershell
.\tests\runners\windows\run-all.ps1
.\tests\runners\windows\run-all.ps1 -RequireGui
```

The runner builds repo-local Windows fixtures and runs the Rust default,
guard, harness, and modality suites. It intentionally skips optional
external-app suites such as LibreOffice because those require extra software
on the VM image.
