# Native app client

> `cua-viewer` (libs/cua/crates/cua-viewer) connects to the `cua-spacesd legacy` WebSocket and QUIC listeners described in [Remote app shares](remote-app-shares.md).

`cua-viewer` presents one remote application window as one local native window.
It is the product client for app-like remote use; the HTML client remains a
useful protocol inspector and multi-window development harness.

## User contract

The client connects through WebSocket compatibility mode or the low-latency
QUIC binding, negotiates the protocol,
discovers its authorized targets, and opens either `--target OPAQUE_HANDLE` or
the first visible target. The resulting macOS or Windows window:

- uses the remote application and window title, suffixed with `Remote`;
- preserves the source aspect ratio with letterboxing;
- translates local physical pointer coordinates into the encoded frame;
- sends Unicode commits, exact key down/up/repeat, pointer phases, live drag
  samples, and pixel-scroll gesture/momentum phases in ordered micro-batches;
- keeps up to 32 acknowledged input batches in flight, so WAN latency never
  becomes a per-character or per-scroll-sample stop-and-wait gate;
- requests `allow_activation`, so the provider opens one persistent CuaDriver
  foreground input session. It activates the target at session open and only
  reasserts foreground ownership if another app takes it; and
- does not join desktop presence and explicitly disables its CUA cursor
  session, so neither a CuaDriver pointer nor RCDP input automation becomes
  part of the host desktop or streamed application image.

RCDP can also install a generated per-app macOS bundle. The bundle carries the
icon fetched from the host through the binary app-icon protocol, a stable local
bundle identifier, and a small connection configuration. Opening it rediscovers
the current target and chooses the largest visible application window, so host
restarts do not bake an ephemeral target handle into the Dock shortcut.

## Build and connect on macOS

Build a signed app bundle (the viewer lives in the `libs/cua` workspace;
run this from `libs/cua-spacesd`):

```sh
CUA_ENV_CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
  ../cua/crates/cua-viewer/scripts/build-macos-app.sh
```

Copy `../cua/target/macos/Cua Viewer.app` to the client Mac. The client itself does not need
Screen Recording or Accessibility permission; those permissions belong to the
signed `Cua Spacesd` app on the server Mac.

Discover the target and launch the native client:

```sh
/Applications/Cua Viewer.app/Contents/MacOS/cua-viewer \
  --url wss://HOST.TAILNET.ts.net/v1/connect \
  --target OPAQUE_HANDLE \
  --sync-window-size
```

The one-click Dock-tile installer (`cua-env-cli app install-macos`) was removed
with `cua-env-cli` (cua-spacesd is server-only). It is not in the `cua` CLI
yet: it needs a `cua.env.v1` app-icon RPC in place of the legacy WebSocket
exchange.

The discovery URL and installed streaming URL are deliberately separate: the
CLI's bounded icon/discovery transaction can use the established WebSocket
binding, while every app launch uses the QUIC media path. The certificate pin
is stored beside the connection configuration.

Generated bundles are ad-hoc signed for local use. A protocol token, when
provided, is stored in the signed bundle's `Resources/remote-app.json`; prefer
the Tailscale-authenticated remote-share binding so no reusable token needs to
be embedded.

`--sync-window-size` is explicit opt-in. When the server grants the capability,
local native resize events are debounced and applied to the real host window;
host-originated changes resize the proxy in the other direction. Without the
flag, geometry remains observe-only.

During migration, a host that still advertises the former protocol name can
be reached explicitly with `--protocol-name crdp`. The client never silently
downgrades the protocol name; omit the option for current RCDP hosts.

For a loopback development daemon protected by `cua-spacesd --token`, pass the same
secret as `--token TOKEN`. Tailscale app shares authenticate at the server's
HTTP upgrade boundary and do not use this protocol token.

## Build and connect on Windows

Build and stage all Windows product binaries from PowerShell:

```powershell
.\scripts\build-windows.ps1 -Configuration release -RunTests
cd .\target\windows\RCDP
.\cua-viewer.exe --url ws://HOST:3211 --target OPAQUE_HANDLE --sync-window-size
```

The package is a directory rather than an installer. `cua-viewer.exe` is the
native one-window client; `cua-spacesd.exe`, the HTML5 client, example
app menu, fixture, licenses, and notices are staged beside it. OpenH264 is
linked from source, so the native media path does not require a separate codec
DLL.

## Transport and media

The native clients prefer H.264 at up to 60 frames per second and 8 Mbps by
default, with configurable `--max-fps`, `--max-dimension`, and
`--max-bitrate-kbps` ceilings. macOS decodes in VideoToolbox; Windows uses
OpenH264 and converts decoded YUV into the same packed BGRA renderer input.
Codec epochs recreate native decoder state, and a missing or failed keyframe
causes one bounded recovery request. Tightly packed BGRA remains the negotiated
fallback for hosts without H.264 support.

Pass `--show-stats` to `cua-viewer` to append a
one-second live telemetry HUD to the native title bar. It reports received FPS
and bitrate, control RTT, host encode time, decode time, receive-to-present time, input
acknowledgement time, and client/server frame replacement.
On QUIC it also reports incomplete network frames. QUIC transport RTT is fed
directly into adaptation.

When both peers advertise `input.interactive.v1`, the input lane assigns one
contiguous sequence to every native event, coalesces adjacent text commits,
and allows up to 32 acknowledged batches in flight. The host acknowledges the
highest sequence only after native posting, preserving order and bounded
backpressure without turning a WAN RTT into a per-event stop-and-wait gate.
Older peers retain the legacy action lane.

CUA's ordinary one-shot automation tools retain their post-action observation,
focus containment, and effect checks. Interactive RCDP sessions instead reuse
one `CGEventSource` and native worker, cache target geometry briefly, commit
Unicode without per-character sleeps, and preserve true key, pointer, drag,
and scroll phases. Capture already supplies continuous frame evidence, so this
path acknowledges native posting rather than polling for a visible effect.

When the server advertises runtime preferences, the client evaluates 500 ms
windows. Datagram loss, rising QUIC RTT, decoder pressure, or replacement in
the one-frame presentation slot first reduces encoder bitrate. Persistent or
local pressure then reduces frame rate and long-edge resolution. Six stable
windows recover quality gradually. Each accepted update restarts only that
session's capture, advances its H.264 codec epoch, and begins from a keyframe.
Servers without the capability receive no update messages.

The renderer always maps actions from the last displayed frame. In observe-only
mode a local resize adds letterboxing and does not invent a new remote geometry
epoch. In bidirectional mode, one controlling session holds the target's resize
lease, sends strictly increasing revisions after a 120 ms debounce, and waits
for capture lifecycle geometry to become authoritative. Server-originated
geometry is tagged locally for one-event suppression so it never echoes back.
Actions outside the displayed frame are ignored.

## Cross-platform client shape

The portable unit is `one RCDP session -> one native top-level window`:

| Concern | macOS | Windows | Linux client |
| --- | --- | --- | --- |
| Top-level surface | AppKit through `winit` | Win32 through `winit` | Wayland/X11 through `winit` |
| Raw renderer | `wgpu`/Metal BGRA texture | `wgpu` Windows BGRA texture | explicit unsupported stub |
| Compressed decode | VideoToolbox H.264 | OpenH264 | VA-API/GStreamer milestone |
| Input source | native window events | native window events | compositor/X11 events |
| Host input | RCDP interactive lease | legacy action path | same |
| Text clipboard | bidirectional, generation-based sync | follow-up | follow-up |
| Regular-file clipboard | bidirectional, bounded and checksummed | follow-up | follow-up |

Only the surface and decoder are OS-specific. Target selection stays opaque,
and target resolution, capture, policy enforcement, and CUA action delivery
stay in the daemon. Linux builds currently produce an explicit unsupported
stub rather than pretending to provide a native client.

## Current limits

- One client process opens one remote window.
- macOS H.264 output stays in VideoToolbox's IOSurface-backed
  `CVPixelBuffer`. The client imports it with `CVMetalTextureCache` and samples
  the exact Metal texture in the `wgpu` surface, avoiding decoded-frame CPU
  copies. Packed BGRA still uses the CPU upload path as a compatibility
  fallback. Windows native-surface import remains a follow-up optimization.
- macOS can project remote application icons and per-app local bundle identity;
  Windows packaging and Start-menu installation remain follow-ups.
- macOS synchronizes bounded UTF-8 text and regular-file clipboards in both
  directions. File transfer is limited to 16 files, 16 MiB per file, and 32
  MiB total; only leaf names and checksummed contents cross the wire. Rich
  clipboard formats, directories, drag-and-drop, audio, IME composition previews, and
  reconnect are not part of the v1 client.
- Bidirectional host resizing is implemented by the macOS and Windows
  providers. Linux hosts remain observe-only until they expose an equivalent
  background-safe geometry primitive.
- A remotely minimized or inactive window remains a native proxy window with a
  suspended status until the host reports resume.
