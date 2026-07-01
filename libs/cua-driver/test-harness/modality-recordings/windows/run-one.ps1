param([string]$Mode="ax-bg")
Get-Process cua-driver,CuaTestHarness.Wpf,chrome -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Sleep 2
& powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\wpf-recorder.ps1" -Mode $Mode *> "C:\Users\Public\cua-$Mode-run.log"
