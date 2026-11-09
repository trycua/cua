# Legacy Windows Sandbox runner

This runner exists only for local smoke checks and must not be treated as the
canonical Windows desktop test entrypoint.

Run it from `libs/cua-driver` on a Windows host with Windows Sandbox enabled:

```powershell
.\tests\runners\windows-sandbox\run-tests-in-sandbox.ps1
.\tests\runners\windows-sandbox\run-tests-in-sandbox.ps1 harness_wpf
```

The host script builds selected Rust test binaries and Windows fixtures, maps
`libs/cua-driver` into the sandbox as `C:\cua-driver`, and streams logs from the
inside-sandbox runner.

For canonical GUI validation, dispatch `.github/workflows/e2e-rust-windows.yml`
on the exact candidate SHA. Its GitHub-hosted runner is accepted only when the
strict interactive-desktop preflight passes, then it runs
`scripts/ci/windows/run-rust-e2e.ps1 -RequireGui`. Use Azure RDP only for an
optional environment-parity replay or when the hosted preflight cannot prove a
required capability.
