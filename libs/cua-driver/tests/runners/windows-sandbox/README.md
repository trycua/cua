# Windows Sandbox Runner

Legacy Windows Sandbox runner for local smoke checks. It predates the Azure/RDP
GUI validation flow and should not be treated as the canonical Windows desktop
test entrypoint.

Run it from `libs/cua-driver` on a Windows host with Windows Sandbox enabled:

```powershell
.\tests\runners\windows-sandbox\run-tests-in-sandbox.ps1
.\tests\runners\windows-sandbox\run-tests-in-sandbox.ps1 harness_wpf
```

The host script builds selected Rust test binaries and Windows fixtures, maps
`libs/cua-driver` into the sandbox as `C:\cua-driver`, and streams logs from the
inside-sandbox runner.

For current GUI validation, prefer a real user desktop session such as the
Azure RDP/scheduled-task runner described in the Rust test README.
