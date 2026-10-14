# Media plane: rcdp wire v2

Video does not travel over gRPC. `cua.env.v1.StreamService` discovers
targets, negotiates the codec and limits, and mints a media ticket. The frames
then flow over the **rcdp wire, version 2**, on either:

- the `/media` WebSocket on the spacesd port (3211). This works through the
  Fleet gateway and the relay.
- direct QUIC on UDP 3212, when the client can reach the guest directly.

This document lists only what v2 changes relative to
[`libs/cua-spacesd/docs/protocol-v1.md`](../../cua-spacesd/docs/protocol-v1.md).
Anything not mentioned here is unchanged from v1:

- JSON control messages `{"type": …, "payload": …}`.
- Length-prefixed binary video packets (`header_len: u32`, `payload_len: u32`,
  both big-endian).
- `VideoFrameDescriptor`, H.264 Annex B with SPS/PPS on every keyframe.
- Codec, geometry and target epochs.
- Latest-frame flow control, actions and lifecycle events.

Each change is tagged with the entry in
[`FRICTION-rcdp.md`](../../spaces-app-swift/Sources/CuaSpacesStreaming/FRICTION-rcdp.md)
it fixes, written as (F*n*).

Only cua-spacesd speaks v2. It does not accept v1 connections.

## 1. Session setup moves to gRPC

| v1 on the media socket | v2 |
|---|---|
| `ListWindows`, `GetAppIcon`, `PickWindow`, `RestoreWindow`, `ListApps`, `LaunchApp` | `StreamService.ListTargets` and `WindowsService`. The media socket rejects these with `error{code: unsupported_operation}`. |
| `OpenSession` | `StreamService.OpenMedia`. The session already exists when the socket attaches. |
| `SetStreamPreferences` | Still accepted on the socket. `StreamService.SetPreferences` is equivalent. |
| `RequestKeyframe` | Still accepted on the socket. `StreamService.RequestKeyframe` is equivalent. |
| `CloseSession` | `StreamService.CloseMedia`, or just closing the socket. Closing the socket does **not** close the session: the ticket can reattach until it expires. |
| `Join`, `Cursor`, `Joined`, `Presence`, `RemoteCursor` | `PresenceService` over gRPC. These are removed from the media plane. |
| `GetClipboard*`, `SetClipboard*` | `ComputerService.GetClipboard` and `SetClipboard`. Removed from the media plane. |

What stays on the media socket: video, `Lifecycle`, `InteractiveInput` and
its acknowledgement, `Action`, `ActionResult`, `ActionFrameCorrelation`,
`GetWindowState`, `SetWindowGeometry`, `GetStats`, preferences and keyframe
requests, and `Error`.

## 2. Ticket authentication (F3, F4)

- The root token never goes on the media socket. `OpenMedia` returns:
  - an opaque `ticket` of at least 128 random bits, base64url;
  - a `ws_path` of `/media?ticket=<ticket>`;
  - `ticket_expires_at`. The default TTL is 60 s and the maximum is 10 min.
- A ticket is bound to one media session. That means one target, the
  negotiated codec and limits, the `SessionPolicy`, and the principal that
  opened it. The ticket authorizes nothing else. That is why it is safe in a
  URL, a log line or an `<img>`/`<video>`-adjacent context.
- **WebSocket.** The ticket can be presented in two ways:
  - in the `ticket` query parameter;
  - as the WebSocket subprotocol `cua.ticket.<ticket>` (the form every
    cua-spacesd ticket route accepts), alongside the subprotocol
    `rcdp.v2`. Browsers cannot set headers, so this is the header-free
    alternative. Servers still accept the legacy `rcdp.v2.ticket.<ticket>`;
    clients send `cua.ticket.<ticket>`.

  The server validates the ticket **during the HTTP upgrade**:
  - Missing, unknown or expired tickets get HTTP 401 before any WebSocket
    frame is sent.
  - A ticket for a closed session gets 410.
  - There is no `Authenticate` / `Authenticated` exchange.
- **QUIC.** The ALPN is `rcdp/2`. The client pins the certificate with
  `QuicEndpoint.certificate_sha256`. The first control message on the
  reliable stream must be `{"type":"ticket","payload":{"ticket":"…"}}`. If it
  is anything else, or the ticket is invalid, the server closes the connection
  with application error code `0x401`.
- **Expiry and reuse.**
  - Expiry only limits *new* attaches. An attached socket stays open after
    the ticket expires.
  - A ticket may attach again after a disconnect, for example on a network
    change, as long as it has not expired.
  - Every attach is its own connection-scoped view with its own flow control.
  - After expiry, call `OpenMedia` again.
- Rotating the root token (`SystemService.Init`) revokes every outstanding
  ticket and closes attached media sockets with close code 4401.

## 3. Server-first handshake

Right after the upgrade (or after the QUIC `ticket` message), the server sends
`hello` and then `session_opened`, in that order. The client sends nothing
first.

```json
{"type":"hello","payload":{"protocol":"rcdp","versions":[2],"selected_version":2,
  "capabilities":["input.interactive.v2","desktop.v1","keyframe_on_attach.v1","audio.v1"]}}
{"type":"session_opened","payload":{ … same shape as v1 SessionOpened … }}
```

`session_opened.session_id` equals `OpenMediaResponse.media_session_id`.
When the session has audio, `session_opened` is followed by one
`audio_config` message per track (§12.3). `audio.v1` is advertised only
when the driver supports audio.
v1's `Hello` negotiation is gone. A client that needs a capability that is
not advertised must close the socket.

## 4. Desktop targets (F5)

- A session's target is a window or a whole display, as chosen by
  `MediaTarget` in `OpenMedia`.
- `session_opened.target` gains a `kind` discriminator:
  - `{"kind":"window","handle":"…","epoch":N}`
  - `{"kind":"display","display_id":"…"}`
- For display targets:
  - Frames cover the whole display framebuffer, scaled to at most
    `max_dimension` on the long edge.
  - Pixel-basis actions and interactive input use the display's frame pixel
    space. The server converts to native coordinates once, as in v1.
  - `Lifecycle` events that apply:
    - `geometry_changed`, when the resolution or scale changes;
    - `suspended{capture_failed}` and `resumed`;
    - `closed`, when the display is removed.
  - `title_changed` and `suspended{minimized}` never occur.
  - `SetWindowGeometry` is rejected with `unsupported_operation`.
- Linux provides desktop capture with XShm plus XDamage. A frame is produced
  only when damage occurs, subject to `max_fps`.

## 5. Keyframe on attach (F8)

- The first video packet delivered on every newly attached socket is a
  keyframe of the current `codec_epoch`. On attach the server forces an IDR,
  or resends the most recent keyframe if it is still current.
- The server discards dependent frames queued for that socket before the
  keyframe.
- Clients must not need to send `RequestKeyframe` to start decoding. They may
  still send it after losing packets.

## 6. Sequence numbers start anywhere (F1, F2)

- **Video.** `FrameSequence` counts per target, not per session. The first
  frame a socket receives may carry any value. After that, sequences strictly
  increase, and gaps mean frames were replaced. Clients must not assume a
  start of 0 or 1.
- **Interactive input.**
  - The first `InteractiveInput` batch on a socket may start its sequence
    range at any value, and the server adopts it as the base.
  - After that, batches must be contiguous, as in v1.
  - A gap or a repeat is rejected with the new error code
    `input_sequence_gap`. v1 used `stale_target`, which was misleading.
  - The error payload carries `expected_sequence`, so the client can re-sync
    instead of failing every batch that follows.
- **Acknowledgements.** `InteractiveInputAcknowledgement.error` is required
  whenever `delivered` is false. Clients should model the ack as a result
  type.

## 7. Geometry authority (F7, F13)

- `WindowInfo.bounds` from `WindowsService` and `StreamService.ListTargets`
  is advisory.
- The authoritative frame geometry comes from two places:
  - `OpenMediaResponse.geometry` and `session_opened.geometry`;
  - every later `lifecycle{geometry_changed}` event.
- The SDK publishes this as one observable value, which is what its
  coordinate helpers read.

## 8. Health counters (F9)

`Stats` gains the following fields. Clients should judge liveness from these,
not from fps. A static window legitimately produces about 0 fps.

| Field | Meaning |
|---|---|
| `frames_since_attach` | Frames delivered on this socket. |
| `last_frame_age_ms` | Time since the last frame was captured for this target, whether or not it was delivered. |
| `target_idle` | `true` when capture is healthy but the target has not changed. |

## 9. QUIC datagrams

- The datagram header is unchanged from v1 except for two fields:
  - the magic is `RVD2`;
  - the version byte is `2`.
- Fragment reassembly, the 150 ms incomplete-packet deadline and
  keyframe-loss recovery work as in v1.

## 10. Close codes (WebSocket) and QUIC application errors

| WS close | QUIC error | Meaning |
|---|---|---|
| 1000 | 0x0 | Normal close by the client. |
| 4400 | 0x400 | Protocol violation, such as a malformed control message. |
| 4401 | 0x401 | Ticket invalid, or the root token was rotated. |
| 4404 | 0x404 | The session was closed with `CloseMedia` or it expired. |
| 4408 | 0x408 | Idle timeout. No pong for 60 s: the server pings every 20 s. |
| 4410 | 0x410 | The target is gone (`lifecycle{closed}` is sent first). |
| 4429 | 0x429 | Too many sockets on the session. The limit is 8. |
| 4500 | 0x500 | Internal error. |

## 11. Limits

These are unchanged from v1:

- control headers ≤ 1 MiB;
- payloads ≤ 64 MiB;
- ≤ 256 queued runtime events per socket;
- one latest frame per target per socket.

New limits in v2:

- at most 8 concurrent sockets per media session;
- at most 64 media sessions per driver.

## 12. Audio

Audio is a first-class track next to video. `OpenMedia` negotiates the
tracks through `AudioOptions` and returns them in `NegotiatedAudio`:

- zero or one video track (`disable_video` gives audio-only sessions);
- any number of **downlink** audio tracks (the desktop mix or per-app
  sources);
- optionally one **uplink** track: the client's microphone into a guest
  virtual source.

Every track uses the same socket as the session's video.

### 12.1 Shared media clock: microseconds

Every timestamp on the media plane is in microseconds on one monotonic
clock per driver. This covers video `capture_timestamp_us` and audio
`pts_us`, for all tracks and all sessions. The origin is arbitrary (driver
start), and values never wrap because they are u64.
`cua_proto::MEDIA_CLOCK_HZ = 1_000_000`.

Why microseconds and not RTP-style 90 kHz or 48 kHz:

- **v1 compatibility.** v1 video descriptors already carry
  `capture_timestamp_us`. v2 only tightens its meaning: the origin is now
  driver-wide, not per capture session. No video field changes unit.
- **No privileged rate.** 90 kHz suits video, and 48 kHz suits one audio
  rate. The contract allows 8 kHz to 48 kHz audio and variable-rate video.
- **Exact frame durations.** Every Opus frame size (2.5, 5, 10, 20, 40 and
  60 ms) is a whole number of microseconds, so packet durations never
  accumulate rounding error.
- **No wraparound.** A u64 of microseconds does not wrap for about
  584,000 years. RTP's 32-bit timestamps force every client to unwrap them.
- **Sub-sample precision is not needed.** A/V sync tolerance is
  milliseconds (the budget is ±40 ms). Inside a track, the decoder counts
  exact sample positions with `frame_samples`.

`pts_us` is the capture time of the packet's first sample. It is derived
from the capture device's timestamp mapped onto the media clock. For
contiguous audio, `pts_us` advances by exactly
`frame_samples * 1_000_000 / sample_rate_hz`. It jumps forward only after
a gap: DTX silence, a paused track, or a capture restart.

### 12.2 Audio packet (both directions)

- An audio packet is **one binary WebSocket message**, or one QUIC datagram
  (§12.6).
- It starts with a fixed 24-byte big-endian header, and the payload runs to
  the end of the message.
- It is told apart from v1-style length-prefixed packets by its first byte:
  - A length-prefixed packet starts with a u32 header length of at most
    1 MiB, so its first byte is always `0x00`.
  - An audio packet starts with the magic `RAU2`, so its first byte is
    `0x52`.
- Binary audio skips JSON headers because an Opus frame at 20 ms and
  32 kbit/s is only about 80 bytes. A JSON descriptor would more than
  double the packet.

| Bytes | Field | Meaning |
|---|---|---|
| `0..4` | magic | `RAU2` |
| `4` | version | `2` |
| `5` | flags | bit 0 `discontinuity`: the previous packet on this track was dropped by the sender, or the track restarted. Bit 1 `dtx`: this is a DTX/comfort-noise frame after silence. Bits 2-7 are reserved and must be 0. |
| `6..8` | track_id | u16 track id from `NegotiatedAudio.tracks` or `AudioUplinkGrant.track_id`. Never 0. |
| `8..12` | sequence | u32 per track and direction. Starts at any value, adds 1 per packet sent, and wraps modulo 2³². |
| `12..20` | pts_us | u64 media-clock time of the first sample (§12.1). |
| `20..22` | frame_samples | u16 samples per channel in this packet, for example 960 for 20 ms at 48 kHz. |
| `22` | config_epoch | u8 that must match the latest `audio_config` for this track (§12.3). It wraps. |
| `23` | reserved | 0 |

The payload depends on the codec:

- **Opus:** exactly one Opus packet (RFC 6716), carrying in-band FEC for the
  previous frame when FEC is on.
- **PCM:** `frame_samples * channels * 2` bytes of interleaved s16le.

Receivers drop any packet in these cases:

- the magic, version or reserved bits are wrong;
- the track id is unknown;
- `config_epoch` is stale;
- a PCM payload length does not match.

A dropped packet is counted in stats. It never closes the socket.

### 12.3 Audio control messages

These are JSON control messages in the v1 `{"type": …, "payload": …}` shape.

- **`audio_config` (server → client), one per track.**
  - Sent after `session_opened`, and again whenever the encoding changes (a
    `SetPreferences` frame-size change or an encoder restart).
  - Payload: `{track_id, config_epoch, direction: "down"|"up", codec,
    sample_rate_hz, channels, frame_ms, bitrate_kbps, fec, dtx,
    opus_pre_skip, source: {source_id, kind, desktop_fallback}}`.
  - Packets with the new `config_epoch` follow it. Clients reset the decoder
    for that track when the epoch changes.
  - For the uplink track, `audio_config` tells the client what to send.
- **`audio_track_state` (server → client).**
  - Payload: `{track_id, state, reason}`.
  - `state` is one of `active`, `silent` (DTX or no audio playing),
    `paused` (`SetPreferences.audio_enabled=false`), `suspended` (capture
    lost; `reason` says why), `fallback` (a per-app source became the
    desktop mix), or `ended` (the source app exited).
  - This is the audio counterpart of video `Lifecycle`.
- **`audio_uplink_state` (client → server).**
  - Payload: `{track_id, muted}`.
  - This is a cheap mute that needs no gRPC round trip. It is equivalent to
    `SetPreferences.audio_uplink_muted`.
- **Uplink errors.**
  - Uplink packets for a track that was not granted get
    `error{code: audio_uplink_denied}` once, and are then silently dropped.
  - Granting is decided only at `OpenMedia`. It depends on the
    `audio.uplink` feature, a policy other than `view_only`, and
    `InitRequest.audio_uplink`.
  - Rotating the token or narrowing the allowlist ends active uplinks with
    `audio_track_state{state: ended, reason: "uplink revoked"}`.

### 12.4 Sequence, loss and recovery

- **Detecting loss.** Order and gaps are detected with a serial-number
  comparison (RFC 1982) on `sequence`.
- **Loss versus silence.**
  - With DTX, the sender *does not send* packets during silence. The
    `sequence` does not advance, but `pts_us` jumps forward. That is
    silence, not loss.
  - Loss is a gap in `sequence` only.
- **Recovering a missing packet N.** When packet N+1 has arrived before N's
  playout deadline and FEC is on, the client decodes N+1 with
  `decode_fec = 1` to rebuild N, then decodes N+1 normally. Otherwise the
  client runs Opus packet-loss concealment (decode with no data) for N's
  duration. For PCM, the client inserts silence.
- **Late packets.** A packet that arrives after its playout time is
  dropped. The client never plays it late.
- **Tuning.** The server tunes FEC redundancy from `expected_loss_percent`.
  Clients should report measured loss by calling `SetPreferences` with
  `audio_expected_loss_percent` when it changes by more than 5 points.
- **Send priority.** Audio is never subject to video's latest-frame
  replacement.
  - On every socket, audio packets go out before queued video.
  - The server keeps at most 200 ms of audio queued per track per socket.
  - Past that, it drops the oldest audio and sets `discontinuity` on the
    next packet it sends. The server never drops audio to make room for
    video.

### 12.5 Jitter buffer and A/V sync (client guidance)

**Jitter buffer.** There is one buffer per downlink track.

- The adaptive target is **20-60 ms**. Start at 2 × `frame_ms`, clamped to
  that range.
- Estimate interarrival jitter the RFC 3550 way, on `pts_us` versus arrival
  time. Aim the buffer at about 2× the jitter estimate plus one frame.
- Grow the buffer immediately on underrun. Shrink it slowly, by at most
  about 5 ms per second, and only during `silent`/DTX periods so the shrink
  is inaudible.
- On WebSocket-over-TCP paths (Fleet gateway, relay), head-of-line blocking
  can exceed 60 ms. Clients may grow past 60 ms there, but must report the
  current target in their own stats.
- **Clock drift.** Correct drift between the guest capture clock and the
  local playout clock (it is typically under 0.1%). Use slight
  resampling, up to ±0.5%, or drop or insert samples during silence. Do
  not let the buffer drift.

**A/V sync.** Audio is the master clock whenever a session has both audio
and video.

1. Keep `audio_clock_us`, the `pts_us` of the sample currently leaving the
   speaker. That is the playout position minus the output device latency.
2. Present a video frame when its `capture_timestamp_us <= audio_clock_us`.
   Keep only the newest frame that is due. Frames that are more than 40 ms
   behind `audio_clock_us` are not waited for: show the newest due frame,
   which fits latest-frame semantics.
3. Hold a video frame that is ahead of `audio_clock_us` by more than one
   video frame interval until it is due. Never delay video by more than
   the audio jitter-buffer target plus 40 ms. Past that, stop slaving video
   and render on arrival until audio catches up (for example after an
   audio stall).
4. With no audio, or when audio is `paused`/`suspended`, render video on
   arrival as in v1.
5. The conformance budget for skew is **±40 ms**. The test uses a fixture
   that flashes and beeps at the same media-clock instant.

**Uplink.** The client stamps `pts_us` from its *own* monotonic clock, in
microseconds. The server uses the uplink timestamps only for ordering,
jitter estimation and gap detection. It runs its own 20-60 ms jitter
buffer before it writes to the virtual source. Uplink audio is not synced
to guest video.

### 12.6 QUIC mapping

- Each audio packet travels as **one QUIC datagram** with the §12.2 header
  unchanged. Audio is never fragmented. The receiver tells it apart from
  video fragments by the magic: `RAU2` for audio, `RVD2` for video.
- **Datagram size.** An Opus packet plus the 24-byte header fits the
  datagram MTU (at least 1200 bytes) when `bitrate_kbps × frame_ms ≤
  about 9,000`. For example, 510 kbit/s at 10 ms fits, and 128 kbit/s at
  60 ms fits.
  - The server caps the negotiated Opus bitrate for QUIC sessions so every
    packet fits.
  - PCM packets and any oversize packet go on the **reliable stream**
    instead, as a binary message with the same header.
- **Priority.** QUIC's non-waiting datagram queue may evict *video*
  datagrams to make room for audio, never the reverse.
- **No retransmission.** Lost audio datagrams are not retransmitted;
  recovery is §12.4. Audio loss never triggers `RequestKeyframe`.
- **Uplink** uses datagrams the same way, from client to server.

### 12.7 Stats

`Stats` gains `audio_tracks: [{track_id, direction, packets, bytes,
packets_lost, packets_recovered_fec, packets_concealed, packets_late,
discontinuities, jitter_ms, buffer_target_ms}]`.

- The server fills the fields it can observe. For downlink tracks that is
  send counts and drops from the queue. For uplink tracks it is receive
  counts, loss and jitter.
- Clients mirror the same shape locally for downlink tracks.
- Benchmarks record audio latency (capture to playout), bitrate and CPU
  from these fields.

### 12.8 Capture sources

| Platform | Desktop mix (`audio.desktop`) | Per-app (`audio.per_app`) | Uplink virtual source (`audio.uplink`) |
|---|---|---|---|
| Linux (Spaces image) | PipeWire null-sink monitor | Per-node capture of the app's stream nodes | PipeWire virtual source |
| macOS | ScreenCaptureKit audio | ScreenCaptureKit app-filtered audio | Virtual audio device (when installed) |
| Windows | WASAPI loopback | Process loopback (Windows 10 2004+) | Virtual audio device (when installed) |

When per-app capture is unsupported:

- the source is still listed with `desktop_fallback: true` and a
  `limitation`;
- the track's `audio_config.source.desktop_fallback` is `true`.

A client can therefore always tell the user that it is hearing everything,
not just the window.
