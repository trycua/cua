# Local Desktop MCP Server for Claude Code

Control your LOCAL PC directly from Claude Code in the terminal - no VMs, no cloud, no APIs.

## What is this?

This is a simplified MCP (Model Context Protocol) server that lets Claude Code control your computer directly through the terminal. It's built on top of the CUA (Computer Use Agents) platform but removes all the complexity of VMs, cloud services, and container orchestration.

## Features

Claude Code can:
- 📸 Take screenshots of your desktop
- 🖱️ Move and click the mouse
- ⌨️ Type text and press keyboard shortcuts
- 📁 Read and write files
- 💻 Run shell commands
- 📊 Get screen dimensions and cursor position
- 🎯 Double-click, right-click, scroll, and more

All actions happen on **your local machine** in real-time.

## Prerequisites

- Python 3.12 or 3.13
- Linux, macOS, or Windows
- Claude Code CLI installed
- CUA project (this repo)

## Quick Start

### Step 1: Install Dependencies

```bash
# From the cua project root directory
cd /home/user/cua

# Install required Python packages
pip install -e libs/python/core
pip install -e libs/python/computer
pip install -e libs/python/computer-server
pip install 'mcp[cli]'
```

### Step 2: Start the Computer Server

The computer server runs locally and provides the low-level interface for controlling your desktop:

```bash
python -m computer_server --host localhost --port 8000
```

Keep this terminal open. You should see:
```
INFO:     Started server process
INFO:     Waiting for application startup.
INFO:     Application startup complete.
INFO:     Uvicorn running on http://localhost:8000
```

### Step 3: Configure Claude Code

Add the MCP server to your Claude Code configuration:

**For Claude Code CLI (~/.config/claude-code/config.yaml or similar):**

```yaml
mcpServers:
  local-desktop:
    command: python
    args:
      - /home/user/cua/local-desktop-mcp/server.py
    env:
      CUA_API_HOST: localhost
      CUA_API_PORT: "8000"
```

### Step 4: Use with Claude Code

Start Claude Code and ask it to control your computer:

```bash
claude-code
```

Then try commands like:
- "Take a screenshot of my desktop"
- "Click on the Chrome icon"
- "Type 'hello world' in the currently focused window"
- "Open a new terminal window"
- "Read the file ~/Documents/notes.txt"

## Available Tools

| Tool | Description |
|------|-------------|
| `screenshot` | Take a screenshot of your desktop |
| `mouse_move` | Move the mouse cursor to (x, y) coordinates |
| `mouse_click` | Click left/right/middle mouse button |
| `double_click` | Double-click the mouse |
| `type_text` | Type text at the current cursor position |
| `press_key` | Press a key or key combination (e.g., 'enter', 'ctrl+c') |
| `scroll` | Scroll up/down/left/right |
| `run_command` | Execute a shell command |
| `read_file` | Read a text file |
| `write_file` | Write content to a file |
| `get_cursor_position` | Get current mouse position |
| `get_screen_size` | Get screen dimensions |

## Configuration

### Environment Variables

- `CUA_API_HOST`: Computer server host (default: `localhost`)
- `CUA_API_PORT`: Computer server port (default: `8000`)

### Custom Port

If you need to use a different port for the computer server:

```bash
# Start server on custom port
python -m computer_server --host localhost --port 9000

# Update Claude Code config
# Set CUA_API_PORT: "9000"
```

## Architecture

```
┌─────────────────┐
│  Claude Code    │
│   (Terminal)    │
└────────┬────────┘
         │ MCP Protocol (stdio)
         │
┌────────▼────────┐
│  local-desktop  │  ← This MCP Server
│   MCP Server    │
└────────┬────────┘
         │ HTTP/WebSocket (localhost:8000)
         │
┌────────▼────────┐
│ computer_server │  ← CUA Computer Server
│  (localhost)    │
└────────┬────────┘
         │ pynput, pyautogui, etc.
         │
┌────────▼────────┐
│   Your Desktop  │  ← Your Local PC
│  (Linux/Mac/Win)│
└─────────────────┘
```

## Safety

⚠️ **IMPORTANT SAFETY NOTES:**

- This server gives Claude Code **FULL CONTROL** of your local computer
- Commands are executed directly on your system
- Always review what Claude Code is doing before confirming actions
- The `run_command` tool can execute any shell command
- Consider running in a restricted user account or VM if you're concerned about safety
- Keep the computer server terminal visible to monitor activity

## Troubleshooting

### "Failed to connect to computer server"

Make sure the computer server is running:
```bash
python -m computer_server --host localhost --port 8000
```

### "Permission denied" errors

On Linux, you may need to install additional dependencies for controlling the desktop:
```bash
sudo apt-get install python3-tk python3-dev scrot xdotool
```

On macOS, you may need to grant accessibility permissions:
1. System Preferences → Security & Privacy → Privacy → Accessibility
2. Add Terminal or your terminal emulator to the allowed apps

### "Module not found" errors

Make sure you've installed the CUA packages:
```bash
pip install -e libs/python/core
pip install -e libs/python/computer
pip install -e libs/python/computer-server
```

### Claude Code doesn't see the tools

1. Check that your Claude Code config file has the correct path to `server.py`
2. Restart Claude Code after modifying the config
3. Check the computer server terminal for connection attempts

## Advanced Usage

### Running on a Different Machine

You can control a remote computer by pointing to its IP:

1. On the remote computer, start the server with a public IP:
   ```bash
   python -m computer_server --host 0.0.0.0 --port 8000
   ```

2. Update your Claude Code config:
   ```yaml
   env:
     CUA_API_HOST: 192.168.1.100  # Remote computer IP
     CUA_API_PORT: "8000"
   ```

⚠️ Warning: Only do this on a trusted network. The computer server has no authentication by default.

### Using with SSL

For secure remote connections, see the computer server docs for SSL setup:
```bash
python -m computer_server --host 0.0.0.0 --port 8443 --ssl
```

## Differences from Full CUA

This simplified MCP server:
- ✅ Targets your local desktop directly
- ✅ No VMs, Docker, or cloud services required
- ✅ Simple stdio-based MCP protocol
- ✅ Works with Claude Code out of the box
- ❌ No session management (single user only)
- ❌ No agent loop (Claude Code handles the loop)
- ❌ No streaming task execution
- ❌ No VM lifecycle management

If you need the full agent capabilities, use the main CUA MCP server in `libs/python/mcp-server/`.

## Examples

### Example 1: Take a screenshot
```
You: Take a screenshot and describe what you see
Claude: [Takes screenshot and describes desktop]
```

### Example 2: Open an application
```
You: Open Firefox browser
Claude: [Uses keyboard shortcut or clicks app icon]
```

### Example 3: Automate a workflow
```
You: Open a terminal, create a new directory called 'test', and create a file 'hello.txt' with 'Hello World'
Claude: [Executes each step sequentially]
```

### Example 4: File operations
```
You: Read the contents of ~/Documents/notes.txt and save it to ~/Desktop/backup.txt
Claude: [Reads file and writes to new location]
```

## Contributing

This is a simplified interface for local desktop control. For the full-featured CUA platform, see the main CUA repository.

## License

Same as CUA (MIT License)

## Support

- Check the [CUA Documentation](https://cua.ai/docs)
- Join the [CUA Discord](https://discord.com/invite/cua-ai)
- Report issues on [GitHub](https://github.com/trycua/cua/issues)
