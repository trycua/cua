# Cua Computer Server

Server component for the Computer-Use Interface (CUI) framework providing low-level computer control primitives.

**[Documentation](https://cua.ai/docs/cua/guide/advanced/local-computer-server)** - Installation, guides, and configuration.

## Interfaces

The Computer Server supports multiple interfaces:

- **HTTP/WebSocket** (default) - REST API and WebSocket for programmatic access
- **MCP** - Model Context Protocol for Claude Code, OpenCode, Cursor, and other MCP-compatible tools

## Installation

```bash
# Basic installation (HTTP/WebSocket only)
pip install cua-computer-server-server

# With MCP support
pip install cua-computer-server-server[mcp]
```

## Usage

### HTTP/WebSocket Mode (Default)

```bash
# Start the server on default port 8000
python -m computer_server

# Or with custom port
python -m computer_server --port 8080
```

### MCP Mode (for Claude Code)

```bash
# Start in MCP mode
python -m computer_server --mcp

# With resolution auto-detection (logs actual screen resolution)
python -m computer_server --mcp --detect-resolution

# With target resolution (useful for Retina displays or VMs)
# Screenshots will be resized to target resolution and coordinates will be scaled
python -m computer_server --mcp --width 1512 --height 982
```

#### Resolution Scaling

When running on Retina displays or in VMs where the coordinate system may differ, use the `--width` and `--height` flags to specify the target resolution:

- Screenshots will be resized to the target resolution
- Click coordinates received will be scaled from target to actual screen coordinates
- Cursor position will be reported in target coordinates

This ensures the AI model sees consistent coordinates between screenshots and mouse actions.

#### Claude Code Integration

Add to your Claude Code configuration:

```bash
# Add the MCP server
claude mcp add cua-computer-server -- python -m computer_server --mcp
```

Or manually add to `~/.claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "cua-computer-server": {
      "command": "python",
      "args": ["-m", "computer_server", "--mcp"]
    }
  }
}
```

## Available MCP Tools

The MCP interface exposes 40+ tools for computer control:

### Screen & Mouse
- `computer_screenshot` - Capture current screen
- `computer_click` - Click at coordinates
- `computer_double_click` - Double-click
- `computer_move` - Move cursor
- `computer_drag` - Drag from start to end coordinates
- `computer_scroll` - Scroll at position
- `computer_get_screen_size` - Get screen dimensions
- `computer_get_cursor_position` - Get cursor position

### Keyboard
- `computer_type` - Type text
- `computer_press_key` - Press a single key
- `computer_hotkey` - Press key combination (e.g., Ctrl+C)
- `computer_key_down` / `computer_key_up` - Hold/release keys

### Clipboard
- `computer_clipboard_get` - Get clipboard content
- `computer_clipboard_set` - Set clipboard content

### Shell
- `computer_run_command` - Execute shell command

### File System
- `computer_file_read` / `computer_file_write` - Read/write files
- `computer_file_exists` / `computer_directory_exists` - Check existence
- `computer_list_directory` - List directory contents
- `computer_create_directory` - Create directory
- `computer_delete_file` / `computer_delete_directory` - Delete files/directories

### Window Management
- `computer_open` - Open file or URL
- `computer_launch_app` - Launch application
- `computer_get_active_window` - Get active window
- `computer_activate_window` - Focus a window
- `computer_minimize_window` / `computer_maximize_window` - Window state
- `computer_close_window` - Close window

### Accessibility
- `computer_get_accessibility_tree` - Get UI element tree
- `computer_find_element` - Find UI element by role/title
