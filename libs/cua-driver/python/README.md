# cua-driver

Python wrapper for [cua-driver](https://github.com/trycua/cua/tree/main/libs/cua-driver) - a cross-platform MCP (Model Context Protocol) server for computer-use automation.

## Installation

```bash
pip install cua-driver
```

## Usage

The package provides a `cua-driver` command that wraps the native Rust binary:

```bash
# Start MCP server over stdio
cua-driver mcp

# Check version
cua-driver --version

# View help
cua-driver --help
```

### Python API

You can also use the Python API directly:

```python
from cua_driver import run_cua_driver, get_binary_path

# Run with custom args
exit_code = run_cua_driver(["mcp"])

# Get path to bundled binary
binary_path = get_binary_path()
```

## Features

- **Cross-platform**: Works on macOS (universal), Linux (x86_64), and Windows (x86_64/ARM64)
- **Zero dependencies**: Pure Python wrapper with no external dependencies
- **Stdio passthrough**: Transparent piping for MCP protocol communication
- **Bundled binary**: No separate installation required - the Rust binary is included in the wheel

## What is cua-driver?

`cua-driver` is a cross-platform MCP server that provides 40+ tools for:
- Screen capture and window management
- Mouse and keyboard automation
- Accessibility tree interaction
- Application launching and control

It enables AI agents to interact with desktop applications through the Model Context Protocol.

## Platform Support

| Platform | Architecture | Status |
|----------|-------------|---------|
| macOS    | Universal (ARM64 + x86_64) | ✅ Supported |
| Linux    | x86_64 | ✅ Supported |
| Windows  | x86_64 | ✅ Supported |
| Windows  | ARM64 | ✅ Supported |

## License

MIT License - see [LICENSE](https://github.com/trycua/cua/blob/main/LICENSE.md)

## Links

- [GitHub Repository](https://github.com/trycua/cua)
- [Documentation](https://github.com/trycua/cua/tree/main/libs/cua-driver)
- [Issue Tracker](https://github.com/trycua/cua/issues)
