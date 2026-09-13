# Typed Driver envelopes over MCP

This opt-in candidate implements the [accepted RFC 2512 transport addendum](../../../rfcs/2512-mcp-envelope-carrier.md).
It is not a released SDK feature or a qualification of an existing Fleet image.
The extension carries the canonical Driver envelopes through an existing MCP
transport. It does not replace computer-server, add agent tools, or open a port.

## Runtime ownership and opt-in

Set `CUA_DRIVER_MCP_ENVELOPES=1` in the trusted launcher. An absent value or `0`
keeps the existing MCP implementation. Other values fail startup.

For direct stdio, the MCP process owns the runtime. For an explicitly selected
daemon, both the proxy and daemon must opt in. The proxy requests an envelope
stream on the existing authenticated local socket or named pipe. The daemon
creates and owns the typed sessions. The proxy gains no trusted-session binding
authority. An old or disabled daemon refuses the upgrade; the proxy does not
fall back to another protocol or runtime.

The receiver uses `CUA_DRIVER_ENVELOPE_PERMISSION_MODE`, with Standard as the
default. Unrestricted requires the existing explicit unrestricted host
acknowledgement as well as launcher selection. Capability-manifest hosts remain
unsupported by this first receiver slice. A request cannot select permissions,
an endpoint path, or an independent trusted session.

The same stateful MCP HTTP wrapper can carry the stdio stream. A deployment
still needs its existing authentication boundary and a verified wrapper logging
policy. Do not log raw action arguments or results. This change does not enable
the extension in any image, expose a public unauthenticated listener, or require
the separate private HTTP receiver listener.

## Wire contract for this candidate

`initialize` adds the following capability without changing the ordinary tools:

```json
{
  "capabilities": {
    "experimental": {
      "ai.cua.driver.envelopes": { "version": 1 }
    }
  }
}
```

Clients must verify this exact extension before opening a typed receiver. The
namespaced methods are JSON-RPC requests with acknowledged responses, not
`tools/call` names or cancellation notifications.

| Method                   | Parameters                                  | Result                                                                                 |
| ------------------------ | ------------------------------------------- | -------------------------------------------------------------------------------------- |
| `cua/driver/v1/open`     | `{}`                                        | Existing receiver open result: connection ID, generation, public session, capabilities |
| `cua/driver/v1/exchange` | `connection_id`, `generation`, `envelope`   | Existing `DriverResponseEnvelope`                                                      |
| `cua/driver/v1/cancel`   | `connection_id`, `generation`, `request_id` | `{"ok":true}` after recording cancellation                                             |
| `cua/driver/v1/close`    | `connection_id`, `generation`               | `{"ok":true}` after invalidating the receiver                                          |

IDs and generations are receiver-generated UUIDs, not credentials. Parameters
reject unknown fields. Requests use JSON-RPC 2.0 with string or integer IDs.
An active outer ID must not be reused: ambiguity ends the stream. The receiver
also refuses duplicate inner action IDs without replaying the action.

Malformed parameters return `-32602`. Receiver service failures map to
`-32000 - status`, such as `-32404` for a foreign/missing connection and
`-32409` for a stale generation. Transport saturation returns `-32029`.
Messages contain bounded reasons, not raw request bodies.

## Lifecycle and limits

Each accepted transport owns a separate receiver registry. Another transport
cannot use its connection IDs. The existing receiver supplies real generations,
deadlines, request ledgers, cancellation, session binding, and permission checks.
There is no action replay or reconnect after a replaced daemon or guest.

Open, cancel, and close do not wait behind pending exchanges. The stream limits
pending action requests to 32 and input lines to 1 MiB. The receiver limits
connections to 64, exchanges to 32, and serialized envelope results to 16 MiB.
Writes have a 10-second bound including lock wait. Idle receiver entries expire
after five minutes without active work, checked at least every 30 seconds.

EOF, output failure, or task cancellation invalidates owned receivers before
aborting transport work. Closing a proxy stream does not shut down a shared
daemon. Direct stdio still shuts down its owned runtime. Interrupted native
effects can outlive the request: cancellation is not rollback, and unknown
completion remains unknown. Closing the MCP HTTP session is not Fleet claim or
pool cleanup.

## Verification and release gate

Run the focused deterministic tests from `libs/cua-driver/rust`:

```sh
cargo test -p cua-driver --bin cua-driver mcp_envelope --locked
cargo test -p cua-driver --bin cua-driver driver_service_http --locked
cargo test -p cua-driver --bin cua-driver proxy --locked
```

Before image enablement, separately prove the generated SDK carrier, real
Linux/Windows guest effects, concurrent session isolation, cancellation,
replacement, computer-server compatibility, and owned-resource cleanup. A
successful wrapper handshake or these headless tests do not prove desktop
behavior. Keep existing images and connection defaults unchanged until that
qualification and the separate rollout approval are complete.
