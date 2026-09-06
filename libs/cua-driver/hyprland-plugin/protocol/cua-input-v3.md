# Local input protocol v3 candidate

This opt-in candidate connects Driver's existing per-action admission to two
independent compositor seats. It is not native certification or a release
announcement. The default plugin build remains discovery-only, and discovery
protocol v2 is unchanged.

The current trace build contains a temporary drag-completion diagnostic: it
delays pointer-leave and the final reply by at least 100 ms after releasing the
button. The consumed grant admits no further input; conflict and lifecycle
checks remain active until cleanup. This compares Inkscape event-queue behavior
with immediate teardown. It is not a client-processing guarantee and is absent
from the uninstrumented production build. Trace results with this diagnostic
cannot certify production drag behavior. Resolve/remove the diagnostic before
the final release candidate is certified.

Driver completes common permission, resource, lifecycle, and application
compatibility checks before each action. The plugin trusts the desktop account
under a local trust model. Same-UID transport checks prevent accidental
cross-account use; they do not authenticate human consent or contain hostile
code running as that user. There is no external signer, `APPROVE` command,
challenge, or separate per-window permission workflow in v3.

## Transport and lane ownership

The private Hyprland instance directory contains `cua-input-v3.sock` and
`cua-input-v3-2.sock`. Each endpoint uses Linux `SOCK_SEQPACKET`, checks
`SO_PEERCRED` for the compositor UID, and owns one lane. Socket permissions are
`0600`; the instance directory must be owned by that user and private. Driver
also verifies the peer against the selected compositor instance.

Packets contain printable ASCII fields separated by exactly one space, with
no newline. The maximum packet size is 2048 bytes. Responses are JSON packets.
Unknown commands, extra fields, integer overflow, nonfinite coordinates, and
out-of-range values refuse. Decimal integers are unsigned; the target address,
epoch, and target token are hexadecimal. Epochs and tokens contain 32 lowercase
hexadecimal characters.

Each socket accepts at most eight connections. A connection must send `HELLO`
within five seconds and must not remain idle for more than 60 seconds. A
successful `CLAIM` reserves the endpoint for that connection until EOF,
timeout, or a desktop/configuration lifecycle transition. Repeated `CLAIM` on
the same connection is idempotent. Driver may try the second endpoint only
after an explicit `lane_busy` reply, before target selection or dispatch.
Connection failures and unknown delivery results do not permit retries.

The v3 seats are `Cua-Agent` and `Cua-Agent-2`. Driver excludes these from its
foreground virtual-input routes. Seat ownership does not come from public
session labels. The signed experiment uses separate sockets, protocol 0, and
its original seat names.

## Requests and responses

The request sequence for every Driver-admitted action is:

1. Send `HELLO` once per connection. The response is
   `{"ok":true,"protocol":3,"epoch":"<epoch>"}`.
2. Send `CLAIM`. The response is `{"ok":true,"lane":0}` or lane `1`.
3. Send `TARGET <pid> <hex-address> <capability>`. The capability is exactly one
   of `1` (click), `2` (key), `4` (scroll), or `8` (drag). Combined masks refuse.
   The response includes `ok`, `target`, `revision`, `width`, and `height`.
4. Send the one matching bounded operation using that token and revision.
   A subsequent action requires a fresh `TARGET`, even on a cached connection.

`TARGET` binds the exact live native top-level surface, generates a fresh token,
and grants at most five seconds of steady-clock technical lifetime. It first
retires any unused previous grant. It refuses unavailable desktops, primary
focus on the target's Wayland client, another lane targeting that client, and
unsupported keymaps. A discovered address is an input to attestation, not a
surface lifetime token. Surface unmap, destruction, or replacement invalidates
the binding. Geometry changes increment the revision and refuse stale actions.

The complete operation requests are:

| Request | Limits |
| --- | --- |
| `CLICK <sequence> <target> <revision> <x> <y> <button> <count>` | Evdev buttons 272–274; count 1–2. |
| `KEY <sequence> <target> <revision> <key> <modifiers>` | Evdev key 1–247 except lock keys 58, 69, and 70. Modifier bits: shift=1, ctrl=2, alt=4, super=8. |
| `SCROLL <sequence> <target> <revision> <x> <y> <axis> <value>` | Axis 0=vertical, 1=horizontal; nonzero value in [-1000,1000]. |
| `DRAG <sequence> <target> <revision> <x1> <y1> <x2> <y2> <duration_ms>` | Left button; duration 50–2000 ms; the grant must cover the duration plus 50 ms. |

Coordinates are finite logical surface-local values, with `0 <= x < width`
and `0 <= y < height`. Subsurface hits refuse. Sequence numbers increase
strictly across the connection, including fresh target selections. Repeated
or lower sequences return `replay`; no operation is replayed automatically.

The plugin repeats target, geometry, desktop, keymap, and conflict checks at
dispatch. It consumes the grant before the first synthetic focus/input event.
Complete synchronous operations release synthetic focus immediately. A drag
keeps its existing bounded lifetime while running; consuming its grant does
not permit another action. A dispatch without fresh authority returns
`action_not_admitted`. Malformed or refused requests never imply application
rollback.

Successful dispatch returns
`{"ok":true,"effect":"unverifiable","route":"synthetic_events"}`. This
acknowledges synthetic delivery, not an application outcome. A drag first
returns `{"ok":true,"phase":"started"}` and later its final result. After
that first acknowledgement, cancellation can leave partial application
effects. EOF or a missing final acknowledgement means delivery is unknown;
Driver must not report that nothing happened or replay the operation.

Driver maps a refusal before an acknowledged start to `effect:refused`, with
no delivery field. After a drag-start acknowledgement, an explicit cancellation
maps to `effect:partial` with `delivery.mode:background` and
`delivery.delivered_count:1`. This count measures acknowledged gesture phases
(the start), not pointer events or application changes. A lost or malformed
final reply preserves that count but sets `delivery.mode:unknown`; later input
may have landed without acknowledgement. A missing initial reply has unknown
delivery with no count. Neither case permits replay.

Refusals use `{"ok":false,"code":"<code>","detail":"<code>"}`. Codes
include `lane_busy`, `lane_not_claimed`, `stale_target`, `stale_geometry`,
`session_unavailable`, `unsupported_layout`, `primary_target_busy`,
`agent_target_busy`, `lease_expired`, `action_not_admitted`, `lease_busy`,
`client_not_bound`, `replay`, `unsupported`, and `invalid_request`.

## Cancellation and recovery

`CANCEL` and `STOP` both require the connection's lane claim. They revoke only
that endpoint's pending/current action, invalidate its target, and release its
synthetic held state. They preserve the reservation and other lane. A drag
receives its cancellation result before the command acknowledgement. A new
unclaimed control connection cannot cancel a runtime's lane. V3 has no global
socket stop command; plugin disable or unload explicitly stops both lanes.

These commands belong to the private plugin wire, not the public MCP tool
surface. Likewise, EOF below means the owned plugin connection closes. Closing
MCP stdin is not necessarily immediate plugin EOF: direct stdio finishes its
current request before reading the next request or EOF. See the
[Driver lifecycle boundary](host-authority-boundary.md#cancellation-and-recovery)
for the separate cancellation paths and transport limits.

EOF cancels only the departing connection's work and frees its reservation.
Lock/unlock, DPMS, session activity, and monitor transitions revoke authority.
Keymap/layout changes revoke authority, including synchronous layout and
active-keyboard keymap notifications. Changing primary focus to a target
client cancels that lane synchronously. Dispatch and timer checks supplement
these listeners. None of these paths wake the display or unlock the session.

Config disable closes input transports but preserves client-owned seats and
resources. Re-enable opens fresh transports with new epochs. Each later
action must pass fresh Driver admission and target checks. Plugin replacement
requires a desktop restart. The shared seat-lifetime marker rejects loading
replacement modules in the same compositor instance. Unload retires globals
and retains inert callbacks for late client cleanup.

Cancellation releases synthetic state; it does not undo application effects.
Cleanup depends on a responsive compositor event loop. A stall is not evidence
of bounded cleanup latency.

## Compatibility and build gates

Driver restricts app/package/version/operation eligibility before connecting.
The initial qualification scope is native Calc and Inkscape. The plugin checks
native surface identity, geometry, client conflicts, and exact compiled XKB
content against the default `evdev`/`pc105`/`us` keymap. Variants, options,
remaps, multiple groups, missing keyboards, XWayland, Unicode, IME input,
arbitrary held-key streams, and modified pointer gestures are outside this
candidate. Layout names alone never establish eligibility. Existing semantic
Driver routes retain their own behavior.

Build against the exact Hyprland 0.56.2 ABI with `CUA_HYPRLAND_INPUT=ON`.
This option defaults to `OFF` and is mutually exclusive with
`CUA_HYPRLAND_TEST_INPUT`. The production build does not link an external
signer or OpenSSL. `CUA_HYPRLAND_INPUT_TRACE=ON` adds test instrumentation for
certification; it defaults to `OFF` and requires v3 input. Uninstrumented
production does not expose `TRACE_START`, `TRACE_STOP`, or `TRACE_READ`.

Portable grant, lifecycle, discovery, and transport tests do not prove native
seat delivery or keymap qualification. Certification must build the actual
native implementation and verify each supported app/operation, both lanes,
independent primary interaction, stale/refusal paths, partial delivery,
cleanup, and surviving-client recovery at the exact candidate SHA. Include
both instrumented evidence and an uninstrumented production-package smoke.

The independent-seat design adapts Dillon DuPont's Hyprland prototype. Earlier
signed-experiment results remain historical evidence for that source only.
