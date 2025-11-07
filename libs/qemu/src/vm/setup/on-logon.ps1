$scriptFolder = "\\host.lan\Data"
$pythonServerPort = 5000

# Start the Caddy reverse proxy in a non-blocking manner
Write-Host "Running the Caddy reverse proxy from port 9222 to port 1337"
Start-Process -NoNewWindow -FilePath "powershell" -ArgumentList "-Command", "caddy reverse-proxy --from :9222 --to :1337"

# Start the CUA Computer Server
Write-Host "Running the CUA Computer Server on port $pythonServerPort..."
python -m computer_server --port $pythonServerPort
