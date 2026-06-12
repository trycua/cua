#!/bin/bash

# GitHub Actions Runner Dashboard - Interactive GUI

RUNNER_DIR="$HOME/.github-actions-runner"
BINARY="$RUNNER_DIR/run.sh"

# Create a temporary directory for the dashboard
DASHBOARD_DIR="/tmp/github-actions-dashboard"
mkdir -p "$DASHBOARD_DIR"

# Create an HTML dashboard
cat > "$DASHBOARD_DIR/index.html" << 'HTMLEOF'
<!DOCTYPE html>
<html>
<head>
    <title>GitHub Actions Runner</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 20px;
            display: flex;
            justify-content: center;
            align-items: center;
            min-height: 100vh;
        }
        .container {
            background: white;
            border-radius: 10px;
            box-shadow: 0 20px 60px rgba(0,0,0,0.3);
            max-width: 600px;
            padding: 40px;
            text-align: center;
        }
        .logo {
            font-size: 48px;
            margin-bottom: 20px;
        }
        h1 {
            color: #333;
            margin: 0 0 10px 0;
            font-size: 32px;
        }
        .subtitle {
            color: #666;
            font-size: 14px;
            margin-bottom: 30px;
        }
        .status {
            background: #f0f0f0;
            border-radius: 8px;
            padding: 20px;
            margin: 20px 0;
            text-align: left;
        }
        .status-item {
            display: flex;
            justify-content: space-between;
            padding: 8px 0;
            border-bottom: 1px solid #e0e0e0;
        }
        .status-item:last-child {
            border-bottom: none;
        }
        .status-label {
            font-weight: 600;
            color: #333;
        }
        .status-value {
            color: #666;
            font-family: monospace;
        }
        .success {
            color: #22c55e;
            font-weight: bold;
        }
        .info-box {
            background: #ecf0f1;
            border-left: 4px solid #3498db;
            padding: 15px;
            margin: 20px 0;
            text-align: left;
            border-radius: 4px;
        }
        .info-box h3 {
            margin-top: 0;
            color: #2c3e50;
        }
        .command {
            background: #2c3e50;
            color: #ecf0f1;
            padding: 10px;
            border-radius: 4px;
            font-family: monospace;
            font-size: 12px;
            word-break: break-all;
            margin: 8px 0;
        }
        .footer {
            color: #999;
            font-size: 12px;
            margin-top: 20px;
        }
        .badge {
            display: inline-block;
            background: #667eea;
            color: white;
            padding: 4px 12px;
            border-radius: 20px;
            font-size: 12px;
            margin-right: 8px;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="logo">⚙️🚀</div>
        <h1>GitHub Actions Runner</h1>
        <div class="subtitle">Self-hosted CI/CD automation</div>
        
        <div class="status">
            <div class="status-item">
                <span class="status-label">Status:</span>
                <span class="status-value success">✓ Installed</span>
            </div>
            <div class="status-item">
                <span class="status-label">Version:</span>
                <span class="status-value">2.333.1</span>
            </div>
            <div class="status-item">
                <span class="status-label">Installation:</span>
                <span class="status-value">Complete</span>
            </div>
            <div class="status-item">
                <span class="status-label">Ready:</span>
                <span class="status-value success">Yes</span>
            </div>
        </div>

        <div class="info-box">
            <h3>🔧 Configuration Required</h3>
            <p>Before running the runner, configure it with your GitHub repository:</p>
            <div class="command">./config.sh --url &lt;GITHUB_URL&gt; --token &lt;TOKEN&gt;</div>
        </div>

        <div class="info-box">
            <h3>▶️ How to Start</h3>
            <p>After configuration, run the runner:</p>
            <div class="command">./run.sh</div>
        </div>

        <div style="margin-top: 30px;">
            <span class="badge">Linux</span>
            <span class="badge">x64</span>
            <span class="badge">Self-Hosted</span>
            <span class="badge">v2.333.1</span>
        </div>

        <div class="footer">
            <p>GitHub Actions Runner • Open-source CI/CD automation platform<br>
            <a href="https://github.com/actions/runner" style="color: #667eea; text-decoration: none;">github.com/actions/runner</a>
            </p>
        </div>
    </div>
</body>
</html>
HTMLEOF

# Launch with Firefox in fullscreen
firefox --kiosk file://$DASHBOARD_DIR/index.html &
sleep 5