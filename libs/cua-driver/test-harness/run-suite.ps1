# Run the full cua-driver interactive #[ignore] modality/harness matrix on
# Windows in one shot, with a PASS/FAIL/SKIP summary. Windows peer of run-suite.sh.
#
# Each test file is `#![cfg(target_os = "windows")]`-gated; the matrix below is
# the Windows subset. Non-Windows files compile to empty binaries if listed, so
# only the Windows-relevant ones are run here.
#
# GUI/UIA tests need an INTERACTIVE desktop session (not SSH/Session 0). On a VM,
# drive this from Session 2 via a scheduled task with an interactive token.
#
# Usage:
#   pwsh -File run-suite.ps1                 # build harness, run the matrix
#   pwsh -File run-suite.ps1 -NoBuild        # reuse the staged harness
#   pwsh -File run-suite.ps1 -Release        # use the release driver binary
param(
  [switch]$NoBuild,
  [switch]$Release
)
$ErrorActionPreference = "Continue"

$HarnessDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RustDir = Resolve-Path (Join-Path $HarnessDir "..\rust")

$Tests = @(
  "harness_wpf_test", "harness_winui3_test", "harness_web_test", "harness_libreoffice_test",
  "modality_capture_mode_test", "modality_background_test", "modality_input_e2e_test",
  "modality_desktop_scope_test", "guard_ux_test"
)

Write-Host "==> cua-driver interactive matrix — host: windows$(if($Release){' (release)'})"

if (-not $NoBuild) {
  Write-Host "==> building Windows harness…"
  & powershell -File (Join-Path $HarnessDir "build\windows.ps1")
}

Push-Location $RustDir
$profileArg = if ($Release) { "--release" } else { "" }
$summary = @()
$overall = 0
foreach ($t in $Tests) {
  $out = & cargo test $profileArg -p cua-driver --test $t -- --ignored --nocapture --test-threads=1 2>&1 | Out-String
  Write-Host $out
  $line = ($out -split "`n" | Select-String '^test result:' | Select-Object -Last 1)
  if (-not $line) {
    $summary += "SKIP  $t (no host tests / did not build)"
  } else {
    $passed = if ($line -match '(\d+) passed') { [int]$Matches[1] } else { 0 }
    $failed = if ($line -match '(\d+) failed') { [int]$Matches[1] } else { 0 }
    if ($failed -gt 0) { $summary += "FAIL  $t ($passed passed, $failed failed)"; $overall = 1 }
    elseif ($passed -eq 0) { $summary += "SKIP  $t (0 tests)" }
    else { $summary += "PASS  $t ($passed passed)" }
  }
}
Pop-Location

Write-Host ""
Write-Host "================ cua-driver matrix summary (windows) ================"
$summary | ForEach-Object { Write-Host $_ }
Write-Host "===================================================================="
exit $overall
