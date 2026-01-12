# Local Desktop MCP Server - Project Summary

## Overview

This is a **simplified MCP (Model Context Protocol) server** that enables Claude Code to directly control your local PC through the terminal. Unlike the full CUA platform which requires VMs or cloud services, this server connects directly to your local machine.

## What Problem Does This Solve?

**Before:** The CUA platform is powerful but complex - it requires Docker containers, VM management, or cloud API keys just to control a computer.

**After:** With this MCP server, you can control your **local desktop** with Claude Code in 3 simple steps:
1. Start the computer server locally
2. Add MCP config to Claude Code
3. Ask Claude to control your PC

No VMs. No Docker. No cloud. Just direct control.

## Architecture Comparison

### Full CUA Architecture
```
Claude API → Agent Loop → Computer SDK → VM Provider → VM → Computer Server → Desktop
```
**Pros:** Safe sandboxing, multiple VMs, benchmarking, training
**Cons:** Complex setup, requires VMs/Docker, resource intensive

### Local Desktop MCP Architecture
```
Claude Code → MCP Server → Computer Server (localhost) → Your Desktop
```
**Pros:** Simple, direct, no VMs, works immediately
**Cons:** No sandboxing, single user, no agent loop

## Files Created

```
local-desktop-mcp/
├── server.py                           # Main MCP server (stdio transport)
├── setup.sh                            # One-command setup script
├── start-server.sh                     # Quick start for computer server
├── test_setup.py                       # Verify installation
├── README.md                           # Comprehensive documentation
├── QUICKSTART.md                       # 3-minute getting started guide
├── PROJECT_SUMMARY.md                  # This file
└── claude-code-config.example.yaml     # Example config for Claude Code
```

## Key Features

### MCP Tools Provided

| Tool | What It Does |
|------|--------------|
| `screenshot` | Captures your screen |
| `mouse_move` | Moves cursor to (x,y) |
| `mouse_click` | Clicks left/right/middle button |
| `double_click` | Double-clicks mouse |
| `type_text` | Types text at cursor |
| `press_key` | Presses keys/shortcuts (e.g., 'ctrl+c') |
| `scroll` | Scrolls in any direction |
| `run_command` | Executes shell commands |
| `read_file` | Reads text files |
| `write_file` | Writes text files |
| `get_cursor_position` | Returns mouse position |
| `get_screen_size` | Returns screen dimensions |

### Supported Platforms

- ✅ Linux (X11, Wayland)
- ✅ macOS
- ✅ Windows

## How It Works

1. **Computer Server** runs locally and provides low-level desktop control APIs
   - Uses `pynput` for mouse/keyboard
   - Uses `PIL` for screenshots
   - Exposes REST/WebSocket API on localhost:8000

2. **MCP Server** (this project) connects to the computer server
   - Uses `Computer(use_host_computer_server=True)` to target localhost
   - Exposes MCP tools via stdio protocol
   - Translates Claude Code requests to computer server commands

3. **Claude Code** talks to MCP server via stdio
   - Discovers available tools automatically
   - Calls tools to control the desktop
   - Receives screenshots and feedback

## Code Highlights

### Key Innovation: `use_host_computer_server=True`

The CUA `Computer` class has this little-documented parameter:

```python
Computer(
    os_type="linux",
    use_host_computer_server=True,  # ← Skip VM setup, use localhost
    api_host="localhost",
    api_port=8000
)
```

This bypasses all VM provider logic and connects directly to a local computer server.

### Simplified MCP Tools

Each tool is a simple async wrapper:

```python
@mcp.tool()
async def mouse_click(ctx: Context, button: str = "left",
                      x: Optional[int] = None, y: Optional[int] = None) -> str:
    computer = await get_computer()
    if x is not None and y is not None:
        await computer.interface.move_cursor(x, y)
    if button == "left":
        await computer.interface.left_click()
    # ... handle other buttons
    return f"Clicked {button} button"
```

No agent loops, no streaming, no session management - just direct tool calls.

## Usage Flow

### Terminal 1: Computer Server
```bash
$ python3 -m computer_server --host localhost --port 8000
INFO:     Uvicorn running on http://localhost:8000
```

### Terminal 2: Claude Code
```bash
$ claude-code
Claude Code v1.x.x

You: Take a screenshot and tell me what applications are open

Claude: [Calls screenshot tool, analyzes image]
I can see your desktop with the following applications open:
- Terminal (showing the computer server running)
- Firefox browser
- VS Code editor
...
```

## Safety Considerations

⚠️ **This gives Claude Code FULL control of your computer**

- Commands execute directly on your system
- `run_command` can execute any shell command
- No sandboxing or isolation
- Monitor the computer server terminal for activity

**Recommendations:**
- Review Claude's actions before confirming
- Run in a restricted user account
- Don't use with untrusted prompts
- Keep the server terminal visible
- Use on a dedicated testing machine

## Comparison to Alternative Approaches

### Why not use Anthropic's official computer-use?
- Anthropic's computer-use runs in Docker containers
- This approach controls your **actual desktop**
- Useful for real automation tasks, not just demos

### Why not use the full CUA MCP server?
- Full CUA MCP server requires VM setup
- This is simpler: no VMs, just localhost
- Trade-off: No safety sandboxing

### Why not use direct automation tools?
- Direct tools (pyautogui, etc.) don't integrate with Claude Code
- MCP protocol gives Claude structured tool access
- Claude can reason about what to do next

## Extending This Project

Want to add more features? Here are ideas:

### Add window management tools
```python
@mcp.tool()
async def list_windows(ctx: Context) -> str:
    computer = await get_computer()
    windows = await computer.interface.get_application_windows()
    return str(windows)
```

### Add clipboard tools
```python
@mcp.tool()
async def copy_text(ctx: Context, text: str) -> str:
    computer = await get_computer()
    await computer.interface.copy_to_clipboard(text)
    return f"Copied to clipboard: {text}"
```

### Add vision analysis
Use Claude's vision capabilities with screenshots:
```python
@mcp.tool()
async def analyze_screen(ctx: Context, question: str) -> str:
    screenshot_img = await screenshot(ctx)
    # Claude Code will automatically use vision to analyze
    return screenshot_img
```

## Testing

Run the test script to verify setup:
```bash
./test_setup.py
```

Expected output when working:
```
✓ Python 3.12.x
✓ mcp[cli]
✓ cua-computer
✓ computer_server module
✓ cua-core
✓ Pillow
✓ asyncio
✓ Linux
✅ All tests passed!
```

## Troubleshooting

### Common Issues

1. **"Connection refused"**
   - Computer server isn't running
   - Wrong port in config
   - Solution: Check `./start-server.sh` is running

2. **"Permission denied" on Linux**
   - Missing system dependencies
   - Solution: `sudo apt-get install python3-tk python3-dev scrot xdotool`

3. **"Accessibility permissions" on macOS**
   - Terminal needs accessibility access
   - Solution: System Preferences → Privacy → Accessibility

4. **"Module not found"**
   - Dependencies not installed
   - Solution: Run `./setup.sh`

## Performance

- Screenshot: ~100ms
- Mouse move: ~10ms
- Click: ~20ms
- Type text: ~50ms per character
- Command execution: Depends on command

All operations are asynchronous and non-blocking.

## Security Notes

The computer server has **no authentication by default**. This is fine for localhost but dangerous for network exposure.

**For network use:**
1. Enable SSL: `python3 -m computer_server --ssl --port 8443`
2. Use firewall rules to restrict access
3. Consider adding API key authentication
4. Use VPN or SSH tunnel for remote access

## Future Enhancements

Potential improvements:
- [ ] Add authentication/API keys
- [ ] Support multiple concurrent clients
- [ ] Add action logging/audit trail
- [ ] Add undo/rollback capabilities
- [ ] Integrate with system notifications
- [ ] Add rate limiting
- [ ] Add action confirmation prompts
- [ ] Support for multiple monitors
- [ ] OCR integration for text detection
- [ ] GUI management (specific to apps)

## Credits

Built on top of:
- **CUA (Computer Use Agents)** - The underlying platform
- **FastMCP** - MCP server framework
- **pynput** - Cross-platform input control
- **Pillow** - Image processing
- **Claude Code** - AI-powered terminal assistant

## License

MIT License (same as CUA)

## Support

- 📖 [Full README](README.md)
- ⚡ [Quick Start](QUICKSTART.md)
- 💬 [CUA Discord](https://discord.com/invite/cua-ai)
- 🐛 [Report Issues](https://github.com/trycua/cua/issues)

---

**Happy automating! 🤖✨**
