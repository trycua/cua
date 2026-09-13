# Isolated-input compatibility experiment

This is a nonshipping implementation spike for [RFC #3550](https://github.com/trycua/cua/issues/3550),
not its accepted production protocol. The normal build remains discovery-only.
The [host authority implementation boundary](host-authority-boundary.md)
records a disabled shared lifecycle primitive and the remaining production
bridge requirements; it does not replace this experiment's signer.
The experiment requires a separate compile-time opt-in and an Ed25519 public
key selected by the operator. Never install this build on an unreviewed host.

## Operator boundary

The disposable-VM test operator holds the private signing key outside the VM.
The agent has neither the key nor an approval tool. An operator signs a grant
only after selecting the pending connection, exact target, capabilities, and
expiry. A grant cannot be moved to another connection, target, or compositor
epoch. Unauthenticated Stop can only revoke, not grant. This harness-specific
boundary does not settle local production approval or defend against a
compromised compositor, root, or an unrestricted hostile desktop user.

## Transport under test

Two additional sockets, `cua-input-test.sock` and `cua-input-test-2.sock`, in
the private compositor instance directory use same-UID `SOCK_SEQPACKET`, bounded packets, nonblocking I/O, and
event-loop dispatch. It does not change discovery protocol v2. Requests are
ASCII fields separated by single spaces; responses are bounded JSON packets.
Integers are decimal except address, nonce, epoch, target, and signature, which
are hexadecimal. Unknown commands, extra fields, overflow, and nonfinite or
out-of-bounds coordinates refuse without input. Each endpoint owns one seat,
one active lease, and independent pointer, keyboard, XKB, and drag state.
The compositor reserves each endpoint for one action connection. Driver tries
the endpoints only during admission, before sending a target or action, and
only an explicit `lane_busy` reply permits trying the second endpoint.
Connection errors or unknown results do not trigger retries. This coordinates
independent Driver processes without sharing seat state. The process-local
registry still bounds and serializes private lifecycle owners; public session
labels do not grant a lane or input authority.

- `HELLO`: negotiate experimental protocol 0; return `epoch` and connection
  `challenge`. A reconnect gets a new challenge.
- `CLAIM`: reserve this endpoint for the connection, or return `lane_busy`.
  A successful response includes `lane` (zero or one). Repeating a claim on the
  same connection is idempotent. Target selection and input require a claim;
  operator and trace connections do not claim a lane. Disconnect frees it.
- `TARGET <pid> <address>`: attest a live native top-level and return `target`,
  `revision`, logical `width` and `height`. The address is discovery input,
  never a lifetime token. A recreated/unmapped surface invalidates its token.
- `APPROVE <challenge> <target> <expires_unix_ms> <capabilities> <signature>`:
  operator-only signed approval on a separate connection; lifetime at most
  60 seconds. Capability bits: click=1, key=2, scroll=4, drag=8.
- `CLICK <sequence> <target> <revision> <x> <y> <button> <count>`: one complete
  bounded click sequence. Buttons are evdev 272–274; count is one or two.
- `KEY <sequence> <target> <revision> <key> <modifiers>`: complete evdev key
  press/release with independent XKB state. Modifiers: shift=1, ctrl=2, alt=4,
  super=8. No Unicode, IME, clipboard, or arbitrary held-key stream.
- `SCROLL <sequence> <target> <revision> <x> <y> <axis> <value>`: one bounded
  axis event; vertical=0, horizontal=1, logical value in [-1000,1000].
- `DRAG <sequence> <target> <revision> <x1> <y1> <x2> <y2> <duration_ms>`:
  one complete left-button drag, 50–2000 ms, scheduled in bounded steps. An
  accepted drag first sends `{"ok":true,"phase":"started"}`, then its final
  delivery result or cancellation refusal. Driver starts the visible drag
  only after this acknowledgement; an animation is not delivery evidence.
- `CANCEL`: revoke this endpoint's active and pending grants and release its
  synthetic input state. Keep the lane reservation and control connections;
  a new action needs a fresh target token and approval.
- `STOP`: apply the same revocation to both endpoints. Previously signed but
  unused grants and renewals also refuse. Release only synthetic-seat state;
  operator and trace connections remain available.
- `TRACE_START`, `TRACE_STOP`, `TRACE_READ <after>`: explicitly start, stop,
  or page the test-only in-memory primary-input trace. Pages contain at most
  eight events; collection must retain sequence numbers and completeness flags.

The signed message is exactly the UTF-8 concatenation below, including the
final newline. Hex fields use lowercase. The signature is Ed25519 over these
bytes, represented as 128 hexadecimal digits.

```text
CUA_TEST_LEASE_1
<epoch>
<challenge>
<target>
<expires_unix_ms>
<capabilities>
```

Sequence numbers increase strictly on the connection; duplicate or old
sequences refuse instead of replaying an action. Dispatch acknowledgement is
`effect: unverifiable`, `route: synthetic_events`; application success needs
independent readback. Disconnect, expiry, Stop, lock, target loss, or plugin
shutdown cancels delivery. Primary input aimed at the same target refuses.

## Build and lifecycle limits

Build with `CUA_HYPRLAND_TEST_INPUT=ON` and
`CUA_HYPRLAND_TEST_OPERATOR_KEY=<64 lowercase hex characters>`, against
Hyprland 0.56.2 and its matching compiler/runtime. Driver additionally requires
`CUA_DRIVER_EXPERIMENTAL_HYPRLAND_INPUT=1`. The ordinary build and installer do
not enable input. This experiment adds `Cua-Test-Agent` and `Cua-Test-Agent-2`;
Driver excludes both from foreground virtual-pointer/keyboard routes.

Disabling input revokes leases and closes input connections, but keeps both
seat globals and client-owned resources. Re-enabling opens fresh transports
with new epochs; existing applications keep their seats and need fresh input
approval. Keymap replacement refreshes independent XKB state and keyboard
resources, and revokes old authority without replacing the seats.

Plugin replacement requires a desktop restart. Unload revokes input, disables
seat capabilities, removes the globals, and retains inert protocol objects
and `NODELETE` callbacks for late client cleanup. A marker in the private
compositor-instance directory refuses replacement modules for that desktop
lifetime, including a different module filename. It is a trusted-local
lifecycle guard, not a boundary against a hostile same-user process. The old
eight-reload workaround is no longer the upgrade contract. A new compositor
instance gets its own directory and new seats.

Synchronous lock/unlock, DPMS, session-active, and monitor-transition listeners
revoke active and pending action connections even when a state returns to its
original value between timer ticks. Operator and trace connections remain
available. Dispatch-time state checks remain a second guard. These listeners
cover stock Hyprland manager paths, not arbitrary third-party code bypassing
them. Input socket cleanup preserves a replaced socket, file, or symlink and
reports the cleanup outcome in lane status.

The first input slice supports exact native top-level click, physical key and
hotkey, scroll, and complete bounded drag. It refuses XWayland, child/subsurface
targets, modified pointer gestures, and raw Unicode/text/IME. The existing
AT-SPI text route is unchanged. A primary pointer or keyboard in the same
Wayland client as the target revokes the lease, even for another window of
that application. Concurrent leases aimed at the same Wayland client also
refuse. Closing a Driver lifecycle closes only its owned input connection;
public session labels do not let a new transport inherit an old lease.

The passive trace hooks the exact Hyprland 0.56.2 cursor-update function and
listens to primary focus signals and outbound pointer/keyboard event categories.
It records coordinates, monotonic timestamps, actor numbers, and button/key
state, but no keycodes, text, window names, or arbitrary protocol arguments.
Capture stops after 60 seconds or 32,768 events. Missing instrumentation,
overflow, sequence gaps, or timeout make evidence inconclusive. This observer
is intrusive test instrumentation, not a proposed production API or a
replacement for the existing cross-platform cursor observer. Calibrate it with
the deliberate warp-and-return control before relying on preservation results.

## Evidence required

Test the exact source/module/Driver pair on the selected VM. Prove target
effects through normal Driver MCP calls and independent application state,
while observing primary pointer focus, keyboard focus, coordinates, and a
foreground gesture. Exercise invalid grants, replay, expiry, disconnect,
target loss, geometry changes, config-toggle recovery, and unload/replacement
refusal. The historical unload/reload delivery test is not applicable to the
restart-required candidate. GTK and Qt successes do not imply
Chromium/Electron compatibility, Unicode support, physical-host acceptance,
or production readiness.

`tests/driver_input_live.py` runs the focused native GTK test through an actual
Driver MCP process. It needs the two drawing-area fixtures from
`tests/fixtures/apps/linux/isolated-input`, their journals and foreground wire
log, and the compiled `tests/primary_grab.c` adversary. The operator signs its
`grant-request.json` on a separate host and transfers only the public
`grant.json`. It checks application event deltas, unchanged primary wire
events, focus, workspace, cursor, a held foreground grab, and Stop refusal.
It does not replace the canonical desktop E2E harness.

The separate-seat design builds on Dillon DuPont's Hyprland prototype.
Input-resource lifecycle and operator authorization are being adapted for a
plugin; the prototype's earlier app results are not evidence for this build.
