---
name: cua-driver-rs
description: Drive a native GUI app (macOS, Windows, Linux) via the cua-driver CLI (default) or MCP server — snapshot its accessibility tree, click/type/scroll by element_index or pixel coords, verify via re-snapshot, all without bringing the target to the foreground. Use when the user asks you to operate, drive, automate, or perform a GUI task in a real application on the host.
---

# cua-driver-rs

Orchestrates cross-platform app automation via `cua-driver`. Whenever
a user asks to drive a native app, follow the loop in this skill
rather than calling tools ad-hoc — the snapshot-before-action
invariant is not optional and silently breaks if you skip it.

## Platform-specific reading — read this first

This file is the **cross-platform core**: snapshot invariant, CLI vs
MCP choice, tool surface naming, behavior matrix, canonical loop,
pixel-click contract, common failure modes. The platform-specific
material (forbidden-list, accessibility tree implementation, launch
semantics, click dispatch) lives in companion files in this same
directory:

- **macOS** — read `MACOS.md` (no-foreground contract, forbidden
  `open`/`osascript`/`cliclick` invocations, AXMenuBar navigation,
  SkyLight pixel-click dispatch, Apple-Events JS bridge).
- **Windows** — read `WINDOWS.md` (UIA tree vs AX, UWP /
  ApplicationFrameHost hosting, layered UIA+PostMessage click chain,
  Session 0 isolation, Windows-specific focus-steal vectors).
- **Linux** — read `LINUX.md` (X11/Wayland status, AT-SPI,
  BETA-level support).

Cross-cutting topics also have their own files:

- `WEB_APPS.md` — browser / Electron / Tauri specifics (sparse AX
  trees, omnibox navigation, the `set_value` workaround for
  minimized windows, tabs-vs-windows guidance).
- `RECORDING.md` — session recording + `replay_trajectory`.
- `TESTS.md` — internal test surface.

Use whichever combination matches the host. When in doubt, run
`cua-driver doctor` — it reports the platform and the right entry
point.

## The no-foreground principle (cross-platform)

**The user's frontmost app MUST NOT change.** Every platform has its
own list of forbidden commands; the principle is universal:

- macOS: any `open` invocation, any `osascript` that mutates GUI
  state, `cliclick`, `cghidEventTap` writes targeting another app's
  window. Full list in `MACOS.md`.
- Windows: any `Start-Process` that triggers a `ShowWindow`/`SetForegroundWindow`
  on the target, `WScript.Shell.AppActivate`, attaching to the
  foreground thread for input forwarding. Full list in `WINDOWS.md`.

If you reach for a command that says "activate", "foreground",
"raise", or "make key", stop and translate to the cua-driver tool
that does the same intent without focus-stealing.

## Defaults — always prefer cua-driver over shell shims

**Default transport is the `cua-driver` CLI** — `Bash` shelling out
to `cua-driver <tool-name> '<JSON-args>'`. MCP tools (prefix
`mcp__cua-driver__*`) only when the user explicitly asks for them.
CLI wins because it picks up rebuilds instantly, failures are
easier to diagnose, and there's no per-tool schema-load overhead.

Every reference to `click(...)`, `get_window_state(...)` etc. in this
skill means `cua-driver click '{...}'` — translate to MCP form only
when MCP is requested.

### Claude Code computer-use compatibility mode

For normal Claude Code use, keep the default CLI or `cua-driver` MCP
server path above. If the user explicitly wants Claude Code's
vision/computer-use-style flow, they can register:

```bash
cua-driver mcp-config --client claude   # then paste + run the printed line
```

Observation: Claude Code vision flows appear to treat a screenshot
MCP tool as the image-grounding anchor. This compatibility mode keeps
the normal CuaDriver tools and changes only `screenshot`. The
compatibility `screenshot` requires `pid` and `window_id`, captures
only that target window, and returns the window-local pixel
coordinate frame. Start with `launch_app` or `list_windows`, then
call `screenshot({pid, window_id})`; do not assume desktop
coordinates or a full-screen capture.

Use MCP for this Claude Code vision/computer-use-style path. Do not
shell out to `cua-driver screenshot` as a substitute: CLI screenshots
still work as CuaDriver calls, but they do not expose the
`mcp__cua-computer-use__screenshot` tool name that Claude Code
appears to use as the image-grounding cue.

## Using cua-driver from the shell

Tool names are `snake_case`, management subcommands are
`kebab-case` — no ambiguity. Tools invoked as `cua-driver
<tool-name> '<JSON-args>'`. Management subcommands:

- `cua-driver serve` — start persistent daemon (**required** for
  `element_index` workflows; without it each CLI invocation spawns a
  fresh process and the per-pid element cache dies between calls).
  macOS users: see `MACOS.md` for the LaunchServices-routed launch
  form.
- `cua-driver stop` / `status`
- `cua-driver list-tools`, `describe <tool>`
- `cua-driver recording start|stop|status` — see `RECORDING.md`

Canonical multi-step workflow (example shape — platform-specific
launch idioms in the per-OS companion file):

```bash
cua-driver serve
cua-driver launch_app '{"bundle_id":"..."}'
# → {pid: 844, windows: [{window_id: 10725, ...}]}
cua-driver get_window_state '{"pid":844,"window_id":10725}'
cua-driver click '{"pid":844,"window_id":10725,"element_index":14}'
cua-driver stop
```

## Agent cursor overlay

Visual cursor overlay for demos and screen recordings. Default:
enabled. Toggle with `cua-driver set_agent_cursor_enabled
'{"enabled":true|false}'`. A triangle pointer Bezier-glides to each
click target, ring-ripples on landing, idle-hides after ~1.5s.
Motion knobs: `set_agent_cursor_motion` takes any subset of
`start_handle`, `end_handle`, `arc_size`, `arc_flow`, `spring` —
tuneable at runtime, persisted to config.

Requires the daemon process's UI runloop, which `cua-driver serve` /
`mcp` bootstraps. One-shot CLI invocations skip the overlay entirely.

## The core invariant — snapshot before AND after every action

**Every action MUST be bracketed by `get_window_state(pid, window_id)`**:

- **Before** — the pre-action snapshot resolves the `element_index`
  you're about to use. Indices from previous turns are stale; the
  server replaces the element index map on every snapshot, keyed
  on `(pid, window_id)`. Indices from turn N don't resolve in turn
  N+1, and indices from window A don't resolve against window B of
  the same app. Skip this and element-indexed actions fail with
  `No cached AX state`.
- **After** — the post-action snapshot verifies the action actually
  landed. Without it you can't tell a silent no-op from a real
  effect. The accessibility-tree change (new value, new window,
  disappeared menu, disabled button, etc.) is your evidence that
  the action fired. If nothing changed, the action probably failed
  silently — say so, don't assume success.

This applies to pixel clicks too — re-snapshot after to confirm the
click landed on the intended target.

### Why window selection is the caller's job now

`get_app_state` used to pick a window for you via a max-area heuristic
that returned the wrong surface on apps with large off-screen utility
panels. Concrete reproducer: IINA's OpenSubtitles helper (600×432
off-screen) out-area'd the visible 320×240 player window, so
`get_app_state(pid)` screenshot'd the invisible panel and clicks landed
there silently. The new `get_window_state(pid, window_id)` makes the
caller name the window explicitly — the driver validates that the
window belongs to the pid and is on the current Space/desktop, then
snapshots exactly what was asked for. Enumerate candidates via
`list_windows` or read the `windows` array `launch_app` already
returns.

## Behavior matrix

Two orthogonal axes shape what the agent can do.

**capture_mode → addressing mode**

| `capture_mode` | `get_window_state` returns | Use for actions |
|---|---|---|
| **`som`** (default) | tree + screenshot | `element_index` preferred; pixel fallback |
| **`ax`** | tree only (no PNG) | `element_index` only |
| **`vision`** | PNG only (no tree) | pixel only — see `SCREENSHOT.md` |

`vision` was renamed from `screenshot` — the old name still decodes
as a deprecated alias, so an on-disk `"capture_mode": "screenshot"`
keeps working. Default is `som` so element_index clicks work the
first time a user calls `get_window_state`; the other modes are
opt-in when the caller specifically doesn't want one half of the
work. Note the tool named `screenshot` is separate (raw PNG, no AX
walk) and unrelated to the capture mode.

When a snapshot looks wrong (tiny screenshot / empty tree), check
`cua-driver get_config` for `capture_mode` before anything else.

Pure-vision mode has its own caveats — Claude Code's vision pipeline
downsamples dense text aggressively, so pixel grounding takes
multiple correction cycles on text-heavy UIs.

**Window state → what works**

| state | `get_window_state` | element-index click (AX/UIA) | `press_key` commit | pixel click |
|---|---|---|---|---|
| frontmost | ✅ | ✅ | ✅ | ✅ |
| backgrounded / visible | ✅ | ✅ | ✅ | ✅ |
| **minimized** | ✅ | ✅ (actions fire in place) | ❌ silent no-op — use `set_value` or click equivalent | ❌ no on-screen bounds |
| hidden | ✅ | ✅ | depends | ❌ |
| on another desktop / Space | ⚠️ tree may be stripped on some apps — response carries `off_space: true` so you can detect it | ✅ | ✅ | ❌ not in current-desktop list |

**Critical cell — minimized + keyboard commit.** The keystroke
reaches the app but accessibility focus doesn't propagate to renderer
focus on a minimized window. Workarounds in order of preference:
`set_value` to write the field's entire value directly, or
element-index-click a commit-equivalent button (Go, Submit,
checkbox). Tell the user the window needs to un-minimize only as a
last resort.

## The canonical loop

```
launch_app(target)
  → pick window_id from the returned `windows` array
    (or call list_windows(pid) separately)
  → get_window_state(pid, window_id)
    → [act]  # every action also takes (pid, window_id)
  → get_window_state(pid, window_id) → verify
```

`launch_app` now returns a `windows` array alongside the pid, so the
common case collapses to two calls (`launch_app` → `get_window_state`)
without a separate `list_windows` hop.

`list_apps` is for app-level discovery (answering "what's installed /
running / frontmost?") — not part of the core action loop. Skip it
in the loop. For **window-level** questions — "does this app have a
visible window?", "which desktop is this window on?", "which of this
pid's windows is the main one?" — call `list_windows` instead; the
app record doesn't carry window state on purpose. In the common
single-window case you can skip `list_windows` entirely and read the
`windows` array that `launch_app` already returned.

### Snapshot and act by element_index

Call `get_window_state({pid, window_id})` with the `window_id` from
`launch_app`'s `windows` array (or a fresh `list_windows({pid})` if
you're interacting with a long-lived process). The default `som`
capture_mode returns **both the tree and screenshot**, so the
canonical loop works immediately without any config change.

In `som` mode (the default) the response carries:

- `tree_markdown` — every actionable element tagged `[N]`. That `N`
  is the `element_index`. The tree can be very large (Finder is
  ~1600 elements, ~190 KB); when it exceeds token limits the MCP
  harness saves it to a file and returns the path. Use `Bash` +
  `jq -r '.tree_markdown'` + `grep` to pull the section you need.
- `screenshot_file_path` — absolute path to the saved screenshot when
  `screenshot_out_file` was passed. Absent otherwise.
- `screenshot_width` / `_height` / `_scale_factor` — dimensions of
  the captured image. Present whenever a screenshot was taken.

**Getting the screenshot as a file (CLI and context-constrained agents):**

```bash
# write to file — stdout stays readable (AX/UIA tree / summary only, no base64)
cua-driver get_window_state '{"pid":N,"window_id":W,"screenshot_out_file":"/tmp/shot.jpg"}'

# CLI --screenshot-out-file flag is equivalent and works for all capture modes
cua-driver get_window_state '{"pid":N,"window_id":W}' --screenshot-out-file /tmp/shot.jpg
```

Pass `screenshot_out_file` when using `get_window_state` via CLI or
from an agent whose context window can't absorb ~31 KB of inline
base64 (e.g. OpenCode with a local Ollama model). The MCP image
content block is omitted from the response when this param is set —
the model receives only the tree and `screenshot_file_path`, then
reads the image from disk.

**Reason over both the tree AND the screenshot — they're
complementary, not redundant.** In `som` mode every turn's
`get_window_state` gives you both halves and you should pull signal
from each:

- The **tree** tells you *what's clickable* — roles, labels,
  `element_index` handles, advertised actions, parent-child
  structure. This is the ground truth for dispatching.
- The **screenshot** tells you *which one* — the tree often has
  many buttons with similar or empty labels ("Delete", "OK",
  anonymous UUID-labeled buttons, repeated static-text), and visual
  context disambiguates. Captions, colors, layout relationships
  visible in pixels often don't show up in the tree at all
  (especially in Chromium / Electron / web content).

Canonical pattern: look at the screenshot to decide "the blue
Subscribe button on the top-right of the video card", then walk the
tree to find the matching element and dispatch by its
`element_index`. Don't try to do it from just the tree — you'll
pick the wrong element when labels repeat. Don't try to do it from
just the screenshot — you lose the reliable accessibility-action
path and the safe backgrounded-dispatch.

Reach for pixel coordinates only when the target is a canvas /
video / WebGL / custom-drawn surface that isn't in the tree
(see "Pixel-coordinate clicks" below).

The `actions=[...]` list on each element is **advisory**, not
authoritative. cua-driver does not pre-flight check against it —
`click({pid, element_index})` always attempts the default action (or
the action you pass) and surfaces whatever the target returns. **Try
the click first** — pivot only on the returned error code.

### Tool dispatch table

Every row assumes a `(pid, window_id)` pair from the last
`get_window_state`; `window_id` is required alongside `element_index`,
ignored on pixel-only forms unless you want to anchor the conversion
against a specific window.

| Intent | Tool | Notes |
|---|---|---|
| List an app's windows | `list_windows({pid})` | returns `window_id`, `title`, `bounds`, `z_index`, `is_on_screen`, `on_current_space`. Already included in `launch_app`'s response — only call this for long-lived pids |
| Snapshot a window | `get_window_state({pid, window_id})` | returns `tree_markdown` + `screenshot_*`; populates the `(pid, window_id)` element_index cache |
| Left click | `click({pid, window_id, element_index})` | default `action: "press"`. Pixel form: `click({pid, x, y})` (window_id optional) — `modifier: ["cmd"\|"ctrl"]` |
| Double-click / open | `double_click({pid, window_id, element_index})` | Default action when the element advertises one (Open on Finder items / openable rows), else stamped pixel double-click at the element's center |
| Right click / context menu | `right_click({pid, window_id, element_index})` or `click({pid, window_id, element_index, action: "show_menu"})` | Chromium web-content coerces pixel right-click to left on macOS — see `WEB_APPS.md` |
| Type at cursor | `type_text({pid, text, window_id, element_index})` | focuses element first, then writes via the platform's text-set primitive |
| Set whole field value | `set_value({pid, window_id, element_index, value})` | sliders, steppers, text fields; **use for keyboard-commit workarounds on minimized windows** |
| Scroll | `scroll({pid, direction, amount, by, window_id, element_index})` | synthesizes per-pid PageUp/PageDown/arrows |
| Focus + send key | `press_key({pid, key, window_id, element_index, modifiers})` | element_index sets focus, then posts key |
| Send key to pid | `press_key({pid, key, modifiers})` | no focus change; key goes to pid's current focus |
| Modifier combo | `hotkey({pid, keys})` | e.g. `["cmd","c"]` / `["ctrl","c"]`; posted per-pid, not HID tap |

**All keyboard/text primitives require `pid`.** There is no
frontmost-routed variant — every key goes to the named target via
the platform's per-pid event-post path, so the driver cannot leak
keystrokes into the user's foreground app.

**Why `element_index` is the primary path:** works on hidden /
occluded / off-desktop windows, no focus steal, stable across
rebuilds, labels tell you what you're clicking. Reach for pixel
coordinates only when the accessibility tree can't.

## Pixel-coordinate clicks

The pixel path (`click({pid, x, y})`) is for surfaces the
accessibility tree doesn't reach — canvases, video players, WebGL,
custom-drawn controls. Coords are **window-local screenshot pixels**
(same space as the PNG `get_window_state` returns). Top-left origin,
y-down. The driver handles screen-point conversion internally.
Passing `window_id` alongside `x, y` is optional but recommended —
it pins the coordinate conversion to the window whose screenshot
produced the pixel.

PNGs returned by `get_window_state` are capped at **1568 px long-side
by default** (`max_image_dimension` config), matching Anthropic's
multimodal-vision downsampling limit. The image the model reasons
over and the image the click tool's coordinate system lives in are
the **same resolution** — just look at the PNG, pick a pixel, click
at that pixel. No scaling math.

This is the default because the mismatch between "rendered
thumbnail" and "native PNG" was a recurring coord-estimation
footgun. If you opt out (explicit `max_image_dimension=0` for
pixel-perfect verification flows), the old rule applies: don't
eyeball coords from whatever your client renders — it may be
2-4× smaller than the PNG on disk, and a 2% error in thumbnail
space becomes ~80 px in the real image.

For precise targeting on small / dense UIs:

1. `get_window_state({pid, window_id})` → image capped at 1568
   long-side plus `screenshot_width` / `screenshot_height`. Write to
   disk via `--screenshot-out-file <path>`.
2. Look at the PNG. Since it matches what you see, pick the target
   pixel directly.
3. When precision matters, draw a crosshair on the image (do
   **not** crop — cropping loses the coordinate system) and verify
   before clicking:

```python
from PIL import Image, ImageDraw
img = Image.open('/tmp/shot.png')
draw = ImageDraw.Draw(img)
x, y = <your_coordinate>
r = 18
draw.ellipse([x-r, y-r, x+r, y+r], outline='red', width=4)
draw.line([x-30, y, x+30, y], fill='red', width=3)
draw.line([x, y-30, x, y+30], fill='red', width=3)
img.save('/tmp/shot_annotated.png')
```

4. Only dispatch the click after the user (or your own re-read of
   the annotated image) confirms the crosshair is on target.

Addressing variants:

- `click({pid, x, y})` — single left-click.
- `click({pid, x, y, count: 2})` — double-click.
- `click({pid, x, y, modifier: ["cmd"\|"ctrl"]})` — modifier click.
  Accepts any subset of `cmd/shift/option/alt/ctrl`.
- `right_click({pid, x, y})` — also takes `modifier`.

The pixel path animates the agent cursor overlay but never warps
the real cursor (the per-pid event paths the driver uses on macOS
and Windows route around HID synthesis). If the pid has no on-screen
window the call errors with `pid X has no on-screen window` — you
need a visible window to anchor the conversion. Dispatch details
(SkyLight on macOS, layered UIA+PostMessage on Windows) are in the
per-OS companion files.

## Web-rendered apps (browsers, Electron, Tauri)

For Chrome / Edge / Brave / Arc / Safari, Electron apps (Slack,
VSCode, Notion, Discord), and Tauri apps — see **`WEB_APPS.md`**.

Covers: sparse accessibility tree population (retry-once pattern for
Chromium), URL navigation via omnibox suggestions, the `set_value`
workaround for keyboard commits on **minimized** windows (Return
silently no-ops — symptom is a system bell; use `set_value` or click
a clickable equivalent), scrolling via synthetic PageUp/Down
keystrokes, in-page clicks, and typing into web inputs.

Browser JS primitives are now **cross-platform** via the `page` tool —
macOS uses Apple Events for Chrome/Brave/Edge/Safari + CDP for Electron
(see `MACOS.md`); Windows + Linux use UIA / AT-SPI for `get_text` /
`query_dom` and the shared CDP client for `execute_javascript` (browser
must be launched with `--remote-debugging-port=N` and the port exported
as `CUA_DRIVER_CDP_PORT`).

## Re-snapshot and verify — mandatory

**Always** call `get_window_state({pid, window_id})` after the
action. This isn't optional verification — it's the second half of
the snapshot invariant.

Check the tree diff: a changed value, a new element, a new window,
or the disappearance of the thing you just clicked (menus collapse
after selection, buttons may become disabled, etc.). If nothing
changed, the action likely failed silently — **tell the user what
you attempted and what you observed**, don't paper over with "done"
language. Agents that skip this step report success on
silently-dropped actions — the single most common failure mode.

## Recording trajectories

Session-scoped action recording + replay, for demos, regressions,
and training data. Only invoke when the user explicitly asks to
record a session — the skill does not auto-enable this. CLI surface:
`cua-driver recording start|stop|status`; raw tool: `set_recording`.

See **`RECORDING.md`** for the full flow: enable/disable, turn folder
contents, replay via `replay_trajectory`, and the element_index
doesn't-survive-across-sessions caveat.

## Common error patterns (cross-platform)

| Error text | Meaning | Fix |
|---|---|---|
| `No cached AX state for pid X window_id W` | You either skipped `get_window_state` this turn, or passed a different `window_id` to the click than the one the snapshot cached against | Call `get_window_state({pid: X, window_id: W})` first — the same window_id you intend to click in |
| `Invalid element_index N for pid X window_id W` | Index is stale or out of range | Re-run `get_window_state` with the same window_id, pick a fresh index from the new tree |
| `window_id W belongs to pid P, not …` | Passed a window_id that's owned by a different process | Use `list_windows({pid: X})` to enumerate this pid's own windows |
| `AX action … failed with code …` / `UIA invoke failed` | Element doesn't support the default action | Try `show_menu`, `confirm`, `cancel`, `pick`, or fall through to a pixel click on the element's center |

Platform-specific errors (TCC dialogs on macOS, Session 0 / UAC
prompts on Windows, AT-SPI bus issues on Linux) live in their
respective companion files.

## Things to avoid

- **Never** reuse an `element_index` across a re-snapshot of the same pid.
- **Never** translate screenshot pixels into a click — the screenshot
  is for visual disambiguation, not coordinates. Use the
  `element_index`.
- **Prefer accessibility actions over pixels.** `click({pid, x, y})`
  works for canvas / WebView regions, but it lands blindly and skips
  the agent-cursor overlay. Exhaust accessibility paths (menu bars,
  cmd-k palettes, toolbar items, keyboard shortcuts) before dropping
  to coordinates.
- **Never** drive destructive actions (delete files, close unsaved
  documents, send messages, submit forms) without explicit user
  intent for that specific destructive step.
- **Never** launch apps autonomously; confirm with the user first
  unless their original request clearly implies the launch.

## Example end-to-end task

**User:** "Open the Downloads folder in the system file manager."

1. `launch_app({bundle_id: "com.apple.finder", urls: ["~/Downloads"]})`
   on macOS, or `launch_app({name: "explorer", args: ["%USERPROFILE%\\Downloads"]})`
   on Windows. Returns `{pid, windows: [{window_id, title, ...}]}`.
   Idempotent launch; the driver opens a hidden window via the
   platform's launch primitive — zero activation, no focus steal.
2. `get_window_state({pid, window_id})` → verify the expected window
   title is present with a populated tree (sidebar, list view, files).
3. Done.

Platform-specific examples and edge cases (Finder menu navigation,
Explorer ribbon, GNOME Files) live in the per-OS companion files.
