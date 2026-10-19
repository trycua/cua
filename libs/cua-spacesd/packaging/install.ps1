# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Installs cua-spacesd on Windows and registers the interactive-session
# scheduled task (packaging/windows/install-scheduled-task.ps1).
#
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Binary .\cua-spacesd.exe -Token <token>
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Version 0.1.0 -Token <token>
param(
  [string]$Binary,
  [string]$Version,
  [string]$Token,
  [string]$Repo = "trycua/cua",
  [string]$InstallDir = "$env:LOCALAPPDATA\cua\spacesd",
  [switch]$NoService
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$exe = Join-Path $InstallDir "cua-spacesd.exe"
if ($Binary) {
  Copy-Item -Force $Binary $exe
} elseif ($Version) {
  $arch = if ([Environment]::Is64BitOperatingSystem -and $env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
  $url = "https://github.com/$Repo/releases/download/cua-spacesd-v$Version/cua-spacesd-$arch-pc-windows-msvc.exe"
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
  $download = Join-Path $InstallDir "cua-spacesd.exe.download"
  Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $download
  # The published <asset>.sha256 must match before the binary is installed.
  $sumFile = "$download.sha256"
  Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile $sumFile
  $expected = ((Get-Content -Raw -LiteralPath $sumFile).Trim() -split '\s+')[0].ToLowerInvariant()
  Remove-Item -Force -LiteralPath $sumFile
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLowerInvariant()
  if (-not $expected -or $expected -ne $actual) {
    Remove-Item -Force -LiteralPath $download
    throw "checksum mismatch for $url (expected $expected, got $actual)"
  }
  Move-Item -Force -LiteralPath $download -Destination $exe
} else {
  throw "pass -Binary PATH or -Version VERSION"
}
$tokenDir = Join-Path $env:USERPROFILE ".cua\spacesd"
# Older install (cua-guestd, or cua-env-driver before it): take over its
# token directory.
foreach ($old in @("guestd", "env-driver")) {
  $oldTokenDir = Join-Path $env:USERPROFILE ".cua\$old"
  if ((Test-Path $oldTokenDir) -and -not (Test-Path $tokenDir)) { Move-Item $oldTokenDir $tokenDir }
}
New-Item -ItemType Directory -Force -Path $tokenDir | Out-Null
$tokenFile = Join-Path $tokenDir "token"
if ($Token) {
  Set-Content -NoNewline -Path $tokenFile -Value $Token
  # Owner-only access.
  icacls $tokenFile /inheritance:r /grant:r "$($env:USERNAME):(R,W)" | Out-Null
}
if (-not $NoService) {
  & (Join-Path $PSScriptRoot "windows\install-scheduled-task.ps1") -Exe $exe -TokenFile $tokenFile
}
Write-Host "cua-spacesd installed to $exe"
