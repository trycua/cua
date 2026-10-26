# Last step of the image build: remove build leftovers (the downloaded build
# scripts, any token a test wrote) and every per-machine secret cua-spacesd
# keeps, verify they are gone, and power the guest off. build-image.sh waits
# for QEMU to exit before it writes disk.img, and fails the build unless this
# script printed "seal verified".
#
# cua-spacesd's state lives in its user's profile (%USERPROFILE%\.cua\spacesd:
# machine-seal.key, the relay machine id, mesh keys, the access log, the
# teleport ledger) and its token in %ProgramData%\cua\spacesd. Every clone
# must make its own, so none may ship. The driver is stopped first so it
# cannot write any of them again before the power-off.
$ErrorActionPreference = "Continue"
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $env:TEMP "cua-build-*")

Stop-ScheduledTask -TaskName "cua-spacesd" -ErrorAction SilentlyContinue
Get-Process -Name "cua-spacesd" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

# Per-user driver state, for every profile (the desktop user's, and the
# system profile's should the driver ever have run as SYSTEM).
$profiles = @(Get-ChildItem -Directory -Force -ErrorAction SilentlyContinue "$env:SystemDrive\Users" | ForEach-Object { $_.FullName })
$profiles += Join-Path $env:SystemRoot "System32\config\systemprofile"
$stateDirs = foreach ($p in $profiles) {
  foreach ($name in @("spacesd", "env-driver", "guestd")) { Join-Path $p ".cua\$name" }
}
foreach ($dir in $stateDirs) {
  if (Test-Path -LiteralPath $dir) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -LiteralPath $dir
    Write-Output "removed $dir"
  }
}
# The token directory keeps its ACL; only its contents go.
$tokenDir = Join-Path $env:ProgramData "cua\spacesd"
Get-ChildItem -Force -ErrorAction SilentlyContinue -LiteralPath $tokenDir | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

$left = @()
foreach ($dir in $stateDirs) { if (Test-Path -LiteralPath $dir) { $left += $dir } }
$left += @(Get-ChildItem -Force -ErrorAction SilentlyContinue -LiteralPath $tokenDir | ForEach-Object { $_.FullName })
$left += @(Get-ChildItem -Recurse -Force -ErrorAction SilentlyContinue -Filter "machine-seal.key" -Path ($profiles + $env:ProgramData) |
  ForEach-Object { $_.FullName })
if ($left.Count -gt 0) {
  Write-Output "STILL PRESENT (not sealing, not shutting down):"
  $left | Sort-Object -Unique | ForEach-Object { Write-Output "  $_" }
  exit 1
}
Write-Output "seal verified: no cua-spacesd token, machine-seal key or per-machine state"
& shutdown.exe /s /t 5 /f /d p:4:1 /c "cua image build"
Write-Output "shutdown scheduled"
exit 0
