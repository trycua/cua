# RCDP architecture

> Design history imported from rcdp. Current behavior of the `cua.env.v1` server is in the [README](../README.md); media wire v2 is in [`MEDIA.md`](../../cua/proto/MEDIA.md).

RCDP separates target selection, capture, action delivery, session state, and
transport. The daemon can change its CUA adapter or macOS capture backend
without changing a client, while another client can use the wire crate without
linking native code.

## Package direction

```text
cua-media-client -> cua-media-protocol
    |                           ^
    +------ cua-media-transport -----+

cua-viewer --------------> cua-media-protocol
        +----------------------> cua-media-transport

cua-spacesd -> cua-spacesd-session -> cua-spacesd-provider-api -> cua-media-protocol
  |
  +----> cua-spacesd-desktop -> pinned public CUA
```

Teleport is split along the same client/server line:

```text
cua-spacesd -> cua-spacesd-server -> cua-spacesd-teleport (receive: import, Keychain install, relaunch)
                                          |
cua SDK / cua CLI -> cua-teleport (send) -+-> cua-teleport-bundle (libs/cua: format, layouts; no effects)
```

The driver never links `cua-teleport`; only `tests/teleport-e2e` (its own
workspace) links both halves.

`cua-media-client` has one direct dependency: `cua-media-protocol`. It does not link the
daemon, providers, CUA, or an operating-system crate.

`cua-viewer` is another protocol consumer. Its macOS and Windows surfaces
use a real native top-level window and a local renderer, but it does not link
CUA or a capture backend. Interactive input crosses the wire as ordered,
semantic event batches; agent-style tools use ordinary RCDP actions. The host
is the only process that resolves the opaque target and invokes CUA.

The provider API owns native identities. Its `BackendTargetKey` has a redacted
`Debug` implementation, and the adapter never serializes it. Public CUA does
not import RCDP; the private dependency points from RCDP to CUA.

## Target selection and identity

A target provider supports three selection paths:

- `enumerate` returns visible or all targets;
- `pick` asks the operating system or adapter to select one target;
- `restore` resolves a server-issued grant from an earlier selection.

Each result contains an opaque handle and target epoch. Native identifiers can
be recycled, so the epoch changes when a previously absent native target
reappears. A handle with an old epoch fails before capture or action delivery.
The macOS adapter implements enumeration and process-local restoration grants;
its system picker remains pending.

## Provider boundaries

`cua-spacesd-provider-api` defines five independent traits:

- `TargetProvider` selects and resolves opaque targets;
- `CaptureProvider` publishes owned frames and lifecycle events through a
  sink, returning an explicit lease;
- `ActionProvider` advertises per-target action guarantees and performs one
  policy-checked invocation;
- `InteractiveInputProvider` opens one ordered, target-bound input lease and
  synchronously dispatches semantic event batches;
- `AccessibilityProvider` returns state with its own snapshot ID.

Capture callbacks publish owned PNG, packed BGRA, or H.264 Annex B frames.
Native buffer lifetimes do not cross the trait. RCDP v1 uses owned CPU memory;
native zero-copy handles can be added as a later provider variant after
measurement.

## Session policy and action delivery

An open session sets a policy ceiling:

- `view-only` rejects every action;
- `background-only` accepts actions advertised as background-safe;
- `allow-activation` also accepts actions that may activate the target.

The provider reports a guarantee for each action: `background`,
`may-activate`, or `unsupported`. Policy enforcement runs before provider
dispatch. A background-only request for `bring_to_front`, for example, returns
`WouldRequireActivation` and never calls CUA.

The CUA adapter resolves the opaque target and overwrites any native target
facts immediately before invoking the tool. Client arguments containing keys
such as `pid`, `window_id`, `hwnd`, or `portal_token` are rejected.

The interactive input path deliberately bypasses repeated tool invocations.
Each connection owns one bounded ordered lease, requires contiguous sequence
numbers, and returns cumulative native-dispatch acknowledgements. The macOS
adapter maps protocol events into CUA's generic interactive API only after it
has resolved the opaque target. `AllowActivation` uses one persistent
foreground session; `BackgroundOnly` uses target-stamped background events;
`ViewOnly` opens no lease.

Clients address pixel actions in the current encoded frame's dimensions. The
session attaches that frame geometry to the internal action invocation, and
the provider converts once to its captured window's native pixel dimensions.
The macOS adapter also gives its private CUA session uncapped accessibility
screenshots, preventing CUA's screenshot-resize registry from applying a
second scale. The Windows adapter preserves the same boundary by mapping WGC
frame pixels through the target's DPI-aware extended frame bounds. A future
Linux adapter must do likewise for display points or surface-local coordinates.

## Freshness domains

RCDP keeps five clocks separate:

- target epoch detects recycled or restored target identity;
- geometry epoch guards pixel coordinates across resize and scale changes;
- frame sequence orders captured frames;
- accessibility snapshot ID guards semantic element references;
- codec epoch marks decoder configuration changes.

A video frame does not invalidate an accessibility snapshot. A new
accessibility snapshot does not imply new geometry.

The runtime records the internal action sequence before dispatch. The provider
delivery result is returned without waiting for capture. The first captured
frame afterward carries that boundary in a separate
`ActionFrameCorrelation`; a correlation with no frame marks the 250 ms
deadline. This proves ordering, not a visible effect.

## Bounded delivery

Capture producers do not wait for a client socket. The connection mailbox
keeps one latest frame per target and replaces older pending frames. It keeps
at most 256 ordered lifecycle/action events and drains those lossless controls
before replaceable video. An event overflow produces a connection error
instead of growing memory without limit. Stream stats report
emitted frames, replaced frames, keyframe requests, and pending state.
Frame, lifecycle, and provider-completion publication wakes the transport
immediately; a slower timer remains only for action-correlation deadlines.

Each connection owns its stream-state hub. Two clients may capture the same
target with different frame limits without sharing geometry epochs, frame
sequences, replacement state, or action boundaries. A connection permits one
active session per target; closing it prunes that target's stream state before
the target can be reopened.

Runtime preference changes use generation-fenced capture replacement. The new
generation becomes active before it starts, the previous lease remains as a
rollback until startup succeeds, and callbacks from any retired generation are
dropped. H.264 restarts reserve a new wire codec epoch even when the replacement
encoder's provider-local epoch starts again at one.

Preferences include an optional explicit H.264 target bitrate. The native
controller evaluates 500 ms windows and reduces bitrate before degrading FPS
or long-edge resolution when datagram loss, QUIC RTT growth, decoder pressure,
or presentation replacement indicates congestion. Recovery is additive after
six stable windows.

The local transport prefixes each packet with two network-order `u32` lengths,
then sends a JSON header and optional binary payload. It rejects headers above
1 MiB and payloads above 64 MiB before allocation. The daemon creates its Unix
socket with mode `0600` and refuses to replace a regular file or symlink.

The direct QUIC binding retains that framing for one reliable bidirectional
control/input/app-icon stream. Video packets are split into authenticated
datagrams with packet and fragment IDs, total length, and keyframe status. The
client keeps only the newest incomplete packet, expires it after 150 ms, and
requests a new keyframe over the reliable stream when dependency state is
lost. A persistent self-signed server identity is certificate-fingerprint
pinned by the client. WebSocket remains the browser and compatibility binding.

The remote binding serves a browser viewer, share metadata, and RCDP WebSocket
on a loopback-only HTTP listener intended for Tailscale Serve. The upgrade
validates Tailscale's injected user and application-capability headers. A
daemon instance exposes one exact application ID and applies a
`AllowActivation` session-policy ceiling. The native client requests that
policy, while view-only and background-only clients remain valid. Provider
filtering happens before opaque handles enter the catalog, rather than
filtering serialized discovery responses after the fact.

## macOS implementation

The CUA adapter uses public CUA primitives for target enumeration,
accessibility state, and actions. A temporary private ScreenCaptureKit backend
implements the new owned-frame seam until a released CUA revision exposes its
macOS provider.

ScreenCaptureKit filters one window and excludes the cursor. For H.264 it
retains the captured `CVPixelBuffer` through the one-frame mailbox and submits
that exact surface to VideoToolbox; tightly packed BGRA remains the negotiated
CPU fallback. A 200 ms monitor reports title, geometry, inactive/resumed, and
closed events. Resizes update the stream configuration behind a callback fence
and discard queued buffers with stale dimensions.

When a session selects H.264, retained native surfaces enter a one-frame
replaceable mailbox and a dedicated VideoToolbox worker. Encoder pressure
therefore never blocks ScreenCaptureKit. Output uses constrained-baseline Annex
B with no frame reordering; every IDR includes SPS/PPS. A resize, preference
change, or encoder restart advances the codec epoch and forces an IDR. If
either the encoder mailbox or connection mailbox replaces a dependent frame,
it requests a new keyframe and suppresses dependent output until recovery.

The macOS client asks VideoToolbox for Metal-compatible IOSurface-backed
output. It retains each decoded `CVPixelBuffer`, creates a `CVMetalTexture`,
imports the exact `MTLTexture` into wgpu, and samples it directly into the
window surface. Packed BGRA peers retain the CPU upload fallback.

Action providers run on a bounded, ordered per-connection worker. Dispatch no
longer holds the socket loop while CUA completes focus preservation or effect
checks, so lifecycle and latest video continue draining during a slow action.
`ActionResult` arrives from the completion mailbox; first-frame correlation is
recorded independently whether the frame or provider completion arrives first.
For CUA input tools, the provider derives `delivery_mode` from the accepted
session policy and overwrites any client-supplied value. `AllowActivation`
persistently activates the shared app before input and forces foreground
delivery; `BackgroundOnly` forces background delivery without activation.

Native interactive input uses a separate persistent CUA worker rather than
the action/tool path. One `CGEventSource` is reused for the session, target
window bounds are cached for a short interval, and foreground ownership is
rechecked only when it changes. The worker posts exact key transitions,
Unicode commits, live pointer phases, and continuous pixel-scroll phases, then
acknowledges the batch after native posting. This removes per-character focus
settling, click-style drag synthesis, and fixed scroll sleeps from the hot
path.

The daemon needs a stable signed macOS app identity. TCC failures return a
typed consent or capture error during session open.

Opt-in host resizing uses a provider-owned Accessibility `AXSize` write wrapped
in CUA's focus-suppression guard. The daemon grants one geometry controller per
target across connections, rejects stale revisions, and releases ownership on
close, target loss, or socket drop. Capture continues to detect the resulting
size and emits the authoritative geometry epoch; the resize acknowledgement is
not allowed to fabricate capture state.

The RCDP provider also disables its fixed CUA cursor session before any input
action. This is independent of whether a general CuaDriver daemon was launched
with an overlay: RCDP remote-window actions never show that pointer.

## Windows implementation

The Windows provider resolves opaque targets to HWND values internally and
enumerates the Win32/UIA catalog through the pinned CUA platform crate.
Windows.Graphics.Capture owns per-window BGRA capture, rebuilds its frame pool
on content-size changes, and emits minimized, resumed, title, geometry, and
close lifecycle events. Packed BGRA is the compatibility path.

For H.264 sessions, capture publishes into a one-frame mailbox serviced by a
software OpenH264 worker. Replacement, resize, and explicit recovery force an
IDR; every independently decodable IDR carries SPS/PPS. The encoder crops odd
WGC extents to even dimensions and advances its codec epoch when dimensions
change. This keeps capture callbacks non-blocking while avoiding queues of
obsolete screen frames.

The native Windows client reuses the session, input, latest-frame,
letterboxing, adaptation, and resize state machine used on macOS. Its
platform-specific decoder resets OpenH264 at codec epochs, waits for a
keyframe, converts decoded YUV to packed BGRA, and presents through the Windows
`wgpu` backend. Native `winit` events supply pointer, drag, wheel, text, and
shortcut input; the Windows super-key vocabulary maps to `win` rather than
`cmd`.

Bidirectional geometry uses a DPI-aware `SetWindowPos` with no-activation and
no-z-order flags, then reports actual extended-frame dimensions in logical
points. WGC lifecycle remains authoritative and causes the encoder and decoder
to restart at a new keyframe. Foreground input mode may activate the selected
app for compatibility, but the provider's CUA cursor session remains disabled.

## Linux mapping

The protocol does not assume enumerable windows. Linux can map enumeration to
X11 and map `pick` or `restore` to portal capabilities and PipeWire nodes. It
must return the same opaque handle, epoch, lifecycle, owned-frame, geometry,
and action-policy types.

Linux has X11 and Hyprland capture adapters (raw BGRA, XTest input); the Linux H.264 path and native client remain pending. Their platform
integration should live in RCDP or another private package, while reusable
capture and action primitives can remain MIT in CUA.

The client follows the same split. macOS and Windows supply native proxy
surfaces; Linux can later use a Wayland/X11 top-level window. No client needs
native target identifiers: discovery, lifecycle, frame freshness, geometry,
and actions remain protocol messages. See
[Native app client](native-app-client.md).
