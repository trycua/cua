[CmdletBinding()]
param(
    [string]$UninstallerPath = (Join-Path (Split-Path -Parent $PSScriptRoot) "uninstall.ps1")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$tokens = $null
$parseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseFile(
    $UninstallerPath,
    [ref]$tokens,
    [ref]$parseErrors)
if ($parseErrors.Count -ne 0) {
    throw "uninstall.ps1 parse errors: $($parseErrors -join '; ')"
}

$source = Get-Content -LiteralPath $UninstallerPath -Raw
$purgeInvocation = '& $HistoryPurgeHelper history purge-offline --yes'
$runtimeRemoval = '# 3. Visible bin directory junction.'
$purgeIndex = $source.IndexOf($purgeInvocation, [StringComparison]::Ordinal)
$runtimeRemovalIndex = $source.IndexOf($runtimeRemoval, [StringComparison]::Ordinal)

if ($purgeIndex -lt 0) {
    throw "Windows uninstaller does not invoke exact installed-helper history purge"
}
if ($runtimeRemovalIndex -lt 0 -or $purgeIndex -ge $runtimeRemovalIndex) {
    throw "Windows history purge must run before installed runtime removal"
}
if ($source -notmatch 'if \(-not \(Test-Path -LiteralPath \$HistoryPurgeHelper\)\)[\s\S]*?history_purge_incomplete[\s\S]*?exit 1') {
    throw "Windows uninstaller does not fail closed when its exact helper is absent"
}
if ($source -notmatch 'if \(\$historyPurgeExit -ne 0\)[\s\S]*?history_purge_incomplete[\s\S]*?exit 1') {
    throw "Windows uninstaller does not fail closed when native key destruction fails"
}
if ($source -notmatch 'preserved encrypted Computer History') {
    throw "Windows normal-uninstall preservation disclosure is absent"
}

Write-Host "Windows Computer History uninstall ordering checks passed."
