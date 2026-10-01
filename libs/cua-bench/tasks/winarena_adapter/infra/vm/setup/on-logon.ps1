$cuaServerPort = 5000

# Add uv tools bin to PATH
$uvToolsBinPath = "$env:USERPROFILE\.local\bin"
$env:PATH = "$uvToolsBinPath;$env:PATH"

# Start the Caddy reverse proxy in a non-blocking manner
Write-Host "Running the Caddy reverse proxy from port 9222 to port 1337"
Start-Process -NoNewWindow -FilePath "powershell" -ArgumentList "-Command", "caddy reverse-proxy --from :9222 --to :1337"

# Wait for cua-computer-server to be installed (setup.ps1 installs it)
$cuaServerExe = Join-Path $uvToolsBinPath "cua-computer-server.exe"
$maxWaitSeconds = 600  # Wait up to 10 minutes for setup.ps1 to complete
$waitedSeconds = 0
$checkInterval = 10

Write-Host "Checking for cua-computer-server installation..."
while (-not (Test-Path $cuaServerExe)) {
    if ($waitedSeconds -ge $maxWaitSeconds) {
        Write-Error "Timeout waiting for cua-computer-server to be installed. setup.ps1 may have failed."
        exit 1
    }
    Write-Host "Waiting for cua-computer-server.exe to be installed... ($waitedSeconds/$maxWaitSeconds seconds)"
    Start-Sleep -Seconds $checkInterval
    $waitedSeconds += $checkInterval
}

Write-Host "cua-computer-server found at: $cuaServerExe"

# Wait a bit more to ensure the file is fully written and ready
Start-Sleep -Seconds 5

# Start the CUA Computer Server
Write-Host "Running the CUA Computer Server on port $cuaServerPort..."
& $cuaServerExe --port $cuaServerPort
