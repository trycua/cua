# RCDP protocol v1 reference

> v1 is the legacy window-streaming wire, served only by `cua-spacesd legacy`. The `/media` socket speaks v2; see [`MEDIA.md`](../../cua/proto/MEDIA.md) for what changed.

`cua-media-protocol` defines the transport-neutral contract for one-window video,
accessibility state, and adapter-provided actions.

## Version and framing

| Field | Value |
| --- | --- |
| Protocol name | `cua-media` (was `rcdp` before the cua-spacesd import) |
| Protocol version | `1` |
| Implemented bindings | Owner-only Unix socket; token-capable direct WebSocket; certificate-pinned direct QUIC; Tailscale-authenticated WebSocket |
| Header encoding | JSON |
| Payload encoding | Binary |

The local binding sends `header_len: u32`, `payload_len: u32`, the serialized
`WireHeader`, and the payload. Lengths use network byte order. Control packets
have an empty payload. Video bytes stay outside JSON.

The transport rejects headers above 1 MiB and payloads above 64 MiB before
allocation. EOF closes every connection-scoped session and releases its
capture lease.

The WebSocket binding sends control headers as JSON text messages and video as
one complete length-prefixed binary packet per WebSocket message. Tailscale
Serve authenticates the HTTP upgrade out of band; after validating its injected
identity and app-capability headers, the binding sends `Authenticated` and the
client starts protocol negotiation with `Hello`. The backend listener is
required to bind loopback.

The direct WebSocket binding uses the same framing and optional protocol token.
It may apply the same provider-side application selector. A non-loopback direct
listener requires a token, but `ws://` is still plaintext and is intended only
for trusted LANs or an external authenticated tunnel.

The direct QUIC binding uses TLS 1.3 with a persistent self-signed identity
whose SHA-256 certificate fingerprint is pinned by the native client. One
bidirectional stream carries the same length-prefixed control and app-icon
packets as the local binding. Video packets are fragmented across QUIC
datagrams with this 24-byte network-order header:

| Bytes | Meaning |
| --- | --- |
| `0..4` | `RVD1` magic |
| `4` | datagram version (`1`) |
| `5` | flags (`bit 0 = keyframe`) |
| `6..8` | fragment index |
| `8..10` | fragment count |
| `10..12` | reserved |
| `12..20` | monotonically increasing video packet ID |
| `20..24` | complete length-prefixed packet size |

The receiver accepts fragments out of order but allocates only one newest
incomplete packet. A newer packet or the 150 ms deadline discards an incomplete
older packet. Losing keyframe fragments triggers `RequestKeyframe` on the
reliable stream. QUIC's non-waiting datagram queue may evict older media so
input and current video never wait behind obsolete frames. The same protocol
token is still required for a non-loopback listener and is encrypted by QUIC.

## Identities and epochs

| Type | Purpose |
| --- | --- |
| `TargetHandle` | Server-issued opaque target capability |
| `TargetEpoch` | Target identity generation |
| `TargetGrant` | Server-issued restoration capability |
| `WindowSessionId` | Connection-scoped stream and action session |
| `GeometryEpoch` | Surface size and scale generation |
| `FrameSequence` | Per-target captured-frame order |
| `AccessibilitySnapshotId` | Accessibility state generation |
| `CodecEpoch` | Decoder configuration generation |

Serialized messages never contain native PIDs, CGWindowIDs, HWND values,
portal tokens, or native frame handles. A target handle is valid only with its
current target epoch.

## Negotiation and discovery

The first local message is `Hello`. An authenticated WebSocket receives
`Authenticated` first and then sends `Hello`. It contains the protocol name, supported
versions, and extensible capability strings. The daemon selects version 1 and
advertises target enumeration, picker, restoration, binary app-icon assets,
action-policy support, and platform-specific clipboard capabilities.
Unknown future messages deserialize to `Unsupported`.

Target messages are:

- `ListWindows { on_screen_only }` returns `Windows`;
- `GetAppIcon { window, target_epoch }` returns one binary `app_icon` packet;
- `PickWindow { prompt }` returns `WindowSelected` or a typed error;
- `RestoreWindow { grant }` returns `WindowSelected` or a typed error.

`WindowDescriptor` includes the opaque handle, target epoch, application name,
title, public geometry, and visibility. The current macOS adapter supports
enumeration and process-local grants. Its picker returns `Unsupported`.

App icons are requested explicitly rather than embedded in every window list.
`AppIconDescriptor` carries only the opaque target, epoch, media type, and byte
length; the icon bytes use the packet payload. Native bundle paths and process
identifiers remain provider-owned. Servers reject empty icons and icons larger
than 8 MiB.

## Clipboard

macOS peers advertise `clipboard.text.v1` and `clipboard.files.v1`. Text uses
generation-based `GetClipboard` / `SetClipboard` messages and is limited to 1
MiB of UTF-8. Regular files use the equivalent `GetClipboardFiles` /
`SetClipboardFiles` messages. A `ClipboardFile` contains a leaf filename,
offset, byte length, and SHA-256 checksum; concatenated contents use the
packet's bounded binary payload. Native source paths never cross the boundary.

A file clipboard contains at most 16 files, 16 MiB per file, and 32 MiB total.
Receivers reject path separators, traversal names, duplicate names, invalid
ranges, length mismatches, and checksum mismatches before publishing files to
the native pasteboard. RCDP retains only the three newest materialized
transfers. Directories, aliases, and promised files are outside the v1
file-clipboard contract.

## Sessions and video

`OpenSession` contains a target handle and epoch, ordered accepted codecs,
advisory frame-rate and long-edge limits, an optional H.264 target bitrate in
kbps, a `SessionPolicy`, and an optional `geometry_control`. Its compatibility
defaults omit bitrate and use `observe_only`.

`SessionOpened` returns the selected codec, effective limits, current geometry
and epochs, generic session capabilities, per-action guarantees, and the
effective policy and geometry-control mode. RCDP v1 selects H.264, packed BGRA,
or PNG when its capture provider supplies that format. The macOS CUA adapter
prefers H.264 and retains BGRA as a fallback.

`bidirectional` geometry is explicit opt-in and requires a controllable session
policy plus a background-safe provider. Only one session may own a target's
geometry lease. `SetWindowGeometry` carries a strictly increasing revision and
host-logical point dimensions; `WindowGeometryResult` acknowledges the actual
provider result without making it the capture geometry authority. Stale,
duplicate, invalid, or ungranted requests return `applied: false`. The next
`geometry_changed` lifecycle event remains authoritative for video dimensions
and coordinate epochs.

`VideoFrameDescriptor` contains:

| Field | Meaning |
| --- | --- |
| `session_id` | Owning session |
| `sequence` | Captured-frame order |
| `geometry_epoch` | Coordinate generation |
| `codec_epoch` | Decoder configuration generation |
| `width_px`, `height_px` | Payload dimensions |
| `capture_timestamp_us` | Source-monotonic timestamp |
| `encode_duration_us` | Optional host encoder submission-to-output time |
| `codec` | `h264`, `bgra`, or `png` |
| `keyframe` | Random-access marker |

Wire BGRA is top-down, tightly packed, and exactly
`width_px * height_px * 4` bytes. Its implicit stride is `width_px * 4`.
Provider frames with padding, bad dimensions, or the wrong length are rejected.

Wire H.264 is one Annex B access unit per video packet. Every keyframe contains
SPS/PPS before its IDR. `codec_epoch` starts at one and changes when encoder
configuration is replaced, including resize or encoder recovery; the first
frame of a new epoch is a keyframe. A client that loses dependency state sends
`RequestKeyframe`, receives `KeyframeRequested`, discards dependent frames, and
resumes from the next keyframe.

Sessions end on `CloseSession`, socket EOF, or target close. Reconnect requires
new negotiation, discovery, and session open. Resume across connections is not
part of v1.

## Lifecycle

`Lifecycle` reports:

- `geometry_changed` with a new geometry epoch;
- `title_changed`;
- `suspended` with `minimized`, `occluded_no_frames`, `consent_required`,
  `consent_revoked`, `capture_failed`, or `window_unavailable`;
- `resumed`;
- `closed`.

## Accessibility state

`GetWindowState` returns `WindowState` with a session ID, snapshot ID, and
adapter-defined JSON state. Accessibility freshness is independent of video
and geometry freshness.

## Actions and policy

Native interactive clients prefer the negotiated `input.interactive.v1`
capability. `InteractiveInput` contains a bounded ordered batch of semantic
events and a strictly contiguous sequence range. Events cover Unicode text
commits, physical key down/up/repeat with modifiers, pointer
down/move/drag/up/cancel in normalized window coordinates, and pixel scroll
samples with gesture and momentum phases. A batch contains at most 256 events
and 64 KiB of committed UTF-8 text.

`InteractiveInputAcknowledgement` reports the highest sequence accepted by the
host input worker, cumulative delivered event count, native dispatch time, and
an optional error. A valid sequence is consumed even when native delivery
fails, preventing retry ambiguity. Clients may pipeline batches while retaining
strict send order; the native client currently permits 32 in flight and
coalesces adjacent text commits without crossing other input events.

Interactive messages never expose or accept native process, window, keyboard,
or surface identifiers. The server opens the provider lease only after target
resolution and advertises the capability only when the selected platform and
session policy can provide it. `view_only` sessions never advertise it.

The following action contract remains the compatibility and
semantic-automation path.

`ActionRequest` contains an action ID, session ID, adapter-defined tool name,
JSON arguments, and one `ActionBasis`:

| Basis | Validation |
| --- | --- |
| `pixel` | Geometry epoch must match; referenced frame cannot be in the future |
| `accessibility` | Snapshot ID must match the latest state read for the session |
| `none` | No visual or accessibility precondition |

Pixel coordinates are expressed in the current video frame's pixel space:
`(0, 0)` is its top-left pixel and valid coordinates are strictly less than
`width_px` and `height_px`. CSS layout pixels, display points, backing pixels,
and native window coordinates are not part of the wire contract. After
freshness validation, the session passes the frame geometry to the trusted
provider. The provider performs exactly one conversion from video pixels to
its native action space before invoking the operating-system action primitive.

Session policies are `view_only`, `background_only`, and `allow_activation`.
Each advertised action has a `background`, `may_activate`, or `unsupported`
guarantee. Policy validation runs before provider dispatch.

A transport may impose a stricter policy ceiling. The current remote app-share
binding caps connections at `allow_activation`, permitting all three policies.
The native client selects `allow_activation`; the trusted CUA provider derives
foreground or background delivery from the accepted policy rather than from
opaque client action arguments. On macOS, activation-allowed input first
persistently activates the selected app so a focus-proxy channel remains armed
across the interaction; background-only sessions never take that path.

Clients can send `SetStreamPreferences` with a new non-zero frame rate,
maximum dimension, and optional 250..100000 kbps target bitrate. A successful
`StreamPreferencesApplied` means the
server has replaced that session's capture. Late callbacks from the retired
generation are ignored, and an H.264 restart advances the wire codec epoch so
the decoder waits for new configuration and a keyframe.

`ActionResult` reports provider delivery immediately, with an optional typed
`ActionError`. It never waits for capture. When the first subsequent frame is
observed, the server sends `ActionFrameCorrelation`; a `null` frame sequence
means the bounded correlation deadline elapsed. The legacy correlation field
on `ActionResult` remains readable for v1 peers but new servers normally leave
it empty. Action errors include stale target, stale geometry, stale
accessibility snapshot, view-only, would-require-activation, native-target
rejected, unsupported, permission denied, target unavailable, and delivery
failure.

The daemon rejects client argument keys that attempt to supply native target
identity. The trusted adapter injects those values after target resolution.

## Flow control and stats

Each connection stores one latest frame per target and at most 256 ordered
runtime events. Lossless control events and action-frame correlations are
drained before replaceable video. Frame replacement increments the matching session's
`frames_replaced` counter. Event overflow produces an internal connection
error. Replacing H.264 output also requests encoder recovery and suppresses
dependent frames until an IDR arrives. `GetStats` returns emitted frames,
payload bytes, replacements, keyframe requests, pending frames, accepted
action dispatches, native-dispatched interactive input events, post-action
frame-correlation timeouts, and runtime
preference updates for the session.
The performance counters default to zero when reading a response from an
older v1 peer.

## Error handling

Non-action errors use `ServerErrorCode`, including hello required, protocol
mismatch, discovery failure, unavailable codec, invalid open, unknown target,
capture failure, unknown session, unsupported operation, invalid frame, rate
limit, and internal failure. Unknown future codes deserialize as `Unknown`.

## Conformance evidence

Tests pin golden v1 JSON, unknown-message behavior, native-ID exclusion,
target-epoch recycling, separate freshness domains, policy rejection before
provider invocation, action-to-frame correlation, bounded frame replacement,
bounded event overflow, QUIC fragment reassembly and obsolete-frame expiry,
framing size limits, safe Unix socket creation, EOF
teardown, a daemon-to-client binary-frame exchange, Tailscale header checks,
and an authenticated WebSocket H.264 discovery/open/keyframe exchange with
remote policy-ceiling rejection. A native VideoToolbox test proves Annex B
conversion, IDR recovery, and codec-epoch advancement after resize.

Signed macOS device evidence covers target enumeration, continuous BGRA,
accessibility state, a background-only action with unchanged foreground
ownership, action-to-frame correlation, and a 450-frame soak.
Current-code live H.264 capture remains unproven until the separately signed
`Cua Spacesd` app identity receives its Screen Recording grant.

## Provenance

The first protocol and session packages were extracted from TryCua's own CUA
window-streaming prototype. They were not written as a clean-room
implementation. The extracted files retain CUA's MIT notice; original RCDP
work uses FSL-1.1-MIT (Apache-2.0 before that). See [Third-party notices](../THIRD_PARTY_NOTICES.md).
