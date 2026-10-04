# Typed Driver access from a sandbox

This developer contract describes the candidate implementation, not a released
setup guide. It needs the matching generated `cua-driver` package and native
library with `connect_remote_channel`, plus a guest carrier: cua-spacesd's
`/mcp` route with the [typed-envelope MCP extension](../../../cua-driver/docs/mcp-envelope-carrier.md)
(the default), or a Fleet service implementing the
[private envelope carrier](../../../cua-driver/docs/private-envelope-http.md).
Do not infer support from an image containing an older Driver or an MCP server.
No published package version or image is qualified by this document.

## Keep the existing responsibilities

Sandbox owns provisioning, claims, shell commands, files, and cleanup. Driver
provides its canonical typed desktop interface. The existing Sandbox methods
(served by cua-spacesd through the cua SDK) remain available; connecting
Driver does not replace them. MCP remains a separate agent-harness integration.

The accessor supports Fleet and cua-spacesd (`EnvTransport`) sandboxes, with
no fallback between carriers:

| Call | Carrier |
|---|---|
| `connect()` | Fleet `driver` service if the claim publishes one, else spacesd `/mcp` |
| `connect(service="env", transport="mcp")` | spacesd `/mcp` via the cua SDK env client (Fleet, direct, local) |
| `connect(service="driver")` | Fleet private envelope HTTP service |
| `connect(service="mcp", transport="mcp")` | Fleet named MCP service |

Fleet carriers use the sandbox's authenticated, namespace-authorized
named-service route. A guest carrier must remain behind that route or the
spacesd token; its loopback listener does not authenticate callers.
Connection generations and public session names are lifecycle metadata, not
credentials or proof of guest-verified claim authorization. There is never a
fallback to the caller's own desktop.

## Use the canonical typed interface

Given an already configured pool exposing the candidate `driver` service, the
proposed usage is:

```python
from cua_driver import GetAgentCursorStateInput, GetScreenSizeInput


async def inspect_guest(pool):
    async with pool.claim() as sandbox:
        async with sandbox.driver.connect(service="driver") as driver:
            size = await driver.get_screen_size(GetScreenSizeInput(session=None))
            session = sandbox.driver.session_name(driver)
            cursor = await driver.get_agent_cursor_state(
                GetAgentCursorStateInput(session=session)
            )
            return size, cursor
```

`driver` is the generated `cua_driver.CuaDriver`, not a parallel desktop API.
Use `session=None` for optional session fields. For required session fields,
`session_name(driver)` returns the active connection's host-bound label. The
carrier creates a Standard session by default. Only the trusted launcher can
opt into Unrestricted mode, with an explicitly acknowledged Unrestricted daemon;
remote clients cannot select session authority. Bounded mode and carrier manifest
configuration are unsupported, as are remote permission grants, new trusted
sessions, and session rebinding.

## Failure and cleanup behavior

Missing dependencies, disconnected transports, missing services, and unsupported
transport types fail explicitly. Invalid, stale, or uncertain responses
invalidate the connection. The accessor does not automatically reconnect or
replay an action whose completion is unknown. Connections belong to the event
loop that created them.

Exiting the Driver context closes that remote session, not the shared daemon,
spacesd, claim, or pool. Sandbox disconnect, release, and destroy close
Driver connections before their transport. Cleanup is best effort with bounded
waits; an unconfirmed cleanup logs a warning and does not block claim release.
The carrier's independent session expiry remains the fallback for unreachable
cleanup. A successful local close is not proof that a remote guest was deleted.

Opening handshakes do not hold the accessor's lifecycle lock. Closing the
accessor invalidates pending opens and bounds the wait for them, so a stalled
handshake cannot indefinitely delay claim release. If a cancelled open later
returns a connection ID, the accessor attempts to delete it without yielding a
Driver. Deletion can remain unconfirmed if the owning transport has already
disconnected or the handshake never returns its connection ID.

Each Driver exchange uses the envelope's remaining deadline, capped at 120
seconds, for its per-request Fleet timeout and local wait. This does not change
the transport's default timeout for other callers. Expired
requests fail before dispatch. A locally timed-out or cancelled exchange
invalidates the connection and attempts bounded cancellation and deletion; it
does not replay the desktop action.

## Verification and remaining proof

The focused tests cover named-service routing, canonical method dispatch,
response validation, cancellation, and lifecycle ordering. CI requires the
native bridge integration test through `CUA_SANDBOX_REQUIRE_NATIVE_DRIVER=1`;
it fails instead of skipping when the matching native package is absent.
Regression tests also cover pending-open cleanup, late-open invalidation,
per-request deadlines, and preservation of ordinary transport timeouts.

These synthetic tests do not prove a real guest desktop effect. Before a
released tutorial or supported-image claim, qualify the exact candidate on a
Fleet guest: private service routing, observed desktop postcondition,
coexistence with the spacesd interfaces, stale-connection rejection, and
resource cleanup.
Local guest support needs a separately verified runtime binding and must never
resolve a guest request to the caller's desktop.
