# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [switch]$RunTests
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $root "target"
}
$cargo = if (Get-Command cargo -ErrorAction SilentlyContinue) {
    "cargo"
} else {
    Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
}
$profileArgs = if ($Configuration -eq "release") { @("--release") } else { @() }
$binaryRoot = Join-Path $targetRoot $Configuration
$packageRoot = Join-Path $targetRoot "windows\RCDP"

Push-Location $root
try {
    if ($RunTests) {
        & $cargo test --workspace
        if ($LASTEXITCODE -ne 0) { throw "cargo test failed" }
    }

    & $cargo build --workspace @profileArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    # The viewer is a client and lives in the libs/cua workspace.
    $cuaRoot = Join-Path (Split-Path -Parent $root) "cua"
    $cuaTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $cuaRoot "target" }
    & $cargo build --manifest-path (Join-Path $cuaRoot "Cargo.toml") -p cua-viewer @profileArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build (cua-viewer) failed" }

    if (Test-Path $packageRoot) {
        Remove-Item -Recurse -Force $packageRoot
    }
    New-Item -ItemType Directory -Force $packageRoot | Out-Null
    foreach ($binary in @("cua-spacesd.exe", "cua-spacesd-test-pad.exe")) {
        Copy-Item -Force (Join-Path $binaryRoot $binary) $packageRoot
    }
    Copy-Item -Force (Join-Path (Join-Path $cuaTarget $Configuration) "cua-viewer.exe") $packageRoot

    Copy-Item -Force "LICENSE" $packageRoot
    Copy-Item -Force "THIRD_PARTY_NOTICES.md" $packageRoot
    Copy-Item -Force "README.md" $packageRoot
    Copy-Item -Force "docs\native-app-client.md" $packageRoot
    Copy-Item -Force "packaging\windows\apps.example.json" $packageRoot

    Write-Output $packageRoot
} finally {
    Pop-Location
}
