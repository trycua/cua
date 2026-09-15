param(
    [Parameter(Mandatory = $true)][string]$UninstallerPath,
    [Parameter(Mandatory = $true)][string]$FixtureRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
Set-PSDebug -Trace 1
$FixtureRoot = [System.IO.Path]::GetFullPath($FixtureRoot).TrimEnd('\') + '\'
$global:CuaUninstallFixtureViolations = [System.Collections.Generic.List[string]]::new()

function Deny-HostOperation([string]$Operation) {
    $global:CuaUninstallFixtureViolations.Add($Operation)
    throw "fixture refused host operation: $Operation"
}

function Get-Process {
    [CmdletBinding()]
    param([string[]]$Name)
    if (($Name -join ' ') -ne 'cua-driver') {
        Deny-HostOperation "Get-Process $Name"
    }
    return @()
}

function schtasks.exe {
    if (($args -join ' ') -ne '/Query /TN cua-driver-serve') {
        Deny-HostOperation "schtasks.exe $args"
    }
    $global:LASTEXITCODE = 1
}

function Stop-Process { Deny-HostOperation "Stop-Process" }
function Start-Process { Deny-HostOperation "Start-Process" }
function Read-Host { Deny-HostOperation "Read-Host" }

function Remove-Item {
    [CmdletBinding()]
    param([string]$LiteralPath, [switch]$Force, [switch]$Recurse)
    $resolved = [System.IO.Path]::GetFullPath($LiteralPath)
    if (-not $resolved.StartsWith($FixtureRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Deny-HostOperation "Remove-Item $LiteralPath"
    }
    Microsoft.PowerShell.Management\Remove-Item @PSBoundParameters
}

& $UninstallerPath
if (-not $?) { throw "uninstaller failed" }
if ($global:CuaUninstallFixtureViolations.Count) {
    throw "fixture refused host operations: $($global:CuaUninstallFixtureViolations -join ', ')"
}
