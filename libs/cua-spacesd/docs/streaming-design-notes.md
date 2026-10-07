# Streaming design notes

How the cua-spacesd media plane (rcdp wire v2, `libs/cua/proto/MEDIA.md`)
handles capture, encoding, fan-out and recovery. These notes summarise the
clean-room research reports in our own words, and point at the code that
implements each decision.

## Capture

- **Damage-driven.** On Linux X11 a stream grabs a frame only after XDamage
  reports a change, or when a keyframe is requested, and never faster than
  the frame-rate cap. A static desktop therefore sends almost nothing (the
  conformance test budget is 50 KB/s). See `cua-spacesd-desktop/src/linux_stream.rs`.
- **XShm reads** into a mapped shared segment. The fallback is `GetImage`.
- **Per-window capture without occlusion.** A window target is redirected
  with Composite, and its own pixmap is read. A freshly named pixmap is
  repainted (an Expose for the whole window) before the first grab.
- **Cursor out of band.** Frames never contain the pointer. Cursors travel
  over `PresenceService`, coalesced to 20 Hz per participant.

## Encoding

- The session owns a codec-neutral `FrameEncoder` seam. Its default factory
  is `cua-media-codec`, which picks the fastest encoder that works (NVENC,
  VA-API, QSV, AMF, VideoToolbox, Media Foundation, or OpenH264) and falls
  back at runtime when one fails.
- Output uses constrained baseline with no B-frames. SPS/PPS go on every IDR,
  and IDRs are sent only on demand (attach, recovery, resize). Bitrate and
  frame rate change live. A size change or a backend fallback starts a new
  codec epoch that begins with an IDR.

## Fan-out and flow control (per viewer)

- **One encode, many sockets.** Each attached socket has its own bounded
  queue: about two seconds of stream at the current bitrate, and at least
  4 MiB.
  - A keyframe replaces everything queued before it.
  - On overflow, or when the oldest frame is more than a second old, the
    viewer drops its queue and waits for the next keyframe.
  - Keyframe requests are limited to one per second per session, so one slow
    viewer cannot flood the others with IDRs.
- **Ack window.** Clients that send `frame_ack` may leave about two seconds of
  frames unacknowledged, at their measured decode rate. The window shrinks as
  the round-trip time grows.
  - When the window is exceeded, the viewer skips to a keyframe.
  - A client that stops acking for 4 s is marked stalled and gets one
    keyframe probe every 2 s.
- **Keyframe on attach.** A short cached GOP is replayed. If the GOP is too
  long, a fresh IDR is forced. Either way, the first packet a socket receives
  is a keyframe.
- **Send priority.** Control messages go first, then audio (at most 200 ms
  queued per track, dropping the oldest and flagging the discontinuity),
  then video.

## Rate control (per session)

Every 500 ms the session looks at the worst viewer's signals:

- queue delay;
- ack round trip above its recent minimum;
- dropped frames.

It reacts as follows:

- Congestion must last three windows before a cut.
- A normal cut lowers bitrate by 20%. Frame rate is cut only once bitrate is
  at its floor, because a bitrate-targeted encoder does not send fewer bytes
  at a lower frame rate.
- One second or more of excess delay halves both bitrate and frame rate
  immediately.
- Recovery is slow and additive. Frame rate is restored first, then bitrate,
  and only while the screen is actually changing.

See `cua-spacesd-session/src/media/rate.rs`.

## Input

- **Explicit delivery.** Background input goes to one window with XSendEvent,
  through cua-driver `platform-linux`. It never moves the pointer or changes
  focus. Foreground input activates the window and uses XTest. `AUTO` uses
  background whenever it can address a window.
- **Per-window input leases.** Two principals can drive two different
  windows at once. A window held by another principal refuses input until
  the lease has been idle for 5 s. Handing a lease over releases any held
  keys and buttons.
- **Interactive input sequencing.** The first batch on a socket sets the
  base. After that, a gap is answered with `input_sequence_gap` and the
  expected sequence.

## Audio

- Audio uses the same socket and the same microsecond media clock as video
  (`cua_media_codec::media_clock_us`).
- Capture and Opus come from `cua-media-codec`. A frame chunker keeps packet
  timestamps contiguous.
- DTX silence is not sent: a gap in `pts_us` with no gap in the sequence
  means silence.
- The container A/V test measures about +10 to +25 ms of skew, within the
  ±40 ms budget. PulseAudio in a container adds unreported buffering between
  a player and the monitor, so the sync test uses a scheduled source to
  isolate the driver's clock plumbing.

## Known gaps

- Direct QUIC media (UDP 3212, `--quic-port`, 0 disables) sends video as
  RVD2 datagrams and audio as RAU2 datagrams; video is dropped rather than
  queued when the datagram buffer could no longer take audio, and the client
  recovers with `request_keyframe`. There is no FEC.
- Content-adaptive encoding (still images for text, video sub-regions,
  scroll detection) and FEC are not implemented. On WebSocket over TCP there
  is no packet loss, only stalls, which the queues above handle.

## Benchmark methodology

The media plane is measured end to end by `libs/cua/bench/streaming`
(README there; nightly in `.github/workflows/bench-streaming.yml`). The
design follows the research reports' main lesson: make frames
self-describing so the oracle is in the pixels, not in logs.

- **Self-describing fixture.** A small GTK app (`fixtures/benchfix.py`,
  uploaded through the driver's process API, so the image is unchanged)
  draws a 48-cell black/white strip holding the guest's wall-clock
  milliseconds with sync patterns and an XOR checksum, and a photon square
  that toggles on every click. It also serves its clock over TCP. The
  client estimates the guest/host clock offset NTP-style (min-RTT sample,
  error ≤ RTT/2), then reads the strip from every decoded frame: that gives
  glass-to-glass latency with no clock in the data path.
- **Input to photon.** Clicks alternate between `interactive_input` and a
  media-plane `action`. The time until a decoded frame shows the toggled
  square is input-to-photon. Action results and `ActionFrameCorrelation`
  are timed as well, and the correlated sequence is compared with the
  photon frame.
- **Scenarios.** Scenarios cover content classes: static window and desktop
  (bytes/s floor), small damage (timecode), text scroll, video-like motion
  (moving blocks over a gradient) and a window moving across the desktop.
  Impairments are injected at the client: seeded QUIC video-datagram loss
  before reassembly, and WebSocket reader stalls. Recovery is timed from
  reference loss (or stall end) to the next usable frame. The client
  re-requests a keyframe at most once per second while it waits.
- **Audio.** The image's A/V fixture flashes and beeps on each wall-clock
  second. Beep onsets are found in decoded PCM with a Goertzel detector.
  A/V skew is flash `capture_timestamp_us` minus onset `pts_us` on the one
  media clock. Audio latency is onset arrival against the second boundary,
  corrected for the clock offset.
- **CPU.** Client CPU comes from `getrusage`. Server CPU is the driver's
  `/proc` times, read through its own process API. Container CPU comes from
  the docker cgroup, which under runsc includes the gVisor sandbox.
- **Matrix.** The matrix crosses runc and runsc (one 4 GiB container at a
  time), WebSocket and QUIC, and VideoToolbox or OpenH264 decode. Clients
  are the native Rust client, a Linux sidecar sharing the Space's network
  namespace (QUIC from macOS, where Docker does not forward UDP), and the
  language examples in bench mode. Hardware encoders are reported as
  deferred (§8.6 of the plan).
- **Gate.** `budgets.json` holds loose thresholds for a hosted runner, and
  `check` fails CI on a regression.
