# Typed Driver through an existing Fleet connection

This candidate shares the typed MCP client in Rust. UniFFI exposes the same
Driver contract to Python and TypeScript. The language adapters forward HTTP
bytes through an existing authenticated Fleet client; they do not implement MCP.

The examples require candidate bindings and their matching native library. They
are not available in Driver 0.26.1. Do not install that release and expect these
entry points to exist. Package release and image qualification are separate
from this implementation.

## Requirements and ownership

- Keep the authenticated Fleet client and claim alive until Driver closes.
- Select a service advertised by the claimed sandbox, normally `mcp`.
- The guest must advertise `ai.cua.driver.envelopes` v1 on MCP protocol
  `2025-06-18`. An ordinary MCP tools endpoint is insufficient.
- The guest's trusted launcher owns permissions. Connecting does not elevate
  permissions or promise identical capabilities on all operating systems.
- The typed TypeScript SDK requires Node. This does not add a browser-native
  Driver SDK or change Fleet's browser entry point.

The application owns claim release and pool/template deletion. Closing Driver
closes only its receiver and MCP session, not the computer or shared daemon.
Always save needed output before deleting capacity. Cancellation does not undo
desktop actions; a lost response can leave completion unknown.

## TypeScript: Fleet plus Driver

There is no TypeScript Sandbox SDK. Given an authenticated Fleet `client` and
an application-owned `claim`, use the optional Driver entry point:

```typescript
import { connectFleetDriver } from '@trycua/cua-driver/fleet';

try {
  const sandbox = await client.waitClaim(claim);
  const connection = await connectFleetDriver({
    client,
    sandbox,
    service: 'mcp',
  });
  try {
    const result = await connection.driver.getScreenSize({ session: undefined });
    console.log(result.text);
  } finally {
    await connection.close();
  }
} finally {
  await client.deleteClaim(claim);
}
```

`connection.driver` is the canonical `CuaDriverLike`, not a list of MCP tool
wrappers. `connection.sessionName` provides the host-bound public label for
typed inputs that require a session. It is not a credential or permission grant.
An optional `signal` requests connection teardown; await `close()` to observe
its result before releasing the Fleet claim. No reconnect or action retry is
performed after cancellation, replacement, or an uncertain response.

## Python: direct Fleet use

The optional module imports Fleet only when constructing the adapter. Given an
authenticated Python Fleet client and an application-owned claim:

```python
from cua_driver import GetScreenSizeInput
from cua_driver.fleet import open_fleet_mcp_driver_channel

try:
    sandbox = await client.wait_claim(claim)
    channel = open_fleet_mcp_driver_channel(client, sandbox, service="mcp")
    try:
        await channel.open()
        driver = channel.driver()
        result = await driver.get_screen_size(GetScreenSizeInput(session=None))
        print(result.text)
    finally:
        await channel.close()
finally:
    await client.delete_claim(claim)
```

Constructing the channel before awaiting `open()` lets the owner close it even
if initialization is interrupted. The shared Rust implementation tracks late
initialization and cleanup. If cleanup cannot be confirmed within its bounds,
the connection reports that uncertainty rather than claiming guest deletion.

## Python Sandbox convenience

The separately staged Sandbox migration preserves its existing API:

```python
async with pool.claim(service="server") as sb:
    await sb.shell.run("uname -a")  # Still computer-server.
    async with sb.driver.connect(service="mcp", transport="mcp") as driver:
        result = await driver.get_screen_size(GetScreenSizeInput(session=None))
```

Sandbox closes its Driver connections before disconnecting the Fleet transport.
Its direct-envelope path and existing computer-server interfaces are unchanged.
The migration must not ship until a Driver release containing the shared client
is available and Sandbox's optional dependency and lockfile select that release.

## Validation scope

Synthetic service fixtures test Rust protocol handling and both generated
bindings without operating a desktop. They prove negotiation, typed calls,
session isolation, cancellation, malformed responses, and owned cleanup. They
do not qualify a Fleet image or replace Linux/Windows guest tests at the final
candidate revision. Existing published images and production routing are not
modified by this client.
