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
`AsyncCuaDriver`, `*Args`, and transport classes. Application code imports the
typed Rust-backed SDK shown below; agent code supplies `cua-driver mcp` to its
agent SDK.

## Installation

Install and usage docs live at https://cua.ai/docs/how-to-guides/driver/install
and https://cua.ai/docs/reference/cua-driver/mcp-tools.

The wheel contains generated UniFFI bindings, a platform-specific Rust SDK
library, and the `cua-driver` executable. `CuaDriver.create()` loads the runtime
in the importing process and does not require the executable or daemon.

## SDK example

```python
import asyncio

from cua_driver import (
    CaptureScope,
    CuaDriver,
    EndSessionInput,
    GetDesktopStateInput,
    StartSessionInput,
)

async def main() -> None:
    driver = CuaDriver.create()
    await driver.start_session(
        StartSessionInput(session="demo", capture_scope=CaptureScope.DESKTOP)
    )
    try:
        desktop = await driver.get_desktop_state(
            GetDesktopStateInput(session="demo", screenshot_out_file=None)
        )
        print(desktop.images[0].mime_type)
    finally:
        await driver.end_session(EndSessionInput(session="demo"))
        await driver.shutdown()


asyncio.run(main())
```

SDK operations are asynchronous. Desktop calls return a typed `ToolResult` with
text, images, verification/error metadata, and `structured_json` / `raw_json`
for platform-extensible results. Session lifecycle calls return dedicated
generated records.

`CuaDriver.connect(socket_path)` remains available while existing applications
migrate. It exposes the same methods over the installed daemon, but it does not
provide a second SDK contract.

`shutdown()` closes admission, waits for already admitted operations to finish,
and is idempotent. Calls started after shutdown fail with `DriverError.Shutdown`.
Destroying a binding handle releases native resources, but orderly applications
should still await `shutdown()`.

## Daemon-backed MCP host

Applications that must also expose MCP to an external agent can own a private
daemon child. The child provides a stable permission identity and session
lifetime for short-lived or external clients:

```python
import asyncio

from cua_driver import CuaDriver, EmbeddedCuaDriverHost, get_binary_path


async def main() -> None:
    host = EmbeddedCuaDriverHost(
        binary_path=str(get_binary_path()),
        host_bundle_id="com.example.your-app",
    )
    connection = await host.start()
    driver = CuaDriver.connect(connection.socket_path)
    try:
        # Application calls use driver. An agent runtime can launch
        # connection.mcp.command with connection.mcp.args and environment.
        print(await driver.metadata())
    finally:
        del driver
        await host.stop()


asyncio.run(main())
```

`start()` coalesces concurrent callers, `stop()` cancels startup and is
idempotent, and `restart()` returns a new generation/PID/endpoint. Destroy SDK
clients and MCP proxies before stopping or restarting, then reconnect from the
new connection. `wait_for_exit(connection.generation)` observes unexpected
termination. Dropping the host closes its parent-liveness pipe and kills the
child as a fallback, but orderly applications should still await `stop()`.

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
