param([string]$CaseName)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.ToString() -notlike '5.1.*') { throw 'Windows PowerShell 5.1 required' }
$FixtureRoot = $env:CUA_UNINSTALL_FIXTURE_ROOT
$DefaultCli = 'localappdata\Programs\Cua\cua-driver-local\bin\cua-driver-local.exe'
$Cases = [ordered]@{
    'default-cli' = @{ Local = $DefaultCli }
    'configured-cli' = @{ Local = 'custom bin\cua-driver-local.exe'; Env = 'CUA_DRIVER_LOCAL_INSTALL_DIR'; Value = 'custom bin' }
    'default-marker' = @{ Local = 'profile\.cua-driver-local\packages\current\cua-driver-local.exe' }
    'configured-marker' = @{ Local = 'custom home\packages\current\cua-driver-local.exe'; Env = 'CUA_DRIVER_LOCAL_HOME'; Value = 'custom home' }
    'path-cli' = @{ Local = 'bin\cua-driver-local.exe' }
    'marker-free-home' = @{}
    'file-entrypoint' = @{ Local = $DefaultCli }
    'guard-remove' = @{ Command = "Remove-Item -LiteralPath '..\outside.txt' -Force" }
    'guard-stop' = @{ Command = 'Stop-Process -Id $PID -Force' }
    'guard-start' = @{ Command = "Start-Process -FilePath '.\missing.exe'" }
    'guard-discovery' = @{ Command = 'Get-Process -Name fixture-not-running' }
    'guard-task' = @{ Command = 'schtasks.exe /Delete /TN fixture-nonexistent /F' }
}
function Assert-Fixture([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
if ($CaseName -and -not $Cases.Contains($CaseName)) { throw "unknown case: $CaseName" }
if (-not $FixtureRoot) {
    $RunRoot = Join-Path ([IO.Path]::GetTempPath()) ('cua uninstall ' + [guid]::NewGuid().ToString('N'))
    [void][IO.Directory]::CreateDirectory($RunRoot)
    $Outside = Join-Path $RunRoot 'outside.txt'
    [IO.File]::WriteAllText($Outside, 'keep')
    $Names = @($Cases.Keys)
    if ($CaseName) { $Names = @($CaseName) }
    try {
        foreach ($Name in $Names) {
            $Root = Join-Path $RunRoot $Name
            [void][IO.Directory]::CreateDirectory($Root)
            $Arguments = "-NoLogo -NoProfile -NonInteractive -File `"$PSCommandPath`" -CaseName $Name"
            $Start = [Diagnostics.ProcessStartInfo]::new("$PSHOME\powershell.exe", $Arguments)
            $Start.UseShellExecute = $false
            $Start.WorkingDirectory = $Root
            $Start.RedirectStandardOutput = $true
            $Start.RedirectStandardError = $true
            $Start.RedirectStandardInput = $true
            $Start.EnvironmentVariables.Clear()
            $Start.EnvironmentVariables['SystemRoot'] = $env:SystemRoot
            $Start.EnvironmentVariables['CUA_UNINSTALL_FIXTURE_ROOT'] = $Root
            $Start.EnvironmentVariables['CUA_DRIVER_RS_UNINSTALL_FORCE'] = '1'
            $Start.EnvironmentVariables['PATHEXT'] = '.EXE'
            $Directories = @{
                USERPROFILE = 'profile'; LOCALAPPDATA = 'localappdata'; APPDATA = 'appdata'
                TEMP = 'temp'; TMP = 'temp'; PATH = 'bin'; PSModulePath = 'modules'
            }
            foreach ($Entry in $Directories.GetEnumerator()) {
                $Value = Join-Path $Root $Entry.Value
                [void][IO.Directory]::CreateDirectory($Value)
                $Start.EnvironmentVariables[$Entry.Key] = $Value
            }
            if ($Cases[$Name]['Env']) {
                $Start.EnvironmentVariables[$Cases[$Name]['Env']] = Join-Path $Root $Cases[$Name]['Value']
            }
            $Child = [Diagnostics.Process]::Start($Start)
            try {
                $Child.StandardInput.Close()
                $Stdout = $Child.StandardOutput.ReadToEndAsync()
                $Stderr = $Child.StandardError.ReadToEndAsync()
                if (-not $Child.WaitForExit(30000)) {
                    $Child.Kill()
                    $Child.WaitForExit()
                    throw "$Name timed out"
                }
                $Output = $Stdout.GetAwaiter().GetResult() + $Stderr.GetAwaiter().GetResult()
                if ($Cases[$Name]['Command']) {
                    Assert-Fixture ($Child.ExitCode -ne 0 -and $Output.Contains('fixture refused host operation')) "$Name did not refuse: $Output"
                } else {
                    Assert-Fixture ($Child.ExitCode -eq 0) "$Name failed: $Output"
                }
                Assert-Fixture ([IO.File]::ReadAllText($Outside) -eq 'keep') 'outside artifact changed'
                Write-Host "PASS $Name"
            } finally { $Child.Dispose() }
        }
        Write-Host "$($Names.Count) Windows uninstall cases passed"
    } finally { Microsoft.PowerShell.Management\Remove-Item -LiteralPath $RunRoot -Recurse -Force }
    return
}

$FixtureCase = $Cases[$CaseName]
$FixtureRoot = [IO.Path]::GetFullPath($FixtureRoot).TrimEnd('\') + '\'
$FixtureViolations = [Collections.Generic.List[string]]::new()
function Deny-HostOperation([string]$Operation) {
    $FixtureViolations.Add($Operation)
    throw "fixture refused host operation: $Operation"
}
function Get-Process {
    [CmdletBinding()] param([string[]]$Name)
    if (($Name -join ' ') -ne 'cua-driver') { Deny-HostOperation "Get-Process $Name" }
    return @()
}
function schtasks.exe {
    if (($args -join ' ') -ne '/Query /TN cua-driver-serve') { Deny-HostOperation "schtasks.exe $args" }
    $global:LASTEXITCODE = 1
}
function Stop-Process { Deny-HostOperation 'Stop-Process' }
function Start-Process { Deny-HostOperation 'Start-Process' }
function Read-Host { Deny-HostOperation 'Read-Host' }
function Remove-Item {
    [CmdletBinding()] param([string]$LiteralPath, [switch]$Force, [switch]$Recurse)
    $Resolved = [IO.Path]::GetFullPath($LiteralPath)
    if (-not $Resolved.StartsWith($FixtureRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Deny-HostOperation "Remove-Item $LiteralPath"
    }
    Microsoft.PowerShell.Management\Remove-Item @PSBoundParameters
}
if ($FixtureCase['Command']) {
    try { Invoke-Expression $FixtureCase['Command'] } catch {}
} else {
    $FixtureRelease = Join-Path $FixtureRoot 'profile\.cua-driver\packages\current\cua-driver.exe'
    $FixtureHome = if ($FixtureCase['Env'] -eq 'CUA_DRIVER_LOCAL_HOME') { $FixtureCase['Value'] } else { 'profile\.cua-driver-local' }
    $FixtureFiles = @((Join-Path $FixtureRoot "$FixtureHome\config.json"))
    if ($FixtureCase['Local']) { $FixtureFiles += Join-Path $FixtureRoot $FixtureCase['Local'] }
    foreach ($Path in @($FixtureRelease) + $FixtureFiles) {
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path))
        [IO.File]::WriteAllText($Path, 'fixture payload')
    }
    $Uninstaller = Join-Path (Split-Path -Parent $PSScriptRoot) 'uninstall.ps1'
    if ($CaseName -eq 'file-entrypoint') {
        $FixtureOutput = & $Uninstaller 6>&1
        $FixtureSucceeded = $?
    } else {
        $FixtureOutput = Invoke-Expression ([Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($Uninstaller))) 6>&1
        $FixtureSucceeded = $?
    }
    Assert-Fixture $FixtureSucceeded 'uninstaller failed'
    $FixtureOutput = ($FixtureOutput | ForEach-Object { $_.ToString() }) -join "`n"
    Assert-Fixture (-not (Test-Path -LiteralPath $FixtureRelease)) 'release payload remains'
    foreach ($Path in $FixtureFiles) {
        Assert-Fixture ((Test-Path -LiteralPath $Path) -and [IO.File]::ReadAllText($Path) -eq 'fixture payload') "local artifact changed: $Path"
    }
    $Notice = 'source-built cua-driver-local installation remains'
    if ($FixtureCase['Local']) {
        Assert-Fixture ($FixtureOutput.Contains($Notice)) 'missing survivor notice'
        Assert-Fixture ($FixtureOutput.Contains((Join-Path $FixtureRoot $FixtureCase['Local']))) "missing survivor path: $FixtureOutput"
        Assert-Fixture ($FixtureOutput.Contains('.\libs\cua-driver\scripts\uninstall-local.ps1')) 'missing removal instruction'
    } else {
        Assert-Fixture (-not $FixtureOutput.Contains($Notice)) 'false survivor notice'
    }
}
Assert-Fixture ($FixtureViolations.Count -eq 0) "fixture refused host operations: $($FixtureViolations -join ', ')"
