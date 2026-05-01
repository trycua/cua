# Recording & replaying trajectories

Session-scoped capture of action sequences + pre/post state, suitable
for demos, regression diffs, and training data. Invoked only when the
user explicitly asks to record — the skill does not auto-enable this.

`set_recording` turns on a session-scoped trajectory recorder. While
enabled, every action-tool call (`click`, `right_click`, `scroll`,
`type_text`, `press_key`, `hotkey`, `set_value`)
writes a numbered turn folder under a caller-chosen output
directory. Read-only tools (`get_window_state`, `list_windows`,
`screenshot`, `list_apps`, permission probes, agent-cursor getters /
setters, and `set_recording` itself) are not recorded.

## Enable / disable

Two equivalent surfaces: the `set_recording` MCP tool, or the
friendlier `cua-driver recording` subcommand group (wraps
`set_recording` + `get_recording_state` with human-readable output).

```
cua-driver recording start ~/cua-trajectories/run-1
# … run the workflow …
cua-driver recording status    # -> enabled / disabled, next_turn, output_dir
cua-driver recording stop      # -> "Recording disabled (N turns captured in …)"
```

Raw-tool equivalent:

```
cua-driver set_recording '{"enabled":true,"output_dir":"~/cua-trajectories/run-1"}'
cua-driver get_recording_state
cua-driver set_recording '{"enabled":false}'
```

The `recording` subcommands require a running daemon (`cua-driver
serve &`) because recording state is per-process. `output_dir` expands
`~` and is created (with intermediates) if missing. Turn numbering
starts at `1` every time recording is (re-)enabled, regardless of any
existing contents in the directory. State lives in memory only — a
daemon restart resets to disabled.

## What each turn folder contains

Each action writes to `turn-NNNNN/` (five-digit zero-padded counter):

- `app_state.json` — post-action AX snapshot for the target pid, same
  shape `get_window_state` returns (tree_markdown, element_count,
  turn_id, etc.) minus the screenshot fields. The recorder resolves a
  frontmost window internally (visible + on-current-Space preferred,
  max-area fallback) since individual action tools carry a
  window_id but the recorder has no caller-supplied anchor.
- `screenshot.png` — post-action capture of the same window the
  recorder just snapshotted. Omitted when the pid has no visible
  window.
- `action.json` — the tool name, full input arguments, result
  summary, pid, click point (when applicable), ISO-8601 timestamp.
- `click.png` — only for click-family actions (`click`,
  `right_click`): a copy of `screenshot.png` with a red dot drawn at
  the click point (screen-absolute point → window-local pixels via
  the screenshot's `scale_factor`). Absent for other tools and for
  clicks whose point falls outside the captured window.

## When to use it

- Demos and screen recordings — play the turn folder back to show
  exactly what the agent saw and what it did.
- Replay for regression — re-run the same sequence against a future
  build and diff the new trajectory against the saved one.
- Training data collection — each turn is a
  `(state, action, next_state)` triple ready for offline learning.

## When to invoke it

This skill does **not** auto-enable recording. The client invokes
`set_recording` explicitly when the user asks to capture a session.
If the user says "record this session" or similar, call
`set_recording({enabled:true, output_dir:…})` before the first
action, and `set_recording({enabled:false})` when done.

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
