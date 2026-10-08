# Native video in the new UI (macOS)

Live Space video in the web UI (`apps/cua-spaces-web`) on the SwiftUI host: the shell draws the video natively, and the React layout says where it goes.

It is on whenever the New UI is used (a Space's page shows its desktop inline). To opt out:

```sh
defaults write com.trycua.spaces.macos WebUINativeVideo -bool NO    # off
CUA_WEBUI_NATIVE_VIDEO=0 / =1                                       # off or on for one run
```

## What you get

- **Space tiles** on the Spaces grid show the live desktop of each running Space. A tile streams at 10 fps with a 960 px long edge (`VideoTier.tile`). It takes no input: clicks, hovers and scrolls go to the page.
- **The Space viewer** on a Space's page shows its desktop at the Space's full rate and size, and takes pointer, scroll and keys through the same path as the native Space window.
- **Fallbacks.** No video here (the browser, Electron, the experiment off) means the page looks as it did before: the drawn thumbnail on a tile, and "Open window" on the viewer. A stream that can't open or fails falls back the same way. The viewer then says "Live video didn't start here." Nothing is ever left as a blank box. While a stream connects, the viewer shows "Connecting…" and a tile keeps its thumbnail.

## How it works

### The page

These files are in `apps/cua-spaces-web/src`:

- `lib/stream-surface.ts` defines the protocol and finds the `cuaVideo` handler. The host registers that handler only when the experiment is on, so its presence is the flag.
- `lib/video-slots.ts` has the reporter. Only hosts with the handler load it.
- `components/video/use-video-slot.ts` is the hook. `components/video/tile-video.ts` covers tiles, and `components/space-detail/stream-surface.tsx` covers the viewer.

The page keeps one reporter, with one set of observers and one animation frame. It sends one message per frame, and only for slots that changed:

```
{ type: "surfaces", update: [{ surfaceId, spaceId, tier: "tile" | "full", interactive,
                               rect, clip, radius, occluded, visible }], remove: [surfaceId] }
{ type: "focus", surfaceId | null }
```

- **`rect`** is the video area inside the slot's border, in CSS px from the web view's top left.
- **`clip`** is the part of `rect` that the viewport and every clipping ancestor leave visible. For example, a tile scrolled halfway under the page's top edge is cut there.
- **`occluded`** is true when page UI covers the slot. Two checks feed it:
  - an element matching `[role=dialog|alertdialog|menu|listbox|tooltip]` or `[data-video-occluder]` (toasts, tooltips) overlaps the slot;
  - something else is on top at one of 9 sample points (a dialog's backdrop, for example).

  The sample points sit inside the rounded corners.
- **`visible`** is false when the page is hidden or the slot is clipped away entirely.

**When it measures.**
- **Events:** the slot's or the page's resize, a scroll in any container (in the capture phase), a window resize, a `devicePixelRatio` change, `visibilitychange`, and DOM mutations (portals mounting and `data-open` or `style` changes).
- **After each event:** every frame for 20 frames, so CSS transitions are followed to the end.
- **Safety net:** a poll every 400 ms while any slot is mounted.

Route changes unmount and mount slots, and those send `remove` and `update`.

**What the host sends back** on `cua:event`:
- `video.surface {surfaceId, state: connecting | live | failed, reason?, opening?}`
  (`opening`: no stream could be opened at all, so the viewer says why
  once, as the SwiftUI detail's banner; a stream that failed once open
  shows only the cover's Try again)
- `video.focus {surfaceId | null}`

### The host

These files are in `Sources/CuaSpacesMacKit/WebHost`:

- **`WebUIVideoSurfaces.swift`** holds the surfaces, sessions, focus and pausing.
- **`VideoSurfaceLayout.swift`** holds the message parsing and the rect-to-frame mapping. Both are pure functions with unit tests.
- **`SyntheticStreamProvider.swift` and `WebUIVideoBench.swift`** are the harness.

**Two views per slot.**
- A clip container sits at `clip`, over the web view.
- Inside it, a `LiveStreamInputView` sits at the full `rect`, rounded to the slot's radius. This is the same view, VideoToolbox decode and input mapping that the native Space window uses.

CSS px become points through `pageZoom × magnification`. A web view that isn't flipped counts y from the bottom. Each `surfaces` message is applied in one Core Animation transaction with actions disabled.

**Streams.**
- There is one `LiveStreamSession` per Space and tier, shared by every slot that shows it. It opens through `backend.streamProvider(id:)`, the native window's path, with `SpaceStreamProvider.maxFPS` and `.maxDimension` set from the tier.
- The viewer joins presence under the native window's name.
- No new decoder or protocol was added.

**Visibility.** A surface shows only when the page says it can be seen and it has a frame.

**Pausing.** While the window is fully hidden (minimized, covered, or on another Space), every session stops. They open again when the window shows.

**Lifecycle.** A page load (`didCommit`), the web content process ending, and the window closing all tear every surface and session down.

### Input and keyboard focus

- **Tiles** return nil from `hitTest`, so the page gets every click and scroll.
- **Pointer:** the viewer takes pointer input like the native window: moves, clicks, drags and right-clicks.
- **Scroll:** scrolls go to the page until the viewer has the keyboard. After that they go to the Space.
- **Getting the keyboard:** a click on the viewer gives it the keyboard (it becomes the first responder).
- **While it has the keyboard:**
  - the line under the viewer says "Keys go to this Space. Press and release ⌃⌥ to stop." with a **Stop controlling** button;
  - every ⌘ chord (⌘Esc too) goes to the Space, because a local key monitor routes it to the view before the web view takes ⌘C and ⌘V (the view's `KeyCapture`, as in the Space window);
  - on a Linux or Windows Space, ⌘ goes as Control: ⌘C arrives as Ctrl+C and ⌘⇧C as Ctrl+Shift+C. Sent as Command, the Space would get Super, and Super+C types "c". A macOS Space keeps ⌘. The Space window does the same, since both send through `LiveStreamSession`, and so does the HTML5 viewer (`metaAsControl`);
  - plain Esc goes to the Space too. Remote apps need it.
- **Giving it back:**
  - Control+Option pressed and released alone (⌃⌥T and other Control+Option chords still go to the Space);
  - a click anywhere on the page;
  - Stop controlling;
  - the viewer being covered by page UI or leaving the screen.

  Keys then go back to the page, and the page hears it.
- **Without the keyboard:** "Click the desktop to control it." Open in window is always next to it.
- **The pointer:** the system pointer is never hidden or replaced. Like the native window, the viewer also draws its small local cursor dot when the Space has no presence cursor.
- **Input lease:** there is no input-lease UI in the native viewer yet, so there is none here either.

## Harness: synthetic streams

`scripts/native-video-harness.sh` runs the debug app on fixture Spaces in a throwaway HOME, with synthetic H.264. It needs no daemon and no real Space, and the input path, decoder and views are the production ones. To set up the streams:

```sh
../cua-spaces-desktop/scripts/video/encode.sh  # tile30, full60, tile10 … (needs ffmpeg)
```

Then build both apps and run the harness:

```sh
(cd ../cua-spaces-web && pnpm build) && swift build
scripts/native-video-harness.sh check [shots-dir]    # drive it; prints what happened
scripts/native-video-harness.sh measure 30           # CPU, memory, fps
TILE=tile10 scripts/native-video-harness.sh measure  # production tile tier
VIDEO=0 scripts/native-video-harness.sh measure      # same windows, no video
```

**What `check` does.**
- Waits for 9 or more tiles and the viewer to go live.
- Opens New Space over the grid: every tile hides. Cancels it: they show again.
- Clicks the viewer: it gets the keyboard, the page hears it, and the click reaches the Space.
- Types a key, which reaches the Space.
- Presses ⌘C: it reaches the (Linux) fixture Space as Ctrl+C.
- Presses and releases Control+Option: the keyboard goes back to the page.

The last run passed every step (it pressed ⌘Esc, the release chord then).

**Debug hooks** (debug builds only):
- `CUA_SPACES_SYNTHETIC_VIDEO=<dir>`
- `CUA_SPACES_SYNTHETIC_TILE=tile30|tile10`
- `CUA_SPACES_FIXTURE_SPACES=<n>`
- `CUA_SPACES_VIDEO_BENCH=<spaceId>` opens a second window on that Space's viewer.
- `CUA_SPACES_VIDEO_STATS=<file>`: each stream's decoded and presented frames, and each presented frame's time from its arrival (`decodeMs`). The Electron app writes the same file; `apps/cua-spaces-desktop/scripts/video-bench.mjs` runs either app on a real Space and reports both the same way (`apps/cua-spaces-desktop/docs/video.md`).
- `CUA_SPACES_VIDEO_SHOT=<dir>` writes composites: the page snapshot with the native frames drawn in.
- `CUA_SPACES_VIDEO_CHECK=<file>`

## Performance (5 Oct 2026)

**Setup.** Mac Studio, M1 Ultra, 20 cores, 128 GB, macOS 26.6.1, one 1920×1080 display at 1x. The machine was heavily loaded during every run, with a load average of 54 to 104 from other builds. Figures are for 30 s after a 12 s warm-up.

**What runs.**
- A grid window with 10 live tiles: 9 bench Spaces and the sample Space.
- A second window with one viewer at 1080p60.
- Both windows are the real web UI.

**What's counted.** The app plus the WebKit processes it started (2 WebContent, GPU and Networking). Figures come from `top`. 100% CPU is one core.

| Run | Tiles | Viewer | CPU (app + WebKit) | App CPU | Memory, all | Memory, app |
|---|---|---|---|---|---|---|
| No video (`VIDEO=0`) | none | none | 1% | 0.4% | 219 MB | 41 MB |
| 10 × 1280×800@30 (`tile30`) | 29.9 fps each | 59.5 to 59.7 fps | **28 to 31%** (max 45%) | 27% | 273 to 277 MB | 97 MB |
| 10 × 960×600@10 (production tile tier) | 10.0 fps each | 59.9 fps | **13%** (max 24%) | 12% | 271 MB | 92 MB |

**Frames:** every stream held its rate, and no frames were dropped. Production tiles cost less than half the CPU of `tile30` tiles. Total memory is the whole app: two complete React pages in two WebContent processes, at about 75 to 80 MB each, the same with video off.

Not measured: GPU and WindowServer time, battery, and Retina displays.

Scroll tracking was checked by eye only: tiles follow a scroll and clip under the top bar. A tile can trail a fast scroll by a frame, because the page reports once per frame.

## Status

**`WebUINativeVideo` is on by default** whenever the New UI is used, so a Space's page shows its desktop, as the SwiftUI detail always does. `WebUINativeVideo` set to NO and `CUA_WEBUI_NATIVE_VIDEO=0` opt out.

A live E2E should check:

1. **Tiles.** Turn the experiment on and open New UI with 2 or more running local Spaces. Tiles go live within a few seconds. Clicking a tile opens the Space, and scrolling the grid moves and clips the video correctly.
2. **The viewer.** Open a Space. Its viewer goes live and "Click the desktop to control it." shows.
   - Click inside: the click lands in the Space at the right place.
   - Type: keys arrive, including ⌘C and ⌘V inside the Space.
   - Control+Option pressed and released alone: the line goes back to "Click the desktop…".
   - Scroll: the page scrolls before you click, and the Space scrolls after.
3. **Covering.** Open the command palette (⌘K), the Stop dialog and a toast over the viewer. The video hides while each is open, keys go to the dialog, and the video comes back after.
4. **Failure.** Stop a Space while its viewer is open. You should see the stopped state, not a frozen frame. Then delete a Space with an open tile.
5. **Hidden window.** Minimize the window, then restore it. The streams pause, then come back.
6. **The pointer.** The system pointer stays visible throughout.
7. **Cost.** `top` on Cua Spaces with 8 to 10 live tiles stays near the harness numbers.

## Not done yet

- The viewer has no window switcher and no picture-in-picture of its own. The Stream section's PiP buttons still open native panels.
- Presence cursors of other participants aren't drawn in the embedded viewer. The native window draws them in SwiftUI (`PresenceOverlay`).
- A tile and the viewer of the same Space open two sessions. That only happens with two windows open.
- Windows and Linux (Electron, WebCodecs): see `apps/cua-spaces-desktop/README.md`, "Live video".
