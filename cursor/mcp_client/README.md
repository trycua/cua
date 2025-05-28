# Cursor MCP Client

A client library for connecting Cursor to the local MCP (Model Control Protocol) server on macOS.

## Overview

The Cursor MCP client provides a seamless integration between the Cursor editor and the local system on macOS. It allows Cursor to access the file system, manage terminal sessions, and handle other local operations through a fast and secure protocol.

## Architecture

```
            ┌───────────────┐        ┌─────────────┐
            │ Cursor Editor │        │ MCP Server  │
            │ (Electron/TS) │────────▶ (Python)    │
            └───────────────┘        └─────────────┘
                    │                       │
                    │                       │
                    ▼                       ▼
            ┌───────────────┐        ┌─────────────┐
            │ MCP Client    │        │ macOS       │
            │ (Python)      │────────▶ System APIs │
            └───────────────┘        └─────────────┘
```

## Components

- **CursorMCPClient**: Core client class that handles the TCP/IP connection to the MCP server
- **FileSystem**: Handles file operations (read, write, delete, list, etc.)
- **Terminal**: Manages terminal sessions (create, input, output, resize, close)
- **UI**: Provides UI components for user interaction (connection dialog, status display)
- **Main**: High-level functions for using the client

## Usage

### Basic Connection

```python
from cursor.mcp_client.main import connect, disconnect, get_filesystem, get_terminal

# Connect to the local MCP server
await connect()

# Get filesystem handler
fs = await get_filesystem()
files = await fs.list_directory("/path/to/dir")

# Create and use a terminal
term = await get_terminal()
terminal_id = await term.create_terminal()
await term.send_input(terminal_id, "ls -la\n")
output = await term.get_output(terminal_id)

# Disconnect when done
await disconnect()
```

### High-level Functions

The main module provides high-level functions for common operations:

- `connect()`: Connect to the local MCP server
- `disconnect()`: Disconnect from the server
- `is_connected()`: Check connection status
- `ensure_connected()`: Ensure connection exists, showing dialog if needed
- `get_filesystem()`: Get the filesystem handler
- `get_terminal()`: Get the terminal handler

## Security

The MCP client follows these security practices:

1. Only connects to localhost by default (127.0.0.1)
2. Validates all inputs from the server
3. Implements proper error handling
4. Uses authenticated connections
5. Limits file system access to appropriate paths

## Development

See the example script in `examples/mcp_client_test.py` for a demonstration of the client functionality. 