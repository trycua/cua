# Windows Rust runner

This directory keeps a local convenience wrapper for an interactive user
desktop. From the repository root, the canonical command is:

```powershell
.\scripts\ci\windows\run-rust-e2e.ps1 -RequireGui
```

Maintainer certification normally dispatches
`.github/workflows/e2e-rust-windows.yml` on the exact candidate SHA. The
GitHub-hosted runner is canonical when its strict desktop preflight passes.

Run from `libs/cua-driver` in an RDP or console session:

```powershell
.\tests\runners\windows\run-all.ps1
.\tests\runners\windows\run-all.ps1 -RequireGui
```

The convenience wrapper delegates to the same canonical runner, builds
repo-local Windows fixtures, and runs the Rust unit and typed harness matrix.
It intentionally skips optional external-app suites such as LibreOffice because
those require extra software on the environment image.
