# Local input protocol v3 candidate

This opt-in candidate connects Driver's existing per-action admission to two
independent compositor seats. The accepted extension adds a distinct exact-target
primary-seat foreground route. It is not native certification or a release
announcement. The default plugin build remains discovery-only, and discovery
protocol v2 is unchanged.

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
   `{"ok":true,"protocol":3,"epoch":"<epoch>","foreground_target":true}`
   for a production plugin implementing the foreground extension. Driver must
   require that explicit capability before using the foreground route.
2. Send `CLAIM`. The response is `{"ok":true,"lane":0}` or lane `1`.
3. For background input, send `TARGET <pid> <hex-address> <capability>`.
   For foreground input, send
   `FOREGROUND_TARGET <pid> <hex-address> <capability>`. The capability is exactly
   one of `1` (click), `2` (key), `4` (scroll), or `8` (drag); foreground selection
   also permits `16` (activate). Combined masks refuse.
   The response includes `ok`, `target`, `revision`, `width`, and `height`.
4. Send the one matching bounded operation using that token and revision.
   A subsequent action requires fresh route-specific target selection, even on
   a cached connection. The binding fixes the route; an operation cannot turn
   a background grant into a foreground grant.

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
| `ACTIVATE <sequence> <target> <revision>` | Capability 16; foreground bindings only. |
| `CLICK <sequence> <target> <revision> <x> <y> <button> <count>` | Evdev buttons 272–274; count 1–2. |
| `KEY <sequence> <target> <revision> <key> <modifiers>` | Evdev key 1–247 except lock keys 58, 69, and 70. Modifier bits: shift=1, ctrl=2, alt=4, super=8. |
| `SCROLL <sequence> <target> <revision> <x> <y> <axis> <value>` | Axis 0=vertical, 1=horizontal; nonzero value in [-1000,1000]. |
| `DRAG <sequence> <target> <revision> <x1> <y1> <x2> <y2> <duration_ms>` | Left button; duration 50–2000 ms; the grant must cover the duration plus 50 ms. |

Coordinates are finite logical surface-local values, with `0 <= x < width`
and `0 <= y < height`. Subsurface hits refuse. Sequence numbers increase
strictly across the connection, including fresh target selections. Repeated
or lower sequences return `replay`; no operation is replayed automatically.

The plugin repeats target, geometry, desktop, keymap, and conflict checks on
the compositor thread at
dispatch. It consumes the grant before the first synthetic focus/input event.
Completed background operations release their held buttons, keys, keyboard focus, and
remaining technical authority. A drag keeps its bounded lifetime while
running; consuming its grant does not permit another action. A dispatch without
fresh authority returns `action_not_admitted`. Malformed or refused requests
never imply application rollback.

Background pointer focus has a separate lifetime. After a successful background pointer operation,
the lane keeps passive pointer focus on that exact live surface. This internal
state is separate from the visible Driver overlay, whose idle fade and
session-end removal remain unchanged. Fresh
`TARGET` admission on the same surface and unchanged geometry preserves this
focus while issuing a new token and one-operation grant. A key action releases
its keyboard focus without removing a previously retained pointer focus.
Retaining pointer focus prevents an unnecessary `wl_pointer.leave` directly
after drag release; some clients coalesce motion and requeue the release.
There is no fixed completion delay or trace-only timing behavior.

Passive focus has no held input or authority, but still participates in
primary-client and inter-lane conflicts. Owner cancellation, EOF, and the
connection's idle timeout release held buttons and all keyboard state and
revoke authority without requiring pointer leave. Passive target references
are weak and independent of the transport owner; a disconnected owner retains
no lane reservation. Fresh admission may reuse unchanged focus on the same
live surface without inheriting old authority.

Because Driver claims a free lane before selecting its target, fresh valid
`TARGET` admission may retire matching hover on a different unreserved lane,
or a lane whose new claimant has never successfully bound a target. A bare
`CLAIM` reserves capacity; it does not adopt a previous owner's hover. This is
allowed only when that peer has no lease, drag, held input, keyboard focus,
capabilities, or remaining grant/expiry state. After a connection's first
successful `TARGET`, its reservation protects its hover even after STOP,
CANCEL, or target invalidation. Existing target owners and active peers still
cause `agent_target_busy`; ordinary dispatch and conflict checks never evict
them. This permits opposite-order reuse when both new connections reserve
their lanes before either selects a target, without retrying any input.

Target replacement, unmap, destruction, geometry change, primary-client
conflict, and desktop/keymap/configuration transitions clear passive focus.
The five-second action grant does not extend to passive focus. These boundaries
may still end focus before a stalled client processes its events; dispatch
acknowledgement is not an application-processing fence. Verify the application's
effect independently.

Successful background dispatch returns
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

## Exact-target foreground extension

`FOREGROUND_TARGET` binds an ordinary live native top-level surface to
`primary_foreground`. The plugin validates the exact PID, compositor address,
surface lifetime, geometry revision, and route at admission and dispatch on the
compositor thread. `ACTIVATE` intentionally activates that target. Foreground
click, key, scroll, and drag use the same bounded operation shapes with a
foreground binding; primary focus and cursor position may change. This route
makes no promise to restore focus or cursor position after delivery.

Before taking over primary input, the plugin must refuse held physical keys or
buttons, active grabs, pointer constraints, and drag-and-drop. It requires a
single primary seat binding, excluding its own agent seats by resource identity,
and refuses binding or input-resource changes during dispatch. Keyboard delivery
requires neutral primary modifiers and layout group zero; latched or locked
modifiers refuse before activation. Foreground drag
cancellation on primary-input and focus transitions remains subject to review
and native verification; do not infer background isolation from this route.
Foreground results use foreground delivery metadata. A drag cancellation after
its start is partial, and a missing acknowledgement is unknown, under the same
no-replay rules as background delivery.

Discovery or activation read-back followed by global `wtype` input does not
establish exact-target delivery and is not a fallback. Background refusal never
escalates to foreground. Both routes use Driver's existing permission and
lifecycle admission, without a separate approval UI.

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
active-keyboard keymap notifications. For background bindings, changing primary
focus to a target client cancels that lane synchronously. Dispatch and timer checks supplement
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

For background `TARGET`, Driver restricts app/package/version/operation
eligibility before connecting. Its qualification scope remains native Calc and
Inkscape. Foreground `FOREGROUND_TARGET` has no Calc/Inkscape package gate; its
scope is ordinary native top-level surfaces. Native GTK3, Electron, and Tauri
foreground coverage is planned and still requires certification. The plugin checks
native surface identity, geometry, client conflicts, and exact compiled XKB
content against the default `evdev`/`pc105`/`us` keymap. Variants, options,
remaps, multiple groups, missing keyboards, XWayland, Unicode, IME input,
arbitrary held-key streams, and modified pointer gestures are outside this
candidate. Layout names alone never establish eligibility. Existing semantic
Driver routes retain their own behavior.

Driver expands bounded ASCII text into complete key operations under the exact
US keymap. The protocol adds no text or IME stream. Background application
qualification is unchanged by this foreground extension.

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
The foreground extension must also pass the existing complete canonical Linux
Rust suite in native Hyprland at the candidate SHA, preserving its runner,
required cells, assertions, and evidence checks. No foreground certification
result is recorded here.

The independent-seat design adapts Dillon DuPont's Hyprland prototype. Earlier
signed-experiment results remain historical evidence for that source only.
