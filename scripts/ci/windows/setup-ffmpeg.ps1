# Ensure FFmpeg is available for Windows E2E trajectory recording.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$chocolateyAttempts = 3
$fallbackAttempts = 3
$retryDelaySeconds = 5
$fallbackBaseUrl = "https://www.gyan.dev/ffmpeg/builds"

function Update-ProcessPath {
    $machinePath = [Environment]::GetEnvironmentVariable("PATH", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    $env:PATH = "$machinePath;$userPath;$($env:PATH)"
}

function Test-FFmpegAvailable {
    return (
        $null -ne (Get-Command ffmpeg.exe -ErrorAction SilentlyContinue) -and
        $null -ne (Get-Command ffprobe.exe -ErrorAction SilentlyContinue)
    )
}

function Invoke-DownloadWithRetry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Uri,
        [Parameter(Mandatory = $true)]
        [string]$OutFile
    )

    for ($attempt = 1; $attempt -le $fallbackAttempts; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing
            return
        }
        catch {
            Remove-Item -LiteralPath $OutFile -Force -ErrorAction SilentlyContinue
            if ($attempt -eq $fallbackAttempts) { throw }
            Write-Warning "Download attempt $attempt of $fallbackAttempts failed; retrying in $retryDelaySeconds seconds: $($_.Exception.Message)"
            Start-Sleep -Seconds $retryDelaySeconds
        }
    }
}

function Install-VerifiedFallback {
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($architecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
        throw "The verified FFmpeg fallback supports only Windows x64; detected $architecture"
    }

    $tempBase = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
        [System.IO.Path]::GetTempPath()
    } else {
        $env:RUNNER_TEMP
    }
    $installRoot = Join-Path $tempBase ("cua-ffmpeg-" + [guid]::NewGuid().ToString("N"))
    $versionFile = Join-Path $installRoot "ffmpeg-release.ver"
    New-Item -ItemType Directory -Force -Path $installRoot | Out-Null

    Invoke-DownloadWithRetry `
        -Uri "$fallbackBaseUrl/ffmpeg-release-essentials.zip.ver" `
        -OutFile $versionFile
    $version = (Get-Content -LiteralPath $versionFile -Raw).Trim()
    if ($version -notmatch '^\d+\.\d+(?:\.\d+)?$') {
        throw "Unexpected FFmpeg fallback version: $version"
    }

    $archiveName = "ffmpeg-$version-essentials_build.zip"
    $packageBaseUrl = "$fallbackBaseUrl/packages"
    $archivePath = Join-Path $installRoot $archiveName
    $checksumPath = "$archivePath.sha256"
    Invoke-DownloadWithRetry -Uri "$packageBaseUrl/$archiveName" -OutFile $archivePath
    Invoke-DownloadWithRetry -Uri "$packageBaseUrl/$archiveName.sha256" -OutFile $checksumPath

    $checksumText = Get-Content -LiteralPath $checksumPath -Raw
    $checksumMatch = [regex]::Match($checksumText, '(?i)\b[0-9a-f]{64}\b')
    if (-not $checksumMatch.Success) {
        throw "The FFmpeg fallback did not publish a valid SHA-256 checksum"
    }
    $expectedHash = $checksumMatch.Value.ToUpperInvariant()
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) {
        throw "FFmpeg fallback checksum mismatch: expected $expectedHash, received $actualHash"
    }

    $expandedPath = Join-Path $installRoot "expanded"
    Expand-Archive -LiteralPath $archivePath -DestinationPath $expandedPath -Force
    $ffmpeg = Get-ChildItem -LiteralPath $expandedPath -Filter ffmpeg.exe -File -Recurse |
        Select-Object -First 1
    if ($null -eq $ffmpeg) {
        throw "The verified FFmpeg fallback archive did not contain ffmpeg.exe"
    }
    $binPath = $ffmpeg.Directory.FullName
    if (-not (Test-Path -LiteralPath (Join-Path $binPath "ffprobe.exe") -PathType Leaf)) {
        throw "The verified FFmpeg fallback archive did not contain ffprobe.exe"
    }

    $env:PATH = "$binPath;$($env:PATH)"
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_PATH)) {
        Add-Content -LiteralPath $env:GITHUB_PATH -Value $binPath
    }
    Write-Host "Installed checksum-verified FFmpeg $version fallback from gyan.dev"
}

if (-not (Test-FFmpegAvailable)) {
    $choco = Get-Command choco.exe -ErrorAction SilentlyContinue
    if ($null -ne $choco) {
        for ($attempt = 1; $attempt -le $chocolateyAttempts; $attempt++) {
            Write-Host "Installing FFmpeg with Chocolatey (attempt $attempt of $chocolateyAttempts)"
            & $choco.Source install ffmpeg -y --no-progress
            $chocolateyExitCode = $LASTEXITCODE
            Update-ProcessPath
            if ($chocolateyExitCode -eq 0 -and (Test-FFmpegAvailable)) { break }
            if ($attempt -lt $chocolateyAttempts) {
                Write-Warning "Chocolatey FFmpeg setup failed with exit code $chocolateyExitCode; retrying in $retryDelaySeconds seconds"
                Start-Sleep -Seconds $retryDelaySeconds
            }
        }
    }

    if (-not (Test-FFmpegAvailable)) {
        Write-Warning "Chocolatey did not provide FFmpeg and ffprobe; using the checksum-verified fallback"
        Install-VerifiedFallback
    }
}

foreach ($toolName in @("ffmpeg.exe", "ffprobe.exe")) {
    $tool = Get-Command $toolName -ErrorAction Stop
    Write-Host "Verified $toolName at $($tool.Source)"
    & $tool.Source -version
    if ($LASTEXITCODE -ne 0) {
        throw "$toolName -version failed with exit code $LASTEXITCODE"
    }
}
