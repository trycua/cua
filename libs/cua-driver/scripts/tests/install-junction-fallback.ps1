# Windows PowerShell 5.1 regression test for the install.ps1 junction fallback.
# Run from the repository root with powershell.exe -File.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($PSVersionTable.PSEdition -ne "Desktop" -or $PSVersionTable.PSVersion.Major -ne 5) {
    throw "this test must run under Windows PowerShell 5.1"
}

$installer = Join-Path $PSScriptRoot "..\install.ps1"
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $installer,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors.Count -ne 0) {
    throw "install.ps1 parse failed: $($parseErrors -join '; ')"
}

$wanted = @("Test-IsJunction", "Get-JunctionTarget", "Set-JunctionTarget")
foreach ($name in $wanted) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $name
    }, $true)
    if (-not $definition) { throw "function $name not found in install.ps1" }
    . ([scriptblock]::Create($definition.Extent.Text))
}

function Add-JunctionSupportType {}
$script:JunctionTypeUnavailable = $true
$script:MklinkCalls = 0
function Invoke-MklinkJunction([string]$linkPath, [string]$targetPath) {
    $script:MklinkCalls++
    if ($script:MklinkCalls -eq 1) {
        throw "simulated replacement mklink failure"
    }
    $output = & $env:ComSpec /d /c "mklink /J `"$linkPath`" `"$targetPath`"" 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "restore mklink failed (exit $LASTEXITCODE): $($output -join ' ')"
    }
}

$oldCodePage = (& $env:ComSpec /d /c chcp) -replace '[^0-9]', ''
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-安装-junction-" + [guid]::NewGuid().ToString("N"))
$oldTarget = Join-Path $root "旧版本"
$newTarget = Join-Path $root "新版本"
$link = Join-Path $root "current"

try {
    & $env:ComSpec /d /c "chcp 936 >nul"
    if ($LASTEXITCODE -ne 0) { throw "could not activate code page 936" }

    New-Item -ItemType Directory -Force -Path $oldTarget, $newTarget | Out-Null
    $output = & $env:ComSpec /d /c "mklink /J `"$link`" `"$oldTarget`"" 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "fixture junction creation failed (exit $LASTEXITCODE): $($output -join ' ')"
    }

    $message = $null
    try {
        Set-JunctionTarget $link $newTarget
        throw "Set-JunctionTarget unexpectedly succeeded"
    } catch {
        $message = $_.Exception.Message
    }

    if ($message -notlike "*simulated replacement mklink failure*restored previous target*") {
        throw "replacement failure did not report restoration: $message"
    }
    if (-not (Test-IsJunction $link)) {
        throw "working junction was lost after replacement failure"
    }
    $actual = Get-JunctionTarget $link
    if ([System.IO.Path]::GetFullPath($actual) -ne [System.IO.Path]::GetFullPath($oldTarget)) {
        throw "junction target changed after replacement failure: expected '$oldTarget', got '$actual'"
    }
    if ($script:MklinkCalls -ne 2) {
        throw "expected one failed replacement and one restoration, got $script:MklinkCalls calls"
    }

    Write-Host "PASS: PowerShell 5.1 code-page-936 fallback restores the prior junction"
} finally {
    if (Test-Path -LiteralPath $link) {
        [System.IO.Directory]::Delete($link, $false)
    }
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
    if ($oldCodePage) {
        & $env:ComSpec /d /c "chcp $oldCodePage >nul"
    }
}
