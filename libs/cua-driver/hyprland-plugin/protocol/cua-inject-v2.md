# Cua Hyprland injection protocol v2

## Status and scope

This document defines the local protocol used by the optional Cua Hyprland
plugin. The monorepo copy is authoritative. The current v2 foundation exposes
status, negotiation, and liveness only. It advertises the `discovery`
capability and rejects every input message with typed
`background_unavailable`.

Target-bound input remains gated on the second-seat compatibility spikes and
acceptance of the governing RFC. The separate-seat design originates in Dillon
DuPont's Hyprland prototype; this records its lineage without claiming that the
prototype or mutation support has shipped in Cua Driver.

## Runtime discovery

The plugin publishes runtime metadata through:

```bash
hyprctl -j cua:status
```

The current JSON object contains:

| Field                              | Meaning                                                                                                                                                              |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`                             | `cua-hyprland-plugin`.                                                                                                                                               |
| `plugin_version`                   | Plugin package version.                                                                                                                                              |
| `state`                            | `discovery_only` in the current foundation.                                                                                                                          |
| `protocol.major`, `protocol.minor` | Wire protocol version.                                                                                                                                               |
| `protocol.max_frame_bytes`         | Maximum complete packet size.                                                                                                                                        |
| `compositor_epoch`                 | Nonzero identifier for the current live plugin transport binding, or zero when no transport is ready. It changes after disable/enable and plugin/compositor restart. |
| `abi.compiled_hash`                | Full Hyprland client ABI fingerprint from the build headers: compositor commit plus linked Hyprland library versions.                                                |
| `abi.runtime_hash`                 | Full ABI fingerprint exported by the running compositor.                                                                                                             |
| `abi.match`                        | Always true for a loaded instance; mismatch aborts load.                                                                                                             |
| `configured`                       | Whether `plugin:cua:enabled` is true.                                                                                                                                |
| `transport`                        | Socket readiness, path, permission policy, and last error.                                                                                                           |
| `capabilities`                     | Supported and currently enabled capability names.                                                                                                                    |
| `connections`                      | Bounded diagnostic counters with no application data.                                                                                                                |

The plugin throws during initialization when the build and runtime Hyprland
hashes differ. In that case no command, socket, or capability remains
registered.

## Transport and peer authorization

The transport is a Unix-domain `SOCK_SEQPACKET` socket at
`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/cua-inject-v2.sock`. It is
created only when `plugin:cua:enabled` is true.

Before binding, the plugin verifies that `XDG_RUNTIME_DIR`, its `hypr`
directory, and the current instance directory pathnames resolve to directories
owned by the compositor UID with no group or other permission bits; each of
those three final pathnames is rejected when it is itself a symlink. The final
socket inode is mode `0600`.

Immediately after `accept4`, the server reads `SO_PEERCRED`. The peer UID must
equal the compositor process UID. Missing credentials or a UID mismatch closes
the connection before a request is processed. This authenticates the local Unix
user but does not distinguish Cua from another process owned by that user.
Filesystem permissions are defense in depth, not authorization. The current
transport exposes no mutation. Before mutation capabilities can be enabled, the
governing RFC must define operator authorization, credential or lease lifetime,
revocation, and replay-resistant binding to the compositor epoch.

The server uses a dedicated bounded I/O thread. That thread never calls
Hyprland or Wayland APIs. It accepts at most eight concurrent clients and one
packet per ready client per polling pass. A client must send `HELLO` within five
seconds or the server closes it, preventing silent clients from holding all
connection slots. A negotiated client must send traffic such as `PING` at least
once per minute; the server closes an idle connection after 60 seconds. A wake
event interrupts the I/O poll so shutdown can join the worker thread promptly.
Mutation work, when added, must cross a separate bounded queue and execute on
the compositor event loop.

The server refuses a pre-existing socket path and records the device and inode
of the path it bound. On shutdown it unlinks only when the current path still
matches that identity. It never replaces or unlinks an unowned object.

## Packet envelope

Every `SOCK_SEQPACKET` packet contains one binary frame in network byte order.
There is no stream reassembly and no JSON parser on the compositor boundary.

| Offset |     Size | Field                                |
| -----: | -------: | ------------------------------------ |
|      0 |        4 | ASCII magic `CUA2`                   |
|      4 |        2 | Protocol major, currently `2`        |
|      6 |        2 | Protocol minor, currently `0`        |
|      8 |        2 | Message type                         |
|     10 |        2 | Flags, currently required to be zero |
|     12 |        8 | Client-selected request ID           |
|     20 |        4 | Payload length                       |
|     24 | variable | Message payload, at most 4096 bytes  |

The maximum complete frame is 4120 bytes. The receiver rejects bad magic,
unsupported versions, nonzero flags, oversized payloads, and any packet whose
actual size does not exactly match the declared payload length.

## Handshake and liveness

`HELLO` (`0x0001`) must be the first message. Its 16-byte payload contains a
requested capability bitset followed by a required capability bitset. A
required capability that is not enabled returns `capability_unavailable` and
does not establish the session.

Successful negotiation returns `WELCOME` (`0x0002`) with this 32-byte payload:

| Offset | Size | Field                                       |
| -----: | ---: | ------------------------------------------- |
|      0 |    8 | Compositor epoch                            |
|      8 |    8 | Capabilities supported by this server build |
|     16 |    8 | Capabilities enabled for this session       |
|     24 |    4 | Maximum complete frame size                 |
|     28 |    4 | Reserved zero                               |

After negotiation, `PING` (`0x0003`) receives a correlated empty `PONG`
(`0x0004`). `STATUS_REQUEST` (`0x0005`) receives `STATUS_RESPONSE` (`0x0006`)
with the same payload shape as `WELCOME`.

The epoch is regenerated whenever the transport socket is created. Clients
must discard cached state after disconnect and accept target or capability
state only after a new `HELLO`/`WELCOME` exchange.

## Capabilities and mutation messages

Capability bits are stable within protocol v2:

| Bit | Name             | Current behavior                               |
| --: | ---------------- | ---------------------------------------------- |
|   0 | `discovery`      | Negotiation, status, and liveness are enabled. |
|   1 | `pointer_motion` | Not enabled.                                   |
|   2 | `pointer_button` | Not enabled.                                   |
|   3 | `pointer_axis`   | Not enabled.                                   |
|   4 | `pointer_drag`   | Not enabled.                                   |
|   5 | `keyboard_key`   | Not enabled.                                   |
|   6 | `keyboard_text`  | Not enabled.                                   |
|   7 | `observation`    | Not enabled.                                   |

The defined mutation message types are `0x0100` through `0x0103` and `0x0110`
through `0x0111`. The current server returns `background_unavailable` for each
of them. Other values in the reserved `0x0100` through `0x0111` range remain
undefined and return `malformed_frame`, as do server-only message types received
from a client. The server never reinterprets a request, changes focus, selects
the focused window, uses pointer-position routing, or falls back to the primary
seat.

Future target-bound mutation payloads must include the compositor epoch and an
opaque target token. Tokens must not expose a pointer, object address, Wayland
object ID, PID, or reusable ordering. A token becomes invalid immediately when
its target is destroyed or the epoch changes.

## Error payload

`ERROR` (`0xffff`) contains a four-byte numeric error code followed by a bounded
UTF-8 detail string. Detail is diagnostic only; clients branch on the code.

| Code | Name                     | Meaning                                           |
| ---: | ------------------------ | ------------------------------------------------- |
|    1 | `malformed_frame`        | Framing or payload validation failed.             |
|    2 | `unsupported_version`    | The major/minor pair is not supported.            |
|    3 | `handshake_required`     | A message arrived before `HELLO`.                 |
|    4 | `capability_unavailable` | A required capability is not enabled.             |
|    5 | `permission_denied`      | The authenticated peer lacks permission.          |
|    6 | `stale_epoch`            | A future target/action references another epoch.  |
|    7 | `server_busy`            | A bounded connection or work budget is exhausted. |
|    8 | `background_unavailable` | Target-bound mutation is unavailable.             |

Connection loss and `server_busy` are transport outcomes, not proof of
application behavior. Even a future successful mutation result will mean only
that the compositor completed its defined dispatch path. Cua Driver and the
desktop harness must use independent application-state, focus, z-order,
workspace, cursor, and input-leak oracles.

## Versioning

Changes to framing, peer authorization, epoch or token lifetime, result
meaning, or operation semantics require a new protocol major version. Additive
capabilities may remain in v2 only when their absence is safe and every client
must explicitly negotiate them. Neither side may downgrade silently.
