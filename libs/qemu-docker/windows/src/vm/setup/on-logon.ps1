# Start the Caddy Reverse Proxy scheduled task (runs in background, hidden)
Write-Host "Starting Caddy Reverse Proxy task..."
Start-ScheduledTask -TaskName "Caddy-Reverse-Proxy"

# Start the CUA Computer Server scheduled task (runs in background, hidden)
Write-Host "Starting CUA Computer Server task..."
Start-ScheduledTask -TaskName "CUA-Computer-Server"
