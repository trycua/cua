param([ValidateRange(1, 20)][int]$Rounds = 10)

$ErrorActionPreference = 'Stop'
$root = Join-Path $PWD 'artifacts/cua-driver/windows-recorder-diagnostic'
New-Item -ItemType Directory -Force $root | Out-Null
& "$PSScriptRoot/verify-user-session.ps1"
& "$PSScriptRoot/setup-ffmpeg.ps1"
$shim = (Get-Command ffmpeg.exe -ErrorAction Stop).Source
$expectedShim = Join-Path $env:ChocolateyInstall 'bin/ffmpeg.exe'
if ($shim -ine $expectedShim) { throw "Expected Chocolatey shim, found $shim" }
$binaries = @(Get-ChildItem (Join-Path $env:ChocolateyInstall 'lib/ffmpeg') -Recurse -Filter ffmpeg.exe -File)
if ($binaries.Count -ne 1) { throw "Expected one package executable, found $($binaries.Count)" }
$direct = $binaries[0].FullName
$ffprobe = Join-Path $binaries[0].DirectoryName 'ffprobe.exe'
if (-not (Test-Path $ffprobe)) { throw 'Missing package ffprobe' }
$shimVersion = (& $shim -version | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'Shim version probe failed' }
$directVersion = (& $direct -version | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'Direct version probe failed' }
if ($shimVersion -ne $directVersion) { throw 'Shim and direct versions differ' }
$probe = Join-Path $env:RUNNER_TEMP 'recorder-shutdown-probe.exe'
& rustc --edition=2021 "$PSScriptRoot/recorder-shutdown-probe.rs" -o $probe
if ($LASTEXITCODE -ne 0) { throw 'Probe compilation failed' }
[ordered]@{
    source_sha = (& git rev-parse HEAD)
    runner_os = $env:RUNNER_OS
    image_version = $env:ImageVersion
    shim = $shim
    direct = $direct
    shim_sha256 = (Get-FileHash $shim).Hash
    direct_sha256 = (Get-FileHash $direct).Hash
    ffmpeg_version = $directVersion
    rust_version = (& rustc --version)
    rounds = $Rounds
    kind = 'bounded process-boundary diagnostic; not Cua Driver certification'
} | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $root 'environment.json')
$variants = @(
    @{ Name = 'shim-close'; Exe = $shim; Mode = 'close' },
    @{ Name = 'direct-close'; Exe = $direct; Mode = 'close' },
    @{ Name = 'shim-hold'; Exe = $shim; Mode = 'hold' },
    @{ Name = 'direct-hold'; Exe = $direct; Mode = 'hold' }
)
$results = @()
for ($round = 0; $round -lt $Rounds; $round++) {
    for ($offset = 0; $offset -lt $variants.Count; $offset++) {
        $variant = $variants[($round + $offset) % $variants.Count]
        $label = '{0:D2}-{1}' -f $round, $variant.Name
        $video = Join-Path $root "$label.mp4"
        $log = Join-Path $root "$label.txt"
        & $probe $variant.Exe $video $variant.Mode 2>&1 | Tee-Object -FilePath $log
        $probeExit = $LASTEXITCODE
        $leftovers = @(Get-CimInstance Win32_Process -Filter "Name = 'ffmpeg.exe'" | Where-Object {
            $_.CommandLine -and $_.CommandLine.Contains($video, [StringComparison]::OrdinalIgnoreCase)
        })
        $leftovers | Select-Object ProcessId, ParentProcessId, CreationDate, ExecutablePath, CommandLine |
            ConvertTo-Json -Depth 5 | Set-Content (Join-Path $root "$label-leftovers.json")
        foreach ($process in $leftovers) {
            Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
        }
        $probeOutput = Get-Content $log -Raw
        $mediaExit = $null
        if (Test-Path $video) {
            & $ffprobe -v error -show_format -show_streams -of json $video 2>&1 |
                Set-Content (Join-Path $root "$label-media.json")
            $mediaExit = $LASTEXITCODE
        }
        $result = [ordered]@{
            label = $label
            variant = $variant.Name
            probe_exit = $probeExit
            finalized = [bool]($probeOutput -match '(?m)^finalized=true\r?$')
            forced_kill = [bool]($probeOutput -match '(?m)^forced_kill=true\r?$')
            leftover_processes = $leftovers.Count
            ffprobe_exit = $mediaExit
            trace = $probeOutput
        }
        $results += [pscustomobject]$result
        $result | ConvertTo-Json -Compress -Depth 5 | Add-Content (Join-Path $root 'results.jsonl')
    }
}
$summary = @('# Windows recorder shutdown diagnostic', '', 'Every attempt is retained. This is not an E2E certification result.', '', '| Variant | Attempts | Finalized | Forced kill | Probe errors |', '| --- | ---: | ---: | ---: | ---: |')
foreach ($group in ($results | Group-Object variant)) {
    $finalized = @($group.Group | Where-Object finalized).Count
    $killed = @($group.Group | Where-Object forced_kill).Count
    $errors = @($group.Group | Where-Object { $_.probe_exit -ne 0 }).Count
    $summary += "| $($group.Name) | $($group.Count) | $finalized | $killed | $errors |"
}
$summary | Set-Content (Join-Path $root 'summary.md')
if ($env:GITHUB_STEP_SUMMARY) { $summary | Add-Content $env:GITHUB_STEP_SUMMARY }
if (@($results | Where-Object { $_.probe_exit -ne 0 }).Count -gt 0) {
    throw 'Probe execution errors occurred; inspect retained traces'
}
