# Recording & replaying trajectories

> **Cross-platform.** Recording is available on macOS (native
> ScreenCaptureKit), Windows (ffmpeg + `gdigrab`), and Linux (ffmpeg +
> `x11grab`). Replay is cross-platform as long as the recorded artifacts
> are present.

Session-scoped capture of action sequences + pre/post state, suitable
for demos, regression diffs, and training data. Invoked only when the
user explicitly asks to record — the skill does not auto-enable this.

`start_recording` turns on a session-scoped trajectory recorder. While
enabled, every action-tool call (`click`, `right_click`, `scroll`,
`type_text`, `press_key`, `hotkey`, `set_value`) writes a numbered
turn folder under a caller-chosen output directory. Read-only tools
(`get_window_state`, `list_windows`, `screenshot`, `list_apps`,
permission probes, agent-cursor getters / setters, and the recording
controls themselves) are not recorded.

**Video on by default.** `start_recording` also captures the main
display to `<output_dir>/recording.mp4` (H.264 / 30 fps) for the
lifetime of the session. The mp4 is finalized on `stop_recording`. Opt
out with `record_video: false` when you don't want video.

**macOS — native ScreenCaptureKit, zero-config.** On macOS the
recorder uses an in-process `SCStream` + `SCRecordingOutput`, so it
inherits cua-driver's own Screen Recording grant — no separate
subprocess prompt, no fast-fail, no second TCC dance. Requires macOS
15.0+ (SCRecordingOutput introduced in macOS 15). No ffmpeg needed.

**Windows / Linux — ffmpeg subprocess.** Outside macOS the recorder
shells to ffmpeg with `gdigrab` (Windows) or `x11grab` (Linux). The
binary needs to be on PATH (`winget install Gyan.FFmpeg` /
`apt install ffmpeg`); when missing, the per-turn capture continues
without video and `last_error` carries the install hint. ffmpeg
startup failures fast-fail with a stderr tail in the error.

## Start / stop

Two equivalent surfaces: the `start_recording` / `stop_recording` MCP
tools, or the friendlier `cua-driver recording` subcommand group
(wraps both with human-readable output).

```
cua-driver recording start ~/cua-trajectories/run-1
# … run the workflow …
cua-driver recording status    # -> enabled / disabled, next_turn, output_dir
cua-driver recording stop      # -> "Recording stopped. (video → recording.mp4)"
```

Raw-tool equivalent:

```
cua-driver start_recording '{"output_dir":"~/cua-trajectories/run-1"}'
cua-driver get_recording_state
cua-driver stop_recording '{}'
```

The `recording` subcommands require a running daemon (`cua-driver
serve &`) because recording state is per-process. `output_dir` expands
`~` and is created (with intermediates) if missing. Turn numbering
starts at `1` every time recording is (re-)enabled, regardless of any
existing contents in the directory. State lives in memory only — a
daemon restart resets to disabled.

## What each turn folder contains

Each action writes to `turn-NNNNN/` (five-digit zero-padded counter):

- `app_state.json` — post-action AX/UIA snapshot for the target
  `(pid, window_id)` carrying the same `tree_markdown` +
  `element_count` shape `get_window_state` returns (minus the
  screenshot fields — those live in `screenshot.png`). On macOS the
  recorder resolves a frontmost window internally when the action's
  args don't carry one; on Windows it uses the first window of the
  target pid. **Omitted on Linux** — ATSPI doesn't expose a cheap
  whole-tree snapshot, and the file is left out rather than faked.
- `screenshot.png` — post-action capture of the target window.
  Omitted when the pid has no visible window.
- `action.json` — the tool name, full input arguments, result
  summary, pid, click point (when applicable), ISO-8601 timestamp.
- `click.png` — for click-family actions (`click`, `double_click`,
  `right_click`): a copy of `screenshot.png` with a red dot drawn at
  the click point. **Both addressing modes are covered:** explicit
  `x, y` clicks use the supplied coordinates directly, and
  `element_index`-addressed clicks resolve to the element's center
  via the live AX/UIA cache, then convert to window-local screenshot
  pixels. Absent for non-click tools and for clicks whose resolved
  point falls outside the captured window.

## When to use it

- Demos and screen recordings — play the turn folder back to show
  exactly what the agent saw and what it did.
- Replay for regression — re-run the same sequence against a future
  build and diff the new trajectory against the saved one.
- Training data collection — each turn is a
  `(state, action, next_state)` triple ready for offline learning.

## When to invoke it

This skill does **not** auto-enable recording. The client invokes
`start_recording` explicitly when the user asks to capture a session.
If the user says "record this session" or similar, call
`start_recording({output_dir:…})` before the first action (video on
by default; pass `record_video: false` to opt out), and
`stop_recording({})` when done.

## Replaying a recorded trajectory

`replay_trajectory({dir})` walks `<dir>/turn-NNNNN/` folders in
lexical order, reads each `action.json`, and re-invokes the recorded
tool with its recorded `arguments`. Optional knobs: `delay_ms`
(pacing between turns, default 500) and `stop_on_error` (halt on
first failure, default true).

```
cua-driver recording start ~/cua-trajectories/demo1
# … run the workflow …
cua-driver recording stop
# Later: replay against a new build.
cua-driver replay_trajectory '{"dir":"~/cua-trajectories/demo1","delay_ms":500}'
```

Important caveat: **element_index doesn't survive across sessions**.
Indices are assigned fresh on every `get_window_state` snapshot,
keyed on `(pid, window_id)`, so a recorded
`click({pid, window_id, element_index: 14})` from yesterday won't
resolve today — the pid is usually different, the window_id always
is. The call returns `Invalid element_index` or `No cached AX
state`. Pixel clicks (`click({pid, x, y})`) and keyboard tools
(`press_key`, `hotkey`, `type_text` without element_index) replay cleanly; element-indexed actions require a
live snapshot that replay doesn't currently re-emit (read-only tools
like `get_window_state` aren't recorded). For a reliable replay, either
compose the trajectory from pixel + keyboard primitives, or capture
it as a regression artifact (compare the failure/success pattern
across builds) rather than a re-driving script.

If recording is still enabled while replay runs, the replay is
itself recorded into the current output directory — that's the
intended regression-diff workflow.
