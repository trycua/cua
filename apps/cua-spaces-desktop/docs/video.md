# Live video in the Electron app

How the Electron app shows a Space's live desktop, how that matches the
SwiftUI app's native video (`apps/cua-spaces-macos/docs/native-video.md`),
and how to measure both apps on the same Space.

## Design

The SwiftUI app draws video natively: VideoToolbox surfaces over the web view,
at the rects the page reports. The Electron app has no native layer. The page
decodes each stream itself with WebCodecs into a `<canvas>`, and Chromium
decodes on the GPU: VideoToolbox on macOS, Media Foundation (D3D11) on Windows,
and VA-API on Linux.

The video never passes through IPC:

1. The page asks the main process for a ticket (`spaces.openStream`).
2. The main process gets it from the cua daemon (`src/media-bridge.ts`).
3. The page's `MediaSession` opens a WebSocket straight to the daemon's
   loopback listener (rcdp wire v2).

| | SwiftUI app | Electron app |
|---|---|---|
| Tiles (Spaces grid) | `VideoTier.tile`: 10 fps, 960 px long edge, view only | the same (`tier: "tile"`) |
| Viewer (a Space's page) | full rate and size, `allow_activation`, input | the same (`tier: "full"`) |
| Picture in picture | `StreamPiPWindow`: a floating panel, its own session | a floating window, its own session (`src/pip.ts`) |
| One window (PiP) | `background_only`, reopened with activation on `would_require_activation` | the same (`windowId`, `activate`) |
| Decode | VideoToolbox, one session per Space and tier | WebCodecs `prefer-hardware` with `optimizeForLatency`, one session per slot; software when the hardware decoder rejects the stream |
| Keyboard | a click takes it; ⌘Esc, a click on the page or Stop controlling gives it back; plain Esc goes to the Space | the same (Ctrl+Shift+F12 off the Mac) |
| ⌘ on a Linux or Windows Space | sent as Control | the same (`metaAsControl`) |
| Scrolls | to the page until the viewer has the keyboard | the same (`scrollNeedsFocus`) |
| Hidden window | every session stops; reopens when shown | the same (`visibilitychange`: minimized, fully covered, another desktop) |
| Connecting, failed | "Connecting…" under the slot; a tile keeps its thumbnail, the viewer falls back to Open window | the same phases (`webcodecs-slots.ts`) |

### Where it lives

- `apps/cua-spaces-web/src/components/video/webcodecs-slots.ts` holds the
  slots: their tickets, sessions, focus, pausing, the activation reopen and
  the per-slot config (`sessionConfig`, `streamTarget`).
- `libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession.ts` is the
  production session. The options this shell sets are:
  - `hardwareAcceleration`;
  - `metaAsControl`;
  - `scrollNeedsFocus`;
  - `onInputAck`;
  - `onFrameTiming`.
- `src/gpu.ts` sets Chromium's switches.
  - macOS and Windows decode on the GPU by default.
  - Linux ships VA-API decode off, so the app turns on
    `AcceleratedVideoDecodeLinuxGL` and `AcceleratedVideoDecodeLinuxZeroCopyGL`.
  - Chromium's GPU blocklist still applies.
  - `CUA_SPACES_VIDEO_DECODE=software` turns hardware decode off, so you can
    compare.
  - The bench's stats and report carry Chromium's verdict as `videoDecode`
    (`app.getGPUFeatureStatus().video_decode`, what `chrome://gpu` shows):
    `enabled` means the GPU decodes, `unavailable_software` means it does not.

### Latency against the SwiftUI app on macOS

Both apps decode with VideoToolbox, so decode time should be the same. The
difference is the path to the screen. The bench measures the part both apps
can time (`decodeMs`, arrival to the frame reaching the view).

**The SwiftUI app.** The SDK's delivery thread feeds the decoder. The decoded
IOSurface goes to a layer on the main actor, and Core Animation composites it.

**The Electron app.** Each frame takes these steps:

1. The network service passes the frame to the renderer (one IPC hop).
2. The renderer's main thread parses it, sharing that thread with React.
3. `VideoDecoder.decode` sends it to the GPU process (another hop), where
   VideoToolbox decodes it.
4. The decoded frame comes back to the renderer as a GPU-backed `VideoFrame`.
5. It is drawn onto an accelerated canvas.
6. The canvas is shown at the next compositor frame, through Chromium's
   display compositor and then Core Animation.

That should add about one display frame: 8 to 17 ms at 60 Hz, more while
the page's main thread is busy. The bench reports arrival to paint (p50,
p95, p99) so this can be checked rather than assumed.

If the measured gap is larger than that, these are the steps to take, in
order.

1. **Present without the canvas.** Write decoded `VideoFrame`s to a
   `MediaStreamTrackGenerator` shown in a `<video>`. Chromium then promotes
   the video to a Core Animation overlay, skipping the canvas copy and
   usually one compositor frame. `requestVideoFrameCallback` then gives
   true presentation times. This is a change to `MediaSession`'s drawing
   only.
2. **Move the socket and decode off the main thread.** Run them in a
   worker, drawing to an `OffscreenCanvas` or a worker
   `MediaStreamTrackGenerator`. Busy React work then no longer delays
   frames.
3. **A native layer.** Only if measurements still show a gap after those
   two. This is the SwiftUI app's `WebUIVideoSurfaces` again, as a Node
   addon:
   - Objective-C++ on macOS: `CALayer`s over the BrowserWindow's
     `NSView` (`getNativeWindowHandle`), fed by the SDK's stream session
     and VideoToolbox, at the rects the page already reports (the
     `cuaVideo` protocol in `lib/video-slots.ts`), with input mapped as
     `LiveStreamInputView` maps it.
   - Windows: DirectComposition visuals.
   - Linux: X11 or Wayland subsurfaces, a different design per compositor.

   It is weeks of work per platform, and Linux is the hardest. Do not
   build it unless the bench proves it is needed.

## The bench, both apps on one Space

Both apps have the same hooks:

| Hook | What it does |
|---|---|
| `CUA_SPACES_VIDEO_BENCH=<spaceId>` | Opens the Spaces grid (its tiles) plus a second window on that Space's viewer. The windows are placed the same way in both apps. |
| `CUA_SPACES_VIDEO_STATS=<file>` | Writes every stream's counts once a second. |

The counts mean the same in both apps:
- `decoded` is frames out of the decoder;
- `presented` is frames handed to the view: the layer in the SwiftUI app, the
  canvas here;
- fps is presented frames per second over the window, as
  `scripts/native-video-harness.sh measure` computes it.

Both apps time each presented frame from its arrival (`decodeMs`). In the
SwiftUI app that runs from the SDK handing over the frame to its decoded
image reaching the layer; here, from the packet arriving on the socket to the
decoded frame being drawn onto the canvas. Neither includes the compositor,
so this is the like-for-like latency.

The Electron app also counts:
- `received`: frames whose packet arrived;
- `painted`: frames on screen at the next compositor frame
  (`requestAnimationFrame`);
- `dropped`: frames that arrived but were never painted;
- arrival-to-paint latency (`latencyMs`) per painted frame.

The SwiftUI app's report leaves these null.

`scripts/video-bench.mjs` runs either app with those hooks. It waits for the
streams to warm up, then measures CPU and memory of the app's processes over
the window, the same way for both:
- macOS: `top`, counting the app, its helpers, and the WebKit services
  started for the SwiftUI app;
- Linux and Windows: the process tree.

It then writes one report (`src/video-report.ts`):

```json
{ "app": "electron", "seconds": 30, "cpuPercent": 14.2, "cpuMax": 21, "memoryMB": 610,
  "tiles": [{ "spaceId": "…", "fps": 10, "decoded": 300, "presented": 300, "received": 300, "painted": 300,
              "dropped": 0, "droppedPercent": 0, "latencyMs": { "samples": 300, "mean": 21.4, "p50": 20.1, "p95": 31, "p99": 38, "max": 44 },
              "decodeMs": { … }, "size": "960x600", "failure": null }],
  "full": [{ … }] }
```

### Running both on one Mac

Prepare:

1. Pick a running Space that streams.
2. Quit both apps, so only one draws video at a time.
3. Use the same display and window sizes for both runs.

Run the Electron app, packaged and signed, from the DMG or the CI artifact:

```sh
cd apps/cua-spaces-desktop
node scripts/video-bench.mjs electron --app "/Applications/Cua Spaces.app/Contents/MacOS/Cua Spaces" \
  --space <spaceId> --seconds 30 --out electron.json
```

Run the SwiftUI app. Its hooks are in debug builds only:

```sh
(cd apps/cua-spaces-web && pnpm build) && (cd apps/cua-spaces-macos && swift build)
cd apps/cua-spaces-desktop
node scripts/video-bench.mjs swift --app ../cua-spaces-macos/.build/debug/CuaSpacesMac \
  --space <spaceId> --seconds 30 --out swift.json
```

Compare `tiles[].fps`, `full[].fps`, `full[].decodeMs` (p50, p95),
`cpuPercent` and `memoryMB` across the two reports. Read `full[].latencyMs`
and `droppedPercent` from the Electron report: they add the compositor's
frame, which the SwiftUI app does not measure.

Without the script, the Electron app can bench itself:

```sh
CUA_SPACES_VIDEO_BENCH=<spaceId> CUA_SPACES_VIDEO_REPORT=electron.json \
CUA_SPACES_VIDEO_SECONDS=30 "<the app>"
```

It writes the same report, with CPU and memory from Electron's
`app.getAppMetrics`, then quits. `CUA_SPACES_VIDEO_DECODE=software` gives the
software-decode baseline.

Notes:
- The script needs Node 22.18 or later, which strips the types of
  `src/video-report.ts`.
- The Electron hooks work in packaged builds too. They only open windows
  and write the files you name.
