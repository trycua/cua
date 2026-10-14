# Presence: cursors, shapes and the datagram channel

`cua.env.v1.PresenceService` (see `cua/env/v1/presence.proto`) is the
authority for who is present. This document covers what the proto cannot:
timing, the cursor shape probe, staleness rules, and the optional QUIC
datagram channel for cursors.

## 1. Model

- **Your own cursor is local.** A client draws its own cursor at the local
  pointer with no network round trip. The server never echoes a sender's
  position back (`CursorMoved` goes to everyone else). Only the *shape* of
  your own cursor comes from the server (`CursorShapeChanged` is sent to the
  participant itself too).
- **Remote cursors are interpolated.** Clients buffer received positions and
  render them `interp_delay` in the past (§4), so jitter and one lost update
  are invisible.
- **Latest wins.** Cursor state is ephemeral: every update carries an
  absolute position, and a newer update replaces an older one. Nothing is
  retransmitted.

## 2. Rates

| What | Rate |
|---|---|
| Client send | newest position, at most every 33 ms, only when it changed; the final position of a movement is sent immediately; hide and show are never throttled |
| Server tick, `Join` stream | 50 ms (20 Hz) per cursor, checked every 10 ms so a move goes out as soon as its interval passes; nothing is sent without changes |
| Server tick, datagram channel | 33 ms (30 Hz); a change is sent at once when 25 ms have passed since the last datagram; plus a full-state keyframe every 1 s |
| `RosterHeartbeat` | `JoinRequest.roster_interval`; SDKs ask for 5 s |
| Shape probe | per participant at most every 100 ms, and only after the position left the last probed region |

With `cursor_batches`, one `CursorBatch` per tick carries every cursor that
moved in it.

## 3. Cursor shapes

Participants that join with `cursor_shapes` get `CursorPosition.shape` and
`CursorShapeChanged`. The server computes a shape per participant position,
cheapest first:

1. **System** (`CURSOR_SHAPE_SOURCE_SYSTEM`): the participant is the one whose
   input positioned the real pointer (their last injected input is recent
   and the pointer is still where it put it). The OS's real cursor is read:
   X11 XFixes cursor name, macOS `NSCursor.currentSystemCursor`, Windows
   `GetCursorInfo`.
2. **Probe** (`CURSOR_SHAPE_SOURCE_PROBE`): when the real pointer is idle (no
   injected input and no agent action in the last 750 ms, and none pending),
   and `PresenceSettings.cursor_probe` is on (the default), the server moves
   the pointer to the participant's position with no button state, waits
   ~40 ms, reads the real cursor, and moves it back to the exact previous
   position. Pending input aborts the probe and restores the pointer first;
   input and probes are serialized so they never interleave. Hover effects
   under the probe may flash briefly; the dwell is far below tooltip delays.
3. **Hit-test** (`CURSOR_SHAPE_SOURCE_HIT_TEST`): the accessibility element
   under the point (text -> I-beam, link or button -> hand, busy ->
   progress) and window edges (-> resize). Never moves the pointer. Used
   whenever 1 and 2 do not apply.

Results are cached per region (the element's frame, else a 16 px cell) for
1 s. A daemon without the capability "presence.cursor_shape" leaves shapes
unspecified, which clients draw as the arrow.

Per-OS support is listed in the capability's attributes (`hit_test`,
`system`, `probe`) and `limitation`; the cua-spacesd README has the table.

## 4. Client rendering

| Parameter | Value |
|---|---|
| Interpolation delay | 100 ms behind the newest sample on the 20 Hz stream, 66 ms on the 30 Hz datagram channel |
| Interpolation | Catmull-Rom through the buffered samples, linear with fewer than 4 |
| Extrapolation cap | 100 ms past the newest sample, then hold |
| Snap | a jump over 25% of the surface, or a target change, snaps instead of gliding |
| Idle fade | a cursor that has not moved for 5 s fades out over 300 ms; it reappears on the next move |
| Stale removal | a participant missing from a `RosterHeartbeat`, or with no heartbeat for 3 intervals, is removed |

Sample times are server timestamps (`CursorMoved.at` or the datagram's
`server_time_us`) mapped to the local clock by the minimum observed offset,
so network delay variance does not become motion jitter.

## 5. Staleness and leaving

- `ParticipantLeft` is sent when a `Join` stream ends (`DISCONNECTED`), on
  `Leave` (`LEFT`), when an agent without a stream has been idle for 15 s
  (`TIMEOUT`), and when the agent run or driver session that owned an agent
  cursor ends (`RUN_ENDED`, for example `DELETE /mcp` of a run's last session
  or the end of an `X-Cua-Agent-Session` run).
- A human cursor with no update for 60 s is broadcast hidden
  (`visible: false`), not removed.
- Clients drop any participant a heartbeat does not list, and fade idle
  cursors (§4), so no client keeps a dead cursor even if it missed a leave.
- cua-driver's own agent cursor overlay, drawn inside the guest, follows the
  same agent rules: it hides at once when the driver session that owns it
  ends (the same events that send `RUN_ENDED`), and fades out over 180 ms
  after 15 s without activity (the agent `TIMEOUT` above; both come from
  `cua_driver_core::agent_cursor::AGENT_CURSOR_IDLE_TIMEOUT`). Each session's
  cursor is independent: one ending or idling never hides another.

## 6. Datagram channel

Requested with `JoinRequest.cursor_datagrams`; described by
`PresenceJoined.datagrams`. Browsers keep using the `Join` stream and
`UpdateCursor`.

### 6.1 Connect

- QUIC to `endpoint.port` on the Space's host, ALPN `cua-presence/1`,
  certificate pinned by `endpoint.certificate_sha256` (the same listener and
  certificate as the media plane).
- The client opens one bidirectional stream and sends one JSON line:
  `{"type":"presence_ticket","payload":{"ticket":"…"}}`. An invalid or used
  ticket closes the connection with application error `0x401`.
- The server replies on that stream with JSON lines:
  - `{"type":"slots","payload":{"slots":{"<slot>":"<participant_id>",…},"you":<slot>}}`
    on attach and whenever the table changes;
  - `{"type":"cursor_target","payload":{"slot":N,"display_id":"…","window":{"id":"…","epoch":N}}}`
    when a participant's cursor target (display or window) changes. `window`
    is omitted for display targets.
- The client sends `{"type":"cursor_target",…}` on the same stream (with its
  own slot) before moving its cursor onto a new target.
- Closing the connection does not leave presence; the `Join` stream does.

### 6.2 Datagram

Big-endian. One datagram, at most 1200 bytes.

| Bytes | Field | Meaning |
|---|---|---|
| `0..4` | magic | `RPC1` |
| `4` | version | `1` |
| `5` | flags | bit 0 `keyframe`: the full cursor set. Other bits 0. |
| `6..10` | tick | u32 server tick (downlink) or client send counter (uplink) |
| `10..18` | server_time_us | u64 Unix microseconds on the server clock when it received the newest record in this datagram (downlink; the same clock as `CursorMoved.at`); 0 uplink |
| `18` | count | u8 number of records |
| `19..` | records | `count` x 9 bytes |

Record (9 bytes):

| Bytes | Field | Meaning |
|---|---|---|
| `0` | slot | u8 participant slot from `slots` |
| `1..3` | seq | u16 per-participant sequence, wrapping; a receiver drops a record whose seq is not newer (mod 2^16, within 32768) than the last it applied |
| `3..5` | x | u16, `round(x * 65535)` of the normalized position |
| `5..7` | y | u16, likewise |
| `7` | shape | u8 `CursorShape` value |
| `8` | state | bit 0 visible, bit 1 pressed, bits 2-3 `CursorShapeSource`, others 0 |

Uplink datagrams carry exactly one record, the sender's own slot; the server
ignores `shape` and source bits from clients and drops records for any other
slot. A malformed datagram is dropped and counted, never fatal.

Bandwidth: 19 + 9n bytes per tick; ten moving cursors at 30 Hz is about
26 kbit/s per receiver.

## 7. Human input through a media session

A viewer that clicks, types or scrolls through a media stream is a person,
not an agent. Exactly one cursor shows for it: the viewer's own presence
cursor, drawn by each client. Neither presence nor the guest desktop adds an
agent cursor for that input.

- **Attribution.** A client that has joined presence sends its participant
  id as `OpenMediaRequest.presence_participant_id`. The media session's input
  is then attributed to that participant's principal (id, display name and
  kind) for input leases and pointer ownership (§3). A caller whose principal
  the server authenticated (relay-asserted, viewer ticket) may only name a
  participant with its own principal id. Without a participant id the
  session keeps the connection's principal. Media input counts as a human's
  unless that principal is an agent.
- **No agent cursor.** Where cua-driver delivers a human's media input
  through its tools (Hyprland, where it has no stateful input session), the
  calls run in a human-origin driver session (the private `_input_origin`
  argument, which public callers cannot send). cua-driver draws no agent
  cursor overlay for such a session on any platform, and cua-spacesd does not
  publish the session's cursor moves as an agent participant. The X11 and
  macOS input sessions inject without the overlay or the cursor hook at all.
  Windows has no media input yet (`OpenMedia` sessions there are view-only).
- **Agents are unchanged.** MCP, SDK and ComputerService agents keep their
  in-guest overlay and their agent participant.
- **Names.** The apps join as the signed-in account's name, else its email's
  local part, else its username, else the computer account's full or short
  name (`cua-spaces-app-core` `presence`). A presence name is never empty,
  an agent's, or "You".
