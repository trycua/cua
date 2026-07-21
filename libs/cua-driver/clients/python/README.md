# Experimental cua-driver Python client

This package is an unpublished reference client generated from
`libs/cua-driver/contract/manifest.json`. It speaks MCP over a spawned
`cua-driver mcp` stdio process and expects the platform daemon to be available.

The native cua-driver process remains the execution and policy boundary. This
client does not approve permissions, escalate capture scope, or infer whether a
GUI action succeeded.

```python
from cua_driver_client import CuaDriverClient, GetDesktopStateArgs, StartSessionArgs

with CuaDriverClient.stdio() as client:
    result = client.start_session(StartSessionArgs("demo", capture_scope="auto"))
    desktop = client.get_desktop_state(GetDesktopStateArgs(session="demo"))
    print(desktop.images[0].mime_type)
```

Async applications such as `computer-server` should use the native asyncio
transport rather than running the blocking client in an executor:

```python
from cua_driver_client import AsyncCuaDriverClient, GetDesktopStateArgs

async with AsyncCuaDriverClient.stdio() as client:
    desktop = await client.get_desktop_state(GetDesktopStateArgs(session="demo"))
```

All stdio transports perform the MCP initialize handshake and enforce bounded
request timeouts. Tool actions are never retried automatically.

The package and generated API are experimental and must not be published from
this draft branch.
