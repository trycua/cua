# Experimental cua-driver Python client

This package is an unpublished reference client generated from
`libs/cua-driver/contract/manifest.json`. It speaks MCP over a spawned
`cua-driver mcp` stdio process and expects the platform daemon to be available.

The native cua-driver process remains the execution and policy boundary. This
client does not approve permissions, escalate capture scope, or infer whether a
GUI action succeeded.

```python
from cua_driver_client import CuaDriverClient, StartSessionArgs

with CuaDriverClient.stdio() as client:
    result = client.start_session(StartSessionArgs("demo", capture_scope="auto"))
    print(result.structured)
```

The package and generated API are experimental and must not be published from
this draft branch.
