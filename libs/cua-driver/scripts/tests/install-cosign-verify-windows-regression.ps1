[CmdletBinding()]
param(
    [string]$InstallerPath
)

# Regression for the Sigstore check in install.ps1 under Windows PowerShell 5.1.
#
# cosign prints "Verified OK" on stderr. install.ps1 runs with
# $ErrorActionPreference = "Stop", and Windows PowerShell 5.1 turns a native
# command's redirected stderr into error records, so a successful verification
# used to abort the install with NativeCommandError. Run this file with
# Windows PowerShell (powershell.exe), where the failure reproduces; pwsh 7
# does not wrap native stderr this way.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Resolved here rather than as a param default: Windows PowerShell 5.1 leaves
# $PSScriptRoot empty while it evaluates param defaults.
if (-not $InstallerPath) {
    $InstallerPath = Join-Path (Split-Path -Parent $PSScriptRoot) "install.ps1"
}

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) { throw $Message }
}

function Import-InstallerFunction {
    param(
        [Parameter(Mandatory = $true)]$Ast,
        [Parameter(Mandatory = $true)][string]$Name
    )
    $definition = $Ast.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $Name
    }, $true) | Select-Object -First 1
    if (-not $definition) { throw "function $Name not found in $InstallerPath" }
    $scriptDefinition = $definition.Extent.Text -replace "^function\s+$([regex]::Escape($Name))", "function script:$Name"
    . ([scriptblock]::Create($scriptDefinition))
}

$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $InstallerPath,
    [ref]$tokens,
    [ref]$parseErrors)
if ($parseErrors.Count -ne 0) {
    throw "install.ps1 parse errors: $($parseErrors -join '; ')"
}

# Every verify-blob call must go through the helper, so the fix cannot be
# bypassed by a second call site.
$verifyCalls = @($ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.Extent.Text -match '\bverify-blob\b'
}, $true))
Assert-True ($verifyCalls.Count -eq 1) "expected exactly one verify-blob call in install.ps1, found $($verifyCalls.Count)"
$owner = $verifyCalls[0].Parent
while ($owner -and -not ($owner -is [System.Management.Automation.Language.FunctionDefinitionAst])) {
    $owner = $owner.Parent
}
Assert-True ($null -ne $owner -and $owner.Name -eq "Invoke-CosignVerifyBlob") `
    "the verify-blob call must live in Invoke-CosignVerifyBlob"

Import-InstallerFunction -Ast $ast -Name "Invoke-CosignVerifyBlob"
$CosignOidcIssuer = "https://token.actions.githubusercontent.com"

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-cosign-regression-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $okCosign = Join-Path $root "cosign-ok.cmd"
    $badCosign = Join-Path $root "cosign-bad.cmd"
    # Mirror real cosign: the success line goes to stderr, not stdout.
    Set-Content -LiteralPath $okCosign -Encoding ASCII -Value @(
        "@echo off",
        "echo Verified OK 1>&2",
        "exit /b 0")
    Set-Content -LiteralPath $badCosign -Encoding ASCII -Value @(
        "@echo off",
        "echo Error: none of the expected identities matched 1>&2",
        "exit /b 1")

    $bundle = Join-Path $root "release.zip.sigstore.json"
    $zip = Join-Path $root "release.zip"
    $identity = "https://github.com/trycua/cua/.github/workflows/cd-rust-cua-driver.yml@refs/tags/cua-driver-rs-v0.0.0"

    $exitCode = $null
    try {
        $exitCode = Invoke-CosignVerifyBlob $okCosign $bundle $identity $zip
    }
    catch {
        throw "a successful cosign verification raised '$($_.Exception.Message)' under ErrorActionPreference=Stop"
    }
    Assert-True ($exitCode -eq 0) "successful verification returned exit code '$exitCode' instead of 0"
    Assert-True ($ErrorActionPreference -eq "Stop") "Invoke-CosignVerifyBlob leaked its ErrorActionPreference to the caller"

    $exitCode = Invoke-CosignVerifyBlob $badCosign $bundle $identity $zip
    Assert-True ($exitCode -ne 0) "failed verification returned exit code 0"
}
finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output "install.ps1 cosign verification regression passed ($($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion))"
