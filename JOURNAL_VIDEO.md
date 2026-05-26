# Recording rename + cross-platform video — JOURNAL

**Started:** 2026-05-26 ~13:15
**Branch:** `cua-driver-rs-recording-rename-video` (local only, no PR per instructions)
**Base:** `origin/main` @ `02f1f033` (MSAA fallback PR merged earlier today)
**Time budget:** ~2 hours, working autonomously
**Goal:**
1. Rename `set_recording` MCP tool → `start_recording` + `stop_recording` (two tools)
2. Promote `video_experimental` flag → default-on video recording (drop the
   `_experimental` suffix; video records by default)
3. Implement cross-platform video capture (macOS / Windows / Linux). The user
   notes the Swift implementation worked; the Rust port hardcodes
   `video.present: false` on all platforms.

**Out of scope per user:** no PR, no remote pushes. Local commit on the
working branch only.

---

## Survey results (15 min in)

**Swift reference** (`libs/cua-driver/swift/Sources/CuaDriverCore/Recording/VideoRecorder.swift`):
- ScreenCaptureKit (SCStream) → AVAssetWriter
- MP4 / H.264 High profile, 30 fps, main-display-only
- Synchronized via NSLock; clean teardown on permission revocation

**Rust port** (`libs/cua-driver/rust/crates/mcp-server/src/recording.rs` +
`recording_tools.rs`):
- `RecordingSession` writes turn folders + `session.json` (hardcoded
  `"video": { "present": false }` — line 103)
- No video capture infrastructure at all on any platform
- The `video_experimental` flag is accepted by the tool schema but only
  emits a text note "(not yet implemented on this platform)" — that note is
  also being stripped by the MCP `structured_content` shape so the warning
  is invisible to callers

**Tool registry** (`tool.rs:79-84`):
- `register_recording_tools()` registers `SetRecordingTool`,
  `GetRecordingStateTool`, `ReplayTrajectoryTool`. CLI dispatch under
  `cua-driver/src/cli.rs` knows the name `"set_recording"`.

**ffmpeg on this host:** NOT installed. `where ffmpeg` returns nothing.

---

## Strategy decision

Cross-platform video capture options I considered:

| Strategy | Pros | Cons | Verdict |
|---|---|---|---|
| Native APIs per platform (ScreenCaptureKit / Windows.Graphics.Capture / PipeWire) | Zero runtime deps, matches Swift impl | Weeks of work × 3 platforms; D3D11/DXGI boilerplate on Windows alone is ~500 lines | ❌ not 2-hour-feasible |
| `scap` Rust crate (cross-platform capture, returns BGRA frames) | Cross-platform, native APIs underneath | Still need encoder → mp4. `mp4` crate exists but no built-in H.264 encoder; would need to vendor x264 or another encoder. Bigger than the 2-hour budget. | ❌ |
| `ffmpeg-next` (libffmpeg bindings) | Full encoder | Requires linking against system ffmpeg libs; build complexity, cross-compile woes | ❌ |
| **ffmpeg subprocess** | Cross-platform single binary, simple stdout/stdin handling, robust, fast to ship | External dep (ffmpeg must be on PATH); user has to install it | ✅ |

**Going with ffmpeg subprocess.** The trade — "user installs ffmpeg" — is the
right one for this budget. The implementation becomes "spawn ffmpeg with the
right `-f <device> -i <source>` flags per platform, kill on stop, surface a
clear error when ffmpeg isn't on PATH." Platform-specific input device:

- **Windows:** `ffmpeg -f gdigrab -framerate 30 -i desktop -c:v libx264 -preset ultrafast -pix_fmt yuv420p <output>.mp4`
- **macOS:** `ffmpeg -f avfoundation -framerate 30 -i "1:" -c:v libx264 -preset ultrafast -pix_fmt yuv420p <output>.mp4` ("1" = main display, ":" = no audio)
- **Linux (X11):** `ffmpeg -f x11grab -framerate 30 -i :0.0 -c:v libx264 -preset ultrafast -pix_fmt yuv420p <output>.mp4`
- **Linux (Wayland):** harder — needs PipeWire / wf-recorder. Skip for now, ship X11 path, document.

Encoder choice: `libx264` with `-preset ultrafast` keeps CPU low; `-pix_fmt yuv420p` ensures broad player compatibility (some platforms default to yuv444p which QuickTime / Windows Media Player won't decode).

## Plan

1. **Tool rename** — split `SetRecordingTool` into `StartRecordingTool` (takes
   `output_dir`, optional `record_video`) and `StopRecordingTool` (no args).
   Update `tool.rs` registration + CLI dispatch + tests + skill docs.
2. **Video runner abstraction** — new `VideoRecorder` trait + per-platform
   impl. Each platform spawns ffmpeg with the right flags. Trait methods:
   `start(path: &Path) -> Result<()>`, `stop() -> Result<FinalMetadata>`.
3. **Wire into RecordingSession** — `start_recording` constructs a
   `VideoRecorder` per host, calls `start()`. `stop_recording` calls
   `stop()`, records metadata in `session.json`.
4. **Promote out of experimental** — drop `_experimental` suffix. Default
   `record_video: true` (caller can opt out with `false`).
5. **ffmpeg-not-found error** — surface a structured error pointing at the
   relevant package manager (winget install Gyan.FFmpeg / brew install ffmpeg
   / apt install ffmpeg).
6. **Test** — install ffmpeg, run the 5+7 calc demo with the new API, verify
   the MP4 plays.

Will journal each step as I land it.

---

## Phase 2: tool rename (done ~50 min in)

`SetRecordingTool` → `StartRecordingTool` + `StopRecordingTool`.

**Schema changes:**
- `start_recording`: required `output_dir`, optional `record_video` (default
  **true**), nothing else. No more `enabled` boolean — verb-first matches CLI.
- `stop_recording`: no args. Idempotent.

**Files touched in this phase:**
- `mcp-server/src/recording_tools.rs` — rewrote the tool defs
- `mcp-server/src/tool.rs` — registry update + recording exclusion list
  updated (`start_recording` / `stop_recording` get excluded so the
  recorded turn stream stays the user-action sequence, not the meta
  start/stop frames)
- `mcp-server/src/recording.rs` — `RecordingSession` grew `start()`/`stop()`
  methods; old `configure()` kept as a thin shim so the bridge layer
  doesn't break mid-refactor
- `cua-driver/src/cli.rs` — `cua-driver recording start|stop` subcommand now
  dispatches to the two new tool names (the CLI subcommand verbiage was
  already verb-first; this just realigns the underlying tool name)
- `platform-windows/examples/list_tools_parity.rs` — registry list updated
- `platform-windows/examples/recording_parity.rs` — rewrote against the new
  surface (dropped the Swift-parity framing since the rename intentionally
  breaks parity with the Swift CLI)
- `cua-driver/tests/mcp_protocol_test.rs` — all `set_recording` references
  converted; `test_set_recording_video_experimental_accepted{_windows}` →
  `test_start_recording_record_video_flag_accepted{_windows}`
- `Skills/cua-driver/SKILL.md` + `RECORDING.md` — doc updates; the
  RECORDING doc now headlines "Video on by default" near the top

The `configure()` shim kept the migration safe — built incrementally and
no callers broke between commits.

## Phase 3: video promotion (done in the same rename pass)

Dropped `video_experimental` entirely. New API: `record_video: true`
default. When `record_video: false` is passed, behaves identically to
the old non-video code path. When omitted or `true`, the recording
session spawns ffmpeg.

The `_experimental` suffix was a code smell — flags named like that
either get promoted or never used. Promoting it now and committing to
the contract is the right call given the user said "should be out of
experimental and be enabled by default."

## Phase 4: cross-platform video capture (done ~90 min in)

New module: `mcp-server/src/video.rs`.

**`VideoRecorder` lifecycle:**
- `VideoRecorder::start(path)` — spawns ffmpeg with platform-appropriate
  flags writing to `path`. Returns the recorder handle.
- `VideoRecorder::stop()` — sends `q\n` on ffmpeg's stdin (ffmpeg's
  clean-shutdown trigger that finalizes the mp4's moov atom), polls for
  exit up to 3 s, falls back to `kill()`. Returns `VideoMetadata` with
  `duration_ms` + `finalized` so the caller can decide what to do with a
  forcibly-terminated file.

**Platform-specific ffmpeg input args:**
- Windows: `-f gdigrab -framerate 30 -draw_mouse 1 -i desktop`
- macOS:   `-f avfoundation -framerate 30 -pix_fmt uyvy422 -i 1:`
- Linux:   `-f x11grab -framerate 30 -i $DISPLAY`

**Cross-platform encoder flags:** `libx264 -preset ultrafast -pix_fmt
yuv420p -movflags +faststart -g 30 -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2"`.
The padding filter was a learning — yuv420p needs even dimensions and
the host's 949 px screen height made the first run produce a 0-byte mp4
(`height not divisible by 2 (1512x949)`). Padding by 1 px on the
bottom/right beats cropping (keeps the full display in frame).

**ffmpeg discovery:** `find_ffmpeg()` first probes `ffmpeg` on PATH,
then falls back to well-known package-manager install locations
(winget Gyan.FFmpeg / Homebrew / apt). Lets a fresh winget install
work without a shell restart — caught a real bug since the cua-driver
process inherits PATH from the parent shell which doesn't see winget's
freshly-added entries until the shell is restarted.

**Stderr drain:** Spawned a thread that drains ffmpeg's stderr into a
4 KB ring buffer. Two reasons: (1) ffmpeg's stderr pipe can fill up
and block the encoder, (2) when the process exits non-zero we want to
log the tail of stderr at `warn` level so the failure mode isn't
silent — that's how I caught the height-divisible-by-2 issue inside
2 min.

## Phase 5: end-to-end test (done ~110 min in)

`flash-repro/test_video_recording.py` — minimal: start, sleep 5s, stop,
verify the mp4 plays. Result:
- mp4: 1.94 MB, H.264, 1512×950 (after padding), yuv420p, ~7 s duration,
  `finalized: true`. ffprobe accepts it cleanly.

`flash-repro/test_video_calc_demo.py` — full agent flow:
- `start_recording(output_dir=…)` (video default on)
- `launch_app(Calculator)` → recorded as turn-00001
- 4 clicks (5, +, 7, =) → turn-00002 through turn-00005
- `get_window_state(query: Display)` → not recorded (read-only)
- One final get_window_state recorded as turn-00006 because it's not
  read-only-tagged (TODO: this is actually a small bug; get_window_state
  should be marked read-only)
- `stop_recording()` → mp4 finalizes
- Final: 6.6 MB mp4, 6 turn folders, `Display is 12`, session.json
  carries full video metadata

A regression I caught with the existing test suite: turn-00001 was
being filled with `start_recording` itself before I added the names to
the exclusion list in `tool.rs::ToolRegistry::invoke()`. Test
`test_recording_session_windows` failed loud and fast — fixed and
green.

## Status at handoff

**Done:**
- `set_recording` → `start_recording` + `stop_recording` rename
- `video_experimental` flag dropped; new `record_video: true` default
- ffmpeg-subprocess video capture wired up; works end-to-end on Windows
  (verified with calc 5+7 demo, 6.6 MB mp4, finalized:true)
- Cross-platform input args coded for macOS (avfoundation) and Linux
  (x11grab) — **not validated** since I'm on Windows
- Updated tests + examples + skill docs

**Not done / known limitations:**
- macOS and Linux ffmpeg branches are coded but unverified — I expect
  they work but the avfoundation device-index "1" might need to be
  resolved programmatically per-host (the Swift impl did via
  SCShareableContent). Worth a smoke test from a Mac.
- Wayland Linux path is missing — x11grab only. PipeWire / wf-recorder
  is the right backend there.
- The `get_window_state` tool isn't tagged `read_only` despite reading
  the UIA tree, so it's being recorded as a turn. Minor, not a blocker
  for the recording feature itself.
- Audio capture: not done. Recording is video-only.
- Per-window video: not done. Currently records the main display.
  Trivially could be a future flag.
- This change breaks Swift-CLI parity intentionally. The `recording_parity.rs`
  example was rewritten to validate the new surface instead.

**One concrete TODO I'd file as a separate ticket:** `cua-driver doctor`
should grow an `ffmpeg` check that surfaces the same install-hint message
the start_recording tool surfaces in `last_error`. Right now the only
way to learn "video doesn't work because ffmpeg isn't installed" is to
actually call start_recording and inspect `last_error`.

**Committed locally on branch `cua-driver-rs-recording-rename-video`.**
No PR per instructions.

