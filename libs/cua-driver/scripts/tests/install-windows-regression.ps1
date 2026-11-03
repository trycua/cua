[CmdletBinding()]
param(
    [string]$InstallerPath = (Join-Path (Split-Path -Parent $PSScriptRoot) "install.ps1")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

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

Import-InstallerFunction -Ast $ast -Name "Remove-LegacyInstall"

$script:Steps = @()
$script:DaemonStops = 0
$script:ScheduledTaskCalls = @()

function Write-Step { param($Message); $script:Steps += [string]$Message }
function Start-Sleep { [CmdletBinding()] param([int]$Milliseconds, [int]$Seconds) }
function schtasks.exe { $script:ScheduledTaskCalls += ,@($args); $global:LASTEXITCODE = 0 }
# _install-common.psm1 owns daemon cleanup and its ancestor protection;
# install-common-windows-regression.ps1 tests that with real processes.
function Stop-CuaDriverDaemons { $script:DaemonStops++ }

$savedInstallDir = $env:CUA_DRIVER_RS_INSTALL_DIR
$savedHome = $env:CUA_DRIVER_RS_HOME
$env:CUA_DRIVER_RS_INSTALL_DIR = $null
$env:CUA_DRIVER_RS_HOME = $null

try {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-driver-installer-regression-" + [guid]::NewGuid().ToString("N"))
    $HomeDir = Join-Path $root "modern-home"
    $LegacyHomeDir = Join-Path $root "profile\.cua-driver-rs"
    $LegacyVendorDir = Join-Path $root "localappdata\Programs\trycua"
    $LegacyVisibleBinDir = Join-Path $LegacyVendorDir "cua-driver-rs\bin"

    # Current releases write cache and telemetry state here. Neither file is a
    # legacy installation marker, and migration must leave both untouched.
    New-Item -ItemType Directory -Force -Path $LegacyHomeDir | Out-Null
    Set-Content -LiteralPath (Join-Path $LegacyHomeDir "version_check.json") -Value "{}"
    Set-Content -LiteralPath (Join-Path $LegacyHomeDir ".telemetry_id") -Value "synthetic-test-id"
    Remove-LegacyInstall
    Assert-True ($script:Steps.Count -eq 0) "cache/telemetry-only home was detected as legacy"
    Assert-True ($script:DaemonStops -eq 0) "cache/telemetry-only home triggered process cleanup"
    Assert-True (Test-Path -LiteralPath $LegacyHomeDir) "cache/telemetry-only home was removed"

    # Every supported on-disk legacy marker independently enters migration.
    foreach ($marker in @("packages", "bin", "visible-bin")) {
        $scenarioRoot = Join-Path $root $marker
        $HomeDir = Join-Path $scenarioRoot "modern-home"
        $LegacyHomeDir = Join-Path $scenarioRoot "profile\.cua-driver-rs"
        $LegacyVendorDir = Join-Path $scenarioRoot "localappdata\Programs\trycua"
        $LegacyVisibleBinDir = Join-Path $LegacyVendorDir "cua-driver-rs\bin"
        if ($marker -eq "visible-bin") {
            New-Item -ItemType Directory -Force -Path $LegacyVisibleBinDir | Out-Null
        } else {
            New-Item -ItemType Directory -Force -Path (Join-Path $LegacyHomeDir $marker) | Out-Null
        }
        $script:Steps = @()
        $script:DaemonStops = 0
        $script:ScheduledTaskCalls = @()
        Remove-LegacyInstall
        Assert-True ($script:Steps -contains "detected legacy install layout (v0.2.13 or earlier); migrating to Cua\cua-driver") `
            "$marker was not detected as a legacy marker"
        Assert-True ($script:DaemonStops -eq 1) "$marker migration did not stop daemons through the shared cleanup"
    }

    Write-Host "Windows installer legacy migration checks passed."
}
finally {
    $env:CUA_DRIVER_RS_INSTALL_DIR = $savedInstallDir
    $env:CUA_DRIVER_RS_HOME = $savedHome
}

$rustWorkspace = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) "rust"
Push-Location $rustWorkspace
try {
    & cargo test -p cua-driver --bin cua-driver 'autostart::tests::' --locked -- --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Windows autostart path tests failed with exit $LASTEXITCODE"
    }

    & cargo build -p cua-driver --bin cua-driver --locked
    if ($LASTEXITCODE -ne 0) {
        throw "Windows autostart task probe build failed with exit $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

# Exercise the installed topology and Task Scheduler end to end. Register the
# task through bin -> current -> probe-v1, retarget current to probe-v2, then
# prove the unchanged action reaches the new executable.
$taskName = "cua-driver-serve"
$taskProbeRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-driver-autostart-task-" + [guid]::NewGuid().ToString("N"))
$taskPackages = Join-Path $taskProbeRoot "packages"
$taskReleaseV1 = Join-Path $taskPackages "releases\probe-v1"
$taskReleaseV2 = Join-Path $taskPackages "releases\probe-v2"
$taskCurrent = Join-Path $taskPackages "current"
$taskVisible = Join-Path $taskProbeRoot "bin"
$taskVisibleExe = Join-Path $taskVisible "cua-driver.exe"
$taskMarker = Join-Path $taskProbeRoot "probe-v2.txt"
$realSchtasks = Join-Path $env:SystemRoot "System32\schtasks.exe"

try {
    New-Item -ItemType Directory -Force -Path $taskReleaseV1, $taskReleaseV2 | Out-Null
    Copy-Item -LiteralPath (Join-Path $rustWorkspace "target\debug\cua-driver.exe") `
        -Destination (Join-Path $taskReleaseV1 "cua-driver.exe")

    $markerLiteral = $taskMarker.Replace('\', '\\').Replace('"', '\"')
    $probeSourcePath = Join-Path $taskProbeRoot "probe-v2.rs"
    $probeSource = @"
fn main() {
    std::fs::write("$markerLiteral", "probe-v2").unwrap();
}
"@
    Set-Content -LiteralPath $probeSourcePath -Value $probeSource -Encoding UTF8
    & rustc $probeSourcePath -o (Join-Path $taskReleaseV2 "cua-driver.exe")
    if ($LASTEXITCODE -ne 0) {
        throw "probe-v2 build failed with exit $LASTEXITCODE"
    }

    New-Item -ItemType Junction -Path $taskCurrent -Target $taskReleaseV1 | Out-Null
    New-Item -ItemType Junction -Path $taskVisible -Target $taskCurrent | Out-Null

    & $taskVisibleExe autostart enable
    if ($LASTEXITCODE -ne 0) {
        throw "autostart task registration failed with exit $LASTEXITCODE"
    }
    $beforeAction = (Get-ScheduledTask -TaskName $taskName -ErrorAction Stop).Actions | Select-Object -First 1
    Assert-True ($beforeAction.Arguments -like "*$taskVisibleExe*") `
        "Scheduled Task action did not retain the visible junction path: $($beforeAction.Arguments)"

    [System.IO.Directory]::Delete($taskCurrent, $false)
    New-Item -ItemType Junction -Path $taskCurrent -Target $taskReleaseV2 | Out-Null
    $afterAction = (Get-ScheduledTask -TaskName $taskName -ErrorAction Stop).Actions | Select-Object -First 1
    Assert-True ($afterAction.Arguments -eq $beforeAction.Arguments) `
        "Scheduled Task action changed when the current junction was retargeted"

    & $realSchtasks /Run /TN $taskName | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "retargeted autostart task failed to start with exit $LASTEXITCODE"
    }
    for ($attempt = 0; $attempt -lt 100 -and -not (Test-Path -LiteralPath $taskMarker); $attempt++) {
        [System.Threading.Thread]::Sleep(100)
    }
    Assert-True (Test-Path -LiteralPath $taskMarker) "retargeted task did not launch probe-v2"
    Assert-True ((Get-Content -LiteralPath $taskMarker -Raw) -eq "probe-v2") `
        "retargeted task did not report the next probe version"

    Write-Host "Windows Scheduled Task junction-retarget regression check passed."
}
finally {
    & $realSchtasks /End /TN $taskName 2>$null | Out-Null
    & $realSchtasks /Delete /TN $taskName /F 2>$null | Out-Null
}
