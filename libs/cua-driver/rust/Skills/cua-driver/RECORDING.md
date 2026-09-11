# Recording and replay

Record only when the user requests it. Keep the recording controls and actions
on one persistent MCP connection; see [RUNTIME.md](RUNTIME.md). The recording
tools do not advertise a public `session` parameter. Do not add one.

## Start, observe, stop

These are tool calls on that connection, not separate CLI invocations:

```text
get_recording_state({})
start_recording({"output_dir":"/absolute/run-dir/trajectory","record_video":true})
get_recording_state({})
# Run the authorized workflow and verify every action.
stop_recording({})
get_recording_state({})
```

Choose an unused output directory: turn numbering restarts at 1. Video is
off by default; explicitly set `record_video:true` when requested. Inspect
`video_active` and `last_error`, not just the successful tool status. Per-turn
capture may continue even when video initialization failed.

The current recorder is shared within its runtime. Manual `stop_recording`
stops whichever recording is active, regardless of its starting session.
Do not start over or stop another run's recording. Session-owned teardown
does not establish concurrent recording isolation. If a recorder is already
active, coordinate with its owner rather than taking it over.

Stop and inspect `last_video_path` before ending the connection. A disconnect
can tear down owned recording; a runtime restart loses in-memory state.
Confirm the expected files exist, inspect images, and decode the finalized
video before claiming success. A file path alone does not prove a playable
or correctly scoped recording.

CLI `recording start|status|stop` commands also exist, but do not form a
session-preserving action loop by themselves. Inspect their help rather than
assuming the convenience command supports every raw-tool argument.

## Platform boundaries

- macOS video uses ScreenCaptureKit/SCRecordingOutput (macOS 15+), under the
  responsible runtime's grant. Permission status is not proof of live capture.
- Windows video uses ffmpeg `gdigrab`; the runtime must be in the interactive
  desktop session.
- Linux X11 video uses ffmpeg `x11grab`. Native Wayland needs a supported
  compositor-specific recorder and any portal grant; see [LINUX.md](LINUX.md).
  A successful desktop PNG does not establish video availability.

## What each turn folder contains

Each action writes to `turn-NNNNN/` (five-digit zero-padded counter):

- `before_state.json` and `after_state.json` — application accessibility
  state immediately before and after the action. They carry the same
  `tree_markdown` and `element_count` shape as `get_window_state`.
- `before.png` and `after.png` — target-window images immediately before
  and after the action. Window capture remains scoped to the target when
  another window covers it.
- `evidence.json` — capture status for each phase. Missing expected capture
  has an explicit classification instead of disappearing from the turn.
- `app_state.json` and `screenshot.png` — compatibility aliases for
  `after_state.json` and `after.png`.
- `action.json` — the tool name, full input arguments, result
  summary, result-error flag, pid, click point (when applicable), ISO-8601
  timestamp.
- `click.png` — for click-family actions (`click`, `double_click`,
  `right_click`): a copy of the pre-input image with a red marker drawn at
  the click point. Usually that source is `before.png`. When Windows scrolls
  an element into view during an action, it retains an additional
  `click_source.png` immediately before coordinate input. In that case,
  `action.json` names `click_point_image: "click_source.png"`, and
  `evidence.json` records `click.source_image` and the `click_source` capture
  status. The original before/after images and state remain intact.
  **Both addressing modes are covered:** explicit
  `x, y` clicks use the platform's recording-coordinate mapping, and
  `element_index`-addressed clicks resolve to the element's center
  via the live AX/UIA cache, then convert to the retained image's coordinate
  space. Native Hyprland recording retains the output image, so both kinds of
  marker use output coordinates. Pixel markers also account for the target
  window's origin and any snapshot resize or zoom. Absent for non-click tools.
  It is also absent, and explicitly
  classified as not applicable, when the driver refuses a click before target
  resolution; no input was aimed in that case. A successful plain Linux AT-SPI
  or Windows UIA element click (Invoke, Toggle, SelectionItem, or ExpandCollapse)
  can activate a control without a visible point, such as an offscreen button.
  In that case, `semantic_action_without_point` records why
  there is no marker. The action must carry explicit accessibility transport
  and known delivery metadata; its before/after state, images, and requested
  video remain required. This exception does not apply to pixel clicks,
  Windows SendInput clicks, uncertain delivery, or failed marker rendering.
  Out-of-image points are rejected, never moved to an image edge. Other dispatched clicks whose markers
  cannot be resolved or rendered remain evidence failures.

## Replay is a new action sequence

`replay_trajectory` invokes recorded arguments in turn order; it is not a
semantic task planner. It accepts `dir`, `delay_ms`, and `stop_on_error`.
Read its live schema before use, and obtain authorization for the actions
being replayed.

Recorded PIDs, window IDs, element tokens, browser refs, and geometry can all
be stale. Read-only snapshots are not part of the action sequence, so replay
does not automatically refresh element handles. Pixel/key actions also need
the original target identity, layout, and focus; they are not portable merely
because they lack an element token.

Use trajectories as evidence unless current targets and preconditions are
independently established. Never replay canceled, partial, or unknown actions
to discover whether they originally landed. If recording remains enabled
during replay, the replay itself may produce new turns; keep source and output
directories distinct.
