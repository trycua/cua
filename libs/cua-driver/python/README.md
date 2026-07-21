# cua-driver Python SDK

Rust-backed Python SDK and bundled executable for
[Cua Driver](https://github.com/trycua/cua/tree/main/libs/cua-driver).

## Product boundary

This package is for client applications importing Cua Driver as an SDK:

```python
from cua_driver import CuaDriver
```

It does not contain a Python MCP client. Agents already have runtime-neutral
MCP clients and should configure the bundled server directly:

```text
cua-driver mcp
```

The removed pre-release MCP facade used `CuaDriver.stdio()`,
`AsyncCuaDriver`, `*Args`, and transport classes. Application code migrates to
the synchronous Rust-backed methods shown below; agent code removes the Cua
package import and supplies `cua-driver mcp` to its agent SDK.

## Installation

Install and usage docs live at https://cua.ai/docs/how-to-guides/driver/install
and https://cua.ai/docs/reference/cua-driver/mcp-tools.

The wheel contains generated UniFFI bindings, a platform-specific Rust SDK
library, and the `cua-driver` executable. The daemon must be running and have
the required OS permissions before SDK calls are made.

## SDK example

```python
from cua_driver import (
    CaptureScope,
    CuaDriver,
    EndSessionInput,
    GetDesktopStateInput,
    StartSessionInput,
)

driver = CuaDriver.connect(None)  # or pass an explicit daemon socket path
driver.start_session(
    StartSessionInput(session="demo", capture_scope=CaptureScope.DESKTOP)
)
try:
    desktop = driver.get_desktop_state(
        GetDesktopStateInput(session="demo", screenshot_out_file=None)
    )
    print(desktop.images[0].mime_type)
finally:
    driver.end_session(EndSessionInput(session="demo"))
```

The SDK is currently synchronous. Desktop calls return a typed `ToolResult`
with text, images, verification/error metadata, and `structured_json` /
`raw_json` for platform-extensible results. Session lifecycle calls return
dedicated generated records.

## Binary wrapper

The package also exposes the bundled executable:

```python
from cua_driver import get_binary_path, run_cua_driver

print(get_binary_path())
exit_code = run_cua_driver(["mcp"])
```

## Platform support

| Platform | Architecture | Status |
| --- | --- | --- |
| macOS | Universal (ARM64 + x86_64) | Supported |
| Linux | x86_64 | Supported |
| Windows | x86_64 | Supported |
| Windows | ARM64 | Supported |

## License

MIT License — see [LICENSE](https://github.com/trycua/cua/blob/main/LICENSE.md).
