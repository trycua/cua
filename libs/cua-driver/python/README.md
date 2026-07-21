# cua-driver Python SDK

Python SDK and bundled binary for [cua-driver](https://github.com/trycua/cua/tree/main/libs/cua-driver), a cross-platform MCP server for computer-use automation.

## Installation

Install and usage docs live at https://cua.ai/docs/how-to-guides/driver/install
and https://cua.ai/docs/reference/cua-driver/mcp-tools.

## Usage

The package provides a `cua-driver` command that wraps the native Rust binary.
See the canonical tool reference at https://cua.ai/docs/reference/cua-driver/mcp-tools.

## Python SDK

The synchronous and async interfaces launch the bundled driver and speak MCP over
stdio. The generated typed methods cover the portable contract, while
`call_tool` remains available for runtime-discovered and platform-specific tools.

```python
from cua_driver import CuaDriver, GetDesktopStateArgs, StartSessionArgs

with CuaDriver.stdio() as driver:
    driver.start_session(StartSessionArgs("demo", capture_scope="auto"))
    desktop = driver.get_desktop_state(GetDesktopStateArgs(session="demo"))
    print(desktop.images[0].mime_type)
```

Async applications can use the native asyncio transport:

```python
from cua_driver import AsyncCuaDriver, GetDesktopStateArgs

async with AsyncCuaDriver.stdio() as driver:
    desktop = await driver.get_desktop_state(GetDesktopStateArgs(session="demo"))
```

## Binary wrapper

The existing wrapper API remains available:

```python
from cua_driver import run_cua_driver, get_binary_path

# Run with custom args
exit_code = run_cua_driver(["mcp"])

# Get path to bundled binary
binary_path = get_binary_path()
```

## Features

- **Cross-platform**: Works on macOS (universal), Linux (x86_64), and Windows (x86_64/ARM64)
- **Zero dependencies**: Pure Python SDK with no external dependencies
- **Typed SDK**: Generated synchronous and async MCP methods
- **Stdio passthrough**: Transparent piping for MCP protocol communication
- **Bundled binary**: No separate installation required - the Rust binary is included in the wheel

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
- [Documentation](https://cua.ai/docs)
- [Issue Tracker](https://github.com/trycua/cua/issues)
