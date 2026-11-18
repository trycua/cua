# Watch and control in the viewer

A user opens a Space's desktop in the browser from a link the CLI prints, sees the live screen with sound, and works it with the mouse and keyboard.

## Sub-features

- `view-link` prints a ticketed viewer link (`cua sb view <id> --no-open`).
- `view-stream` streams the desktop (H.264, PNG fallback) and the status leaves "Connecting".
- `view-input` delivers clicks at the right place and typed keys to the app under them.
- `view-audio` plays desktop audio when the Space has a sound server.
- `view-no-audio` still streams video when the Space has no reachable sound server.
- `view-index` lists the Space in the `cua viewer` index page and opens it from there.

## How to get to it (user POV)

- `cua sb view <id>` opens the browser (`--no-open` prints the link, `--view-only` watches only).
- `cua viewer` serves an index of every sandbox on one loopback port; a row opens that Space.
- The apps' Space viewer windows use the same stream (unverified here).

## Driving it with spaces-desk

Preconditions:

- Baseline, with `audio=pipewire` in the `READY` line for `view-audio`.
- Chromium is installed (`CHROMIUM=` overrides the binary).

- **Open, stream, click and type.** Run `$H viewer "$RUN" desktop`. It prints `open {"status":"streaming",...}`, a `frames` line with `framesDecoded` >= 1 and the frame `1280x800`, and `input {"click":{"button":1,...,"x":>=640,"y":>=400},"keys":"ok"}`, then `PASS`. `viewer-desktop.png` shows the grid under the toolbar; `pad.jsonl` has the new `button` and two `key` lines.
- **Audio track.** In the same `viewer-desktop.json`, `stats.audio[0].decoded` > 0 and `audioPackets` > 0.
- **No sound server.** Stop audio with `kill` on the `pipewire-pulse`, `wireplumber` and `pipewire` pids from `$EVIDENCE/pids.txt` (only those), then run `$H viewer "$RUN" no-audio`. Expected: `PASS` with video and input as above (no audio track).
- **Audio off by the user.** Run `$H viewer "$RUN" audio-off "audio=none"`. `PASS`; `stats.audio` is empty.
- **Index page.** Run `$H index "$RUN"` (starts `cua viewer --no-open`, follows its keyed URL with a cookie jar, stops it). Expected: `PASS index lists <SPACE_ID>`; `viewer-index.html` has a row linking `/s/<SPACE_ID url-encoded>/open`.

## Gotchas

- PipeWire without a session manager (no WirePlumber) accepts the record stream but never links it: `OpenMedia` then blocks and the viewer sits at "Connecting to the sandbox…". `up` starts WirePlumber for that reason; a missing WirePlumber is a setup failure, not a product verdict.
- The link carries a viewer ticket (a credential, 1 h by default). `drive-viewer.mjs` redacts it in its JSON.
- Input is leased to one viewer at a time for 5 s after its last input. A second viewer opened right after another one gets `RateLimited ... leased to Web viewer` (in `cua-spacesd.log`) and nothing reaches the pad; `drive-viewer.mjs` waits 6 s before acting for that reason.
- The click lands in pad coordinates: 958,579 for the 75%/75% canvas point, because openbox's title bar takes the top 19 px.
