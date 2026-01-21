# Cua Computer Server

Server component for the Computer-Use Interface (CUI) framework providing low-level computer control primitives.

**[Documentation](https://cua.ai/docs/cua/guide/advanced/local-computer-server)** - Installation, guides, and configuration.

## Interfaces

The Computer Server exposes multiple interfaces simultaneously:

- **HTTP/WebSocket** - REST API and WebSocket for programmatic access
- **MCP via HTTP** - Model Context Protocol over streamable HTTP at `/mcp` endpoint

## Installation

```bash
# Basic installation (HTTP/WebSocket only)
pip install cua-computer-server

# With MCP support
pip install cua-computer-server[mcp]
```

## Usage

```bash
# Start the server on default port 8000
python -m computer_server

# Or with custom port
python -m computer_server --port 8080

# With resolution scaling (useful for Retina displays or VMs)
python -m computer_server --width 1512 --height 982
```

This provides:
- HTTP API at `/ws`, `/cmd`, `/status` endpoints
- MCP server at `/mcp` endpoint (requires `fastmcp` package)

MCP clients can connect via streamable HTTP at `http://localhost:8000/mcp`.

#### Resolution Scaling

When running on Retina displays or in VMs where the coordinate system may differ, use the `--width` and `--height` flags to specify the target resolution:

- Screenshots will be resized to the target resolution
- Click coordinates received will be scaled from target to actual screen coordinates
- Cursor position will be reported in target coordinates

This ensures the AI model sees consistent coordinates between screenshots and mouse actions.

#### Claude Code Integration

1. Start the server (or run as a service/LaunchAgent):
```bash
python -m computer_server --port 8000
```

2. Add the MCP server URL to Claude Code:
```bash
claude mcp add cua-computer-server --transport http http://localhost:8000/mcp
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
