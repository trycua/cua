# Quick Start Guide

Get Claude Code controlling your PC in 3 minutes!

## 1. Install (one time)

```bash
cd /home/user/cua/local-desktop-mcp
./setup.sh
```

This installs all dependencies and shows you the config to add to Claude Code.

## 2. Start the Computer Server

In Terminal 1:
```bash
cd /home/user/cua/local-desktop-mcp
./start-server.sh
```

Keep this running! You should see:
```
INFO:     Uvicorn running on http://localhost:8000
```

## 3. Configure Claude Code

Edit your Claude Code config file (e.g., `~/.config/claude-code/config.yaml`):

```yaml
mcpServers:
  local-desktop:
    command: python3
    args:
      - /home/user/cua/local-desktop-mcp/server.py
    env:
      CUA_API_HOST: localhost
      CUA_API_PORT: "8000"
```

## 4. Use Claude Code

In Terminal 2:
```bash
claude-code
```

Try these commands:
- "Take a screenshot"
- "What's on my screen?"
- "Click at coordinates 500, 300"
- "Type 'hello world'"
- "Press enter"
- "Open a new terminal window"

## That's it! 🎉

Claude Code can now control your computer directly through the terminal.

## Troubleshooting

### Computer server won't start
```bash
# Try a different port
python3 -m computer_server --host localhost --port 9000

# Update CUA_API_PORT in Claude Code config to "9000"
```

### Claude Code can't connect
1. Check computer server is running (Terminal 1)
2. Check Claude Code config file path is correct
3. Restart Claude Code after config changes

### Permission errors (Linux)
```bash
sudo apt-get install python3-tk python3-dev scrot xdotool
```

### Permission errors (macOS)
Grant Accessibility permissions:
1. System Preferences → Security & Privacy → Privacy → Accessibility
2. Add your terminal app

## Next Steps

Read the full [README.md](README.md) for:
- All available tools
- Advanced configuration
- Safety notes
- Remote desktop control
- Architecture details
