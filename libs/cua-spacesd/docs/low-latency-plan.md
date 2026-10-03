# Low-latency plan

> Design history imported from rcdp. Current behavior of the `cua.env.v1` server is in the [README](../README.md); media wire v2 is in [`MEDIA.md`](../../cua/proto/MEDIA.md).

Interactive streaming latency is the sum of capture scheduling, encode,
network transit, decode, presentation, input transit, host dispatch, and the
next changed frame. Network RTT is only one term, so RCDP measures and improves
the stages independently.

## Current latency shape

- The native client can expose a one-second title-bar HUD with received FPS
  and bitrate, control RTT, host encode time, decode time,
  receive-to-present time, input acknowledgement time, and client/server frame
  replacement. Enable it with `cua-viewer --show-stats`.
- Native sessions default to 60 FPS and retain one latest frame at every
  replaceable queue boundary.
- Interactive input uses monotonic, micro-batched event sequences with up to
  32 batches in flight. The host acknowledges the highest native-dispatched
  sequence, so typing, key repeats, live drags, and trackpad momentum never
  wait one network RTT per sample. Legacy action requests remain available for
  semantic automation and peers that do not advertise `input.interactive.v1`.
- Embedded input skips CUA's agent-oriented one-second post-action
  window-change polling because RCDP already observes the captured target
  continuously. Regular CUA calls retain that observer. Foreground typing also
  preserves existing unreadable text and uses the short already-frontmost
  settle path, preventing per-character overwrite and acknowledgement buildup.
- On macOS, an interactive CUA session owns one persistent event worker and
  `CGEventSource`, caches target geometry briefly, and keeps the selected app
  frontmost for the lifetime of an activation-allowed session. It accepts true
  key down/up/repeat, Unicode commits, pointer down/move/drag/up, and continuous
  pixel scroll phases without tool-level sleeps or per-event focus checks.
- Frame and action publication wake transports immediately. A timer remains
  only for the 250 ms action/frame-correlation deadline.
- H.264 uses hardware VideoToolbox encode/decode on macOS, disables frame
  reordering, and bounds every mailbox.
- WebSocket/TCP remains ordered and reliable. Packet loss can therefore delay
  newer video behind retransmission of an obsolete frame.
- The direct QUIC binding now carries control, input, app icons, and lifecycle
  on one ordered bidirectional stream while carrying video in authenticated
  datagrams. A 24-byte fragment header identifies the packet, fragment range,
  total length, and keyframe status. The receiver retains only the newest
  incomplete frame, expires it after 150 ms, and asks for a new keyframe after
  dependency loss instead of retransmitting late video.
- macOS H.264 capture now retains ScreenCaptureKit's native `CVPixelBuffer` in
  the one-frame mailbox and submits that exact surface to VideoToolbox. The
  owned CPU BGRA capture/encode path remains only as fallback.
- The macOS client asks VideoToolbox for Metal-compatible IOSurface-backed
  output, retains the decoded `CVPixelBuffer`, maps it through
  `CVMetalTextureCache`, and samples that exact texture in the wgpu surface.
  Packed BGRA remains a compatibility fallback, but H.264 presentation no
  longer copies decoded pixels through CPU memory.

## Transport and pipeline principles

Prioritize latency, then frame rate, then quality. Keep raw frames in GPU
memory from capture through hardware encode and from hardware decode through
shader-based presentation. Drop accumulated frames rather than buffering them.
Expose capture, encode, network, decode, frame-time, bitrate, and loss metrics
in the client overlay.

On the wire, the minimum packet contract is sequence number, type, and
payload; the receiver reports its read sequence so the sender can calculate
RTT; out-of-order ACKs trigger fast retransmit while a multiple of RTT is only
a failsafe. Drop-versus-retransmit decisions must be fine-grained, because
some real-time data is already obsolete by the time reliability recovers it.

RCDP applies those semantics on QUIC. Reliable QUIC streams
fit control and input; authenticated QUIC datagrams fit deadline-bound video.
Video fragments carry session, frame/codec epoch, sequence, fragment indexes,
and keyframe status. Incomplete obsolete frames expire rather than blocking a
newer frame. Keyframe loss requests a fresh keyframe over the reliable control
stream instead of retransmitting an already late delta frame.

That split is now implemented for the native macOS and Windows clients. The
certificate is persistent and explicitly SHA-256 pinned; the existing protocol
token remains the application authentication layer. WebSocket remains the
compatibility and browser binding.

The native controller evaluates 500 ms windows. It uses incomplete datagram
frames, QUIC RTT growth above a path baseline, decoder-budget pressure, and
one-frame presentation replacement. Congestion reduces explicit H.264 bitrate
first; sustained or local pressure then reduces resolution and FPS. Recovery
requires six stable windows and increases quality additively.

## Ordered work

1. Extend the implemented client HUD with media delivery/loss telemetry. Done:
   QUIC reports incomplete network frames separately from local/server queue
   replacement.
2. Render VideoToolbox output without a packed-BGRA CPU round trip. Done on
   macOS; keep the fallback for raw-video peers and pursue the corresponding
   native-surface path on Windows separately.
3. Separate reliable control from replaceable video. Done for native clients
   with QUIC streams plus datagrams; WebSocket remains available.
4. Feed RTT, loss, delivery rate, and decoder pressure into bitrate,
   resolution, and frame-rate control on a sub-second cadence. Done for QUIC
   RTT/loss and client decode/presentation pressure; delivered bitrate remains
   visible in the HUD.
5. Add H.265 where both peers advertise low-latency hardware support, while
   retaining H.264 as the broad compatibility path.

Direct `ws://` mode is useful on a trusted LAN and for controlled A/B tests,
but it only removes proxy/tunnel overhead. It cannot remove geographic RTT or
TCP head-of-line blocking.
