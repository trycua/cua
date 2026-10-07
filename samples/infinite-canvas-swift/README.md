# infinite-canvas (Swift)

Every window of every Space on one zoomable canvas. A global hotkey opens it;
each window is a live tile you can move, resize, zoom into and use. Agent and
coworker cursors show up over the windows they act in, and agent threads are
canvas windows of their own. Inspired by Zsolt Kacso's
[infinite canvas for Omarchy](https://x.com/kaolti/status/2103257208616104111).

SwiftUI and AppKit on the Cua Swift packages (`Cua`, `CuaSpaces`,
`CuaSpacesStreaming`), macOS 26 (Liquid Glass).

The sample links Cua Spaces pieces (`CuaSpacesStreaming`, in
[`libs/spaces-app-swift`](../../libs/spaces-app-swift)), so it is
source-available under the
[Functional Source License, Version 1.1, MIT Future License](LICENSE)
(FSL-1.1-MIT), like Cua Spaces itself. See [LICENSING.md](../../LICENSING.md).

## Run

```sh
(cd ../../libs/cua && cargo build --release -p cua-sdk && node scripts/stage-uniffi-library.mjs --only=swift)
swift build -c release
.build/release/infinite-canvas --spaces spaces.json --show
```

`spaces.json`:

```json
{
  "spaces": [
    {"label": "macOS", "os": "macos", "url": "http://192.168.64.2:3211", "token": "…", "maxWindows": 4,
     "apps": ["Calculator", "TextEdit"]},
    {"label": "Linux", "os": "linux", "url": "127.0.0.1:3211", "token": "…", "maxWindows": 4,
     "agent": {"harness": "claude-code", "env": {"ANTHROPIC_API_KEY": "…"}}}
  ],
  "threads": ["Linux"]
}
```

Every Space is an existing machine running cua-spacesd, added to a throwaway
registry (`--registry DIR`, default a temp directory; `~/.cua` is never read
or written). The canvas never creates or deletes a Space.

| Flag | |
|---|---|
| `--hotkey option+space` | the global hotkey (Carbon, no Accessibility permission) |
| `--show` | open at launch |
| `--windowed 1600x1000` | a normal window instead of the full-screen overlay |
| `--chromeless` | with `--windowed`: borderless, for window-only recordings |
| `--control SOCKET` | a local control socket for scripted runs (0600) |
| `--synthetic N` | N local H.264 test-pattern tiles (benchmark and tests only) |
| `--perf-hud` | tiles decoding and frames requested, in the status pill |

Keys: the hotkey hides, Return zooms into the selected tile, ⌘0 fits all,
Esc steps back (search, then focus, then the overlay), typing searches window
titles and Tab moves between matches. Click a tile to give it input; drag it
by its title or while unfocused; resize from its bottom-right corner.

## Architecture

| Target | What it is |
|---|---|
| `CanvasModel` | Pure value types, no AppKit or SDK: `Camera` (zoom about a point, fitting, van Wijk smooth flights), `CanvasLayout` (tiles, z order, aspect-locked resize, justified-row arrangement), `LODPolicy`/`LODState` (tiers with hysteresis and a downgrade hold), `OverlayState` (hotkey toggling, debounce, reversal), `PresenceDirectory` (members, thread bindings, styles from the SDK's presence colors), `CursorGlide` (critically damped spring), `HumanPath` (a person's pointer path and typing cadence), `HoverInfo`, `StreamErrorMessage`, `FrameStats` |
| `CanvasStreaming` | One tile's pipeline: `SpaceMediaSource` (a cua SDK `SpaceStreamSession` on one window), `TileDecoder` (VideoToolbox, hardware required, NV12 IOSurfaces, enqueued straight into an `AVSampleBufferDisplayLayer`), `TileStream` (level of detail applied as the media socket's `set_stream_preferences`, keyframe requests, ping latency, fps), `SyntheticMediaSource` (a local H.264 test pattern), `ProcessMetrics` |
| `InfiniteCanvasApp` | The overlay window and hotkey, `CanvasScrollView` (the world is a huge layer-backed `NSScrollView` document; tiles are subviews at fixed world positions), tile views, the agent cursor (Cua Driver's `cua.default.lottie`, read and drawn natively), agent threads over ACP (`SpacesdClient.agents()`), presence (`SpacePresence` + the SDK's `PresenceRoster`), app icons fetched from each Space, the HUD, the control socket and the perf sweep |
| `CanvasBench` | `canvas-bench`: the model's hot paths and the decode pipeline, headless |

Performance design:

- Pan and zoom are the scroll view's own, on the compositor: a gesture changes
  one clip-view bounds. No tile moves, no layout runs, no SwiftUI view
  updates per frame. The HUD reads a camera copy refreshed at most 20 times a
  second.
- Frames never touch the main thread or SwiftUI: SDK delivery thread, then
  VideoToolbox, then the display layer's renderer.
- Level of detail per tile from its drawn size: paused (off screen: 1 fps at
  160 px from the Space, not decoded), thumbnail (4 fps, 360 px), low (12,
  720), medium (30, 1280), full (60, 2560, or when focused). Upgrades are
  immediate and ask for a keyframe; downgrades wait 450 ms and need an 18%
  margin. While the overlay is hidden everything is paused.
- A thread window (SwiftUI) is swapped for a bitmap of itself while the zoom
  changes and restored 200 ms after.
- `os_signpost` intervals under `ai.cua.infinite-canvas`: `decode`, `tick`,
  `sweep`, and `tier` events.

## Performance

Measured on an M5 Max, 120 Hz ProMotion display, release build, a 1440x810 pt
window. The sweep is the control socket's `perf` command: 20 or 30 s of
pan (Lissajous) and log-scale zoom (overview to 140%) driven once per display
frame, which crosses every level-of-detail transition. Frame times are the
display link's intervals.

| Run | fps | late (>12.5 ms) | worst frame | main thread | process CPU | GPU (mean / max) | memory |
|---|---|---|---|---|---|---|---|
| 8 live tiles (4 macOS, 4 Linux), 20 s | 119.9 | 1 | 20.8 ms | 10% | 12% | 26% / 42% | 229 MB |
| 10 live tiles (+2 Omarchy), 30 s | 119.3 | 5 | 166 ms | 17% | 23% | 1% / 38% | 234 MB |
| 8 live tiles before the thread-window freeze | 117.5 | 19 | 170 ms | 50% | 54% | 28% / 44% | 239 MB |
| 8 synthetic 60 fps tiles (debug build), 10 s | 119.7 | 2 | 23.8 ms | 13% | 64% (incl. encoding) | 25% / 35% | 270 MB |

All tiles decoded in hardware in every run. Decode latency per frame (submit
to output), live tiles: p50 0.6 to 1.2 ms, p95 1.4 to 2.6 ms. GPU is
whole-machine utilization (IOAccelerator), so it includes the rest of the
Mac. The 166 ms frame is the first freeze of the thread window (one bitmap
render); CPU and memory are this process.

`canvas-bench --tiles 8 --seconds 10` (release):

| | |
|---|---|
| level of detail for 100 tiles | 1.1 µs per frame |
| one camera flight sample | 19 ns |
| arranging 60 tiles | 15 µs |
| 8 tiles at full (60 fps asked) | 475 decoded fps, p50 0.59 ms, p95 0.76 ms, hardware, 149 MB |
| 8 tiles at mixed tiers | 150 decoded fps, 47 MB |

Instruments is not available on this machine (Command Line Tools only, no
Xcode), so the numbers come from the app's own counters and signposts.

## Presence

- An agent's cursor points along its direction of travel (the driver
  overlay's convention: heading = travel + π, resting at π/4), low-passed so
  it does not jitter and eased back upright at rest; it pivots on the tip.
  People's pointers stay upright.
- An agent's cursor is the Cua Driver default theme, tinted with the color
  the Space's presence service assigned it. Its thread window's avatar uses
  the same color (before the agent joins, the SDK's stable `presence_color`
  for the thread). Message bubbles are neutral.
- Before the agent acts, its cursor waits as a glyph in the thread window's
  title; it glides out to the first window it acts in and home when the turn
  ends.
- A harness that opens two MCP sessions gets two driver cursors; while one
  thread's turn runs in a Space, the second is drawn as the first.
- People are a plain pointer in their color. `{"cmd": "coworker"}` on the
  control socket joins a second, human participant whose pointer follows
  `HumanPath` and whose click and typing are real `interactive_input`.

## Tests

```sh
scripts/test.sh                     # 59 tests
scripts/sync-cursor-assets.sh --check
```

Hermetic: the model, the tile pipeline on the local test pattern
(VideoToolbox encode and decode), presence and dock logic, icon lookup and
fallback, stream errors, and a UI smoke test in an offscreen window (frames
present in hardware, a flight ends focused, level of detail pauses off-screen
tiles, search selects, the hotkey presents and dismisses). Nothing connects
to a Space. `snapshots/` holds images the suite writes (the stream error and
the stats card).
