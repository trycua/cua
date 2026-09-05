# Isolated-input compatibility experiment

This is a nonshipping implementation spike for [RFC #3550](https://github.com/trycua/cua/issues/3550),
not its accepted production protocol. The normal build remains discovery-only.
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

An additional `cua-input-experiment.sock` in the private compositor instance
directory uses same-UID `SOCK_SEQPACKET`, bounded packets, nonblocking I/O, and
event-loop dispatch. It does not change discovery protocol v2. Requests are
ASCII fields separated by single spaces; responses are bounded JSON packets.
Integers are decimal except address, nonce, epoch, target, and signature, which
are hexadecimal. Unknown commands, extra fields, overflow, and nonfinite or
out-of-bounds coordinates refuse without input. There is one active lease.

- `HELLO`: negotiate experimental protocol 0; return `epoch` and connection
  `challenge`. A reconnect gets a new challenge.
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
  one complete left-button drag, 50–2000 ms, scheduled in bounded steps.
- `STOP`: revoke the active lease and release only synthetic-seat state.

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

## Evidence required

Test the exact source/module/Driver pair on the selected VM. Prove target
effects through normal Driver MCP calls and independent application state,
while observing primary pointer focus, keyboard focus, coordinates, and a
foreground gesture. Exercise invalid grants, replay, expiry, disconnect,
target loss, geometry changes, and unload. GTK and Qt successes do not imply
Chromium/Electron compatibility, Unicode support, physical-host acceptance,
or production readiness.

The separate-seat design builds on Dillon DuPont's Hyprland prototype.
Input-resource lifecycle and operator authorization are being adapted for a
plugin; the prototype's earlier app results are not evidence for this build.
