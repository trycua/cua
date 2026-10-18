# Last step of the image build: remove build leftovers (the downloaded build
# scripts, any token a test wrote) and power the guest off. build-image.sh
# waits for QEMU to exit before it writes disk.img.
$ErrorActionPreference = "Continue"
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $env:TEMP "cua-build-*")
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $env:ProgramData "cua\spacesd\token")
& shutdown.exe /s /t 5 /f /d p:4:1 /c "cua image build"
Write-Output "shutdown scheduled"
exit 0
