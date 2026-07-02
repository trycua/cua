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
- **Linux** — read `LINUX.md` (X11 background input via AT-SPI +
  XSendEvent, recording, Wayland opt-in/preview).

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
- `cua-driver check-update [--json] [--no-cache]` — read-only "is a newer release available?" probe. Same payload as the `check_for_update` MCP tool; pair with `cua-driver update --apply` to install.

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
enabled — you do NOT need to enable it. Toggle with
`cua-driver set_agent_cursor_enabled '{"enabled":true|false}'` only to
hide or re-show it. A triangle pointer Bezier-glides to each click
target, ring-ripples on landing, idle-hides after ~1.5s. Motion knobs:
`set_agent_cursor_motion` takes any subset of `start_handle`,
`end_handle`, `arc_size`, `arc_flow`, `spring` — tuneable at runtime,
persisted to config.

**Per-session cursors.** Each MCP session automatically owns its own
cursor, keyed by the session's id (the proxy mints one session id per
MCP connection and the daemon scopes the cursor, config overrides, and
recording to it). You normally pass nothing — the session key is wired
through for you. Pass an explicit `cursor_id` only to *deliberately
share* one cursor across sessions. When a session ends (the MCP client
disconnects) its cursor is removed automatically.

**Visibility caveat (AX runs).** On a pure accessibility-action run
(clicking by `element_index`), the first action **seeds the cursor
on-screen a short distance from the target and plays a brief glide +
pulse** — not the long Bezier sweep a cursor already on-screen would
trace from its previous spot. It's subtle and easy to miss in a
recording. If you want a clearly *gliding* cursor for a demo or screen
recording, do a pixel click (`click({pid,x,y})`) or a `move_agent_cursor`
first to put the cursor on-screen; subsequent AX actions then glide the
full path normally.

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

### Perception is mode-agnostic — `get_window_state` returns BOTH

`get_window_state(pid, window_id)` **returns both the accessibility
tree AND a screenshot by default.** There is no capture mode to pick
and nothing to configure — you ground on the tree and the screenshot
together, and you cross-check one against the other. This matters
because the tree **lies** on some surfaces:

- **Electron** echo-confirms a `set_value` / `type_text` against the AX
  shim while the rendered text view never changed.
- **Catalyst** (iOSAppOnMac) exposes null / placeholder `AXValue`s.
- **Virtualized / off-viewport list rows** report bogus frames (an
  `h:1` height, an off-screen origin) for rows that aren't actually
  laid out.

A grounding screenshot is present by default, so when the tree looks
wrong you look at the pixels **in the same response** — no second
capture, no mode flip.

> **Perf opt-out — `include_screenshot`.** `include_screenshot`
> (boolean, default `true`) is the one knob, and it is a **perf** knob,
> not a modality choice. Default returns both (grounding-first). Pass
> `include_screenshot:false` to skip the screen grab and get the tree
> only — the cheap path when you're just **re-indexing before an
> element ax action** and don't need to re-ground on pixels. The
> `ax`/`px` decision still lives at action time, not here.

> **`capture_mode` is DEPRECATED and ignored.** It is still *accepted*
> on `get_window_state` so old callers don't error, but it has **no
> effect** — both the tree and the screenshot come back regardless of
> what you pass (`ax`, `vision`, `som`, anything). There is no
> `ax`/`vision`/`som` capture choice anymore. Drop the word "vision"
> for perception entirely. (The tool named `screenshot` is separate —
> raw PNG, no AX walk — and unrelated.)

### The modality is chosen at ACTION time — `ax` vs `px`

You don't pick a capture mode; you pick **how you address the target**
on the action call, and that one choice selects the rung:

- **element ax action** — pass `element_index` / `element_token`.
  Dispatches through the **accessibility rung**: AXPress (macOS) / UIA
  Invoke (Windows) / AT-SPI `doAction` (Linux). Backgroundable,
  z-order-independent, and the only **driver-verifiable** rung.
- **element px action** — pass `x`, `y`. Dispatches through the **pixel
  rung**, reading the coordinate straight off the screenshot that's
  already in the `get_window_state` response. Best-effort; the caller
  confirms the effect.

`ax`↔`element_index`, `px`↔pixel `x,y`. We retired the word "vision"
for the *dispatch* path — it conflated perception with dispatch.
Perception is always both; dispatch is `ax` or `px`.

**The keyboard family has both forms too.** `type_text`, `press_key`,
and `hotkey` take `element_index` (ax) **or** `x,y` (px) — mutually
exclusive, same as the pointer tools. The px form **pixel-clicks at
`(x,y)` to establish real renderer focus, then delivers the
keystroke(s)** to the now-focused element (it reuses `click`'s
coordinate translation + `delivery_mode`). That gives e.g.
`type_text({pid, window_id, x, y, text})` as a one-call focus-then-type
for Chromium/Electron inputs the AX path can't reach, and
`hotkey({pid, x, y, keys:["cmd","v"]})` to paste into a specific field.

**Typing default (the ladder).** Call `type_text` directly with
`element_index` (ax) — it targets the field, no pre-click. On
Electron/Catalyst the AX layer echoes the write without rendering it,
so the driver returns `effect:"unverifiable"` + `escalation:"px"`
there (never a false `verified:true`) — follow it, and cross-check the
screenshot in the response (the only ground truth). Escalate to the px
form — `type_text({pid, window_id, x, y, text})` — which pixel-clicks
to focus, then types. **If the target control is closed** (a search
button, a collapsed field), AX-press to open it first (AX actions work
in the background): a px focus-click won't reliably open *and* focus a
closed control, so the text leaks into whatever's already focused.
Escalate to `delivery_mode:"foreground"` only if it still drops.

**`set_value` stays AX-only by design** — it's for **non-text**
controls (dropdown / `AXPopUpButton`, checkbox, slider, stepper). Its
pixel counterpart is a `click`/`drag` on the control, not a "set value
at a pixel." So: text → `type_text` (ax+px); non-text control values →
`set_value`; pixel-manipulate a control → `click`/`drag`.

**Action responses carry an effect/escalation verdict**

Every action response keeps `verified` (did the driver read back a
post-condition?) and adds two machine-readable fields so you know
whether — and where — to climb the ladder:

- `effect`: one of
  - `"confirmed"` — the driver read back the effect (`ax` rung only).
  - `"unverifiable"` — dispatched, but the driver has no handle to
    read back (every `px`/CGEvent path; foreground rung). **You**
    confirm it off the screenshot — it is not a failure.
  - `"suspected_noop"` — the `ax` action **likely did nothing** (the
    element didn't actually advertise the action, or you hit a passive
    label). This is the explicit **"cross to `px`"** trigger.
- `escalation`: `{recommended, reason}` when the driver thinks you
  should change rung —
  - `"px"` — the element isn't really actionable in `ax`; do an
    **element px action** off the screenshot you already have.
  - `"foreground"` — a background insert/click was *dropped* on
    delivery; re-call the same action with `delivery_mode:"foreground"`.

`get_window_state` itself, when the AX tree comes back empty (a non-AX
surface like Electron/Chromium/canvas), returns `degraded: true`
**plus the same `escalation` hint** — normally pointing at `px` (you
still have the screenshot from the same call to click off).

**Platform nuance for `escalation`.** On **Wayland** an unfocused
window cannot be pixel-targeted in the background (libei →
`background_unavailable`), so there the recommendation is
**`foreground`, not `px`**. macOS, X11, and most Windows surfaces
*can* pixel-target in the background, so they recommend `px`. See
`LINUX.md` / `WINDOWS.md`.

## The verify-then-escalate ladder (algorithm)

Every snapshot already hands you both the tree and the screenshot, so
verifying never means "go take a screenshot" — it means cross-check
the tree against the pixels you already have, and only change
*dispatch rung* on a real signal. Walk the rungs:

```
# Rung 1 — element ax action, backgrounded (the cheap default)
get_window_state(pid, window_id)            # tree + screenshot, both, always
resp = click(pid, window_id, element_index) # or type_text / set_value / press_key
get_window_state(pid, window_id)            # re-snapshot — did the tree change?

if resp.effect == "confirmed" and tree changed:
    done                                    # driver-verified

# escalate only on a real signal
if resp.effect == "suspected_noop"
   or resp.escalation.recommended == "px"
   or get_window_state.degraded            # empty tree → non-AX surface
   or the tree looks wrong vs the screenshot:   # e.g. an h:1 / off-viewport row

    # Rung 2 — element px action off the SAME screenshot
    pick the target pixel from the screenshot already in the response
    click(pid, x, y)                        # background pixel — still no foreground
    get_window_state(pid, window_id)        # re-snapshot, eyeball the result
    if it landed: done

# Rung 3 — background delivery was dropped (insert/click never arrived)
if resp.escalation.recommended == "foreground"
   or the px action still did nothing:
    re-call the same action with delivery_mode:"foreground"
    # on Wayland this is the ONLY escalation — px-bg can't target an
    # unfocused window there; see LINUX.md
    verify again
```

The two ideas to hold onto: (1) the AX tree **lies** on canvas / web /
Catalyst / virtualized surfaces, so an unchanged-or-bogus tree plus
`suspected_noop`/`degraded` — or a tree that simply disagrees with the
screenshot — is your cue to do an **element px action** off the
screenshot you already have; (2) `px` is a *conscious* switch to the
pixel addressing path, not a different capture.

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
start_session(session)            # once per run: declares this run's identity
launch_app(target)
  → pick window_id from the returned `windows` array
    (or call list_windows(pid) separately)
  → get_window_state(pid, window_id)
    → [act]  # every action also takes (pid, window_id) + your `session`
  → get_window_state(pid, window_id) → verify
end_session(session)              # when the run finishes
```

`launch_app` now returns a `windows` array alongside the pid, so the
common case collapses to two calls (`launch_app` → `get_window_state`)
without a separate `list_windows` hop.

**Declare a session.** A session is *your run's* identity — a stable id
you choose (`"research-1"`), declared with `start_session` and passed as
`session` on every action. It owns your agent cursor (a distinct colour
per id), follows the run across any apps/windows, and is the same whether
you drive over MCP, the CLI, or the socket. The cursor is **opt-in**: it
appears only once you declare a session (anonymous actions run cursor-less).
End with `end_session` (or the idle-TTL reclaims it).

**Concurrent runs/subagents:** `launch_app` is idempotent — two runs that
launch the same app get the **same** instance (and on single-instance apps
like Calculator, the same window), so they clobber each other. Give each run
its **own `session`** (→ its own cursor) AND pass
`creates_new_application_instance: true` to `launch_app` (→ its own window).
The element cache is keyed on `(pid, window_id)` and the cursor on `session`,
so distinct instances + distinct sessions keep the runs fully separated.

**Parallelism vs. ordering.** Distinct sessions give distinct *cursors*, not
distinct *connections*. Subagents that share one `cua-driver mcp` (stdio)
connection have their tool calls **serialized** by the transport — they take
turns, not run in parallel. That's not a correctness problem (session + window
isolation means they can't collide), just a throughput one. For genuinely
parallel agents, give each its **own connection**: separate `cua-driver mcp`
processes, or point each agent's MCP client at the daemon's HTTP endpoint
(`CUA_DRIVER_RS_MCP_HTTP_PORT` → `POST http://127.0.0.1:<port>/mcp`). The daemon
serves connections concurrently; per-connection ordering keeps each agent's own
sequence (e.g. `3 → + → 1 → =`) correct.

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
you're interacting with a long-lived process). It returns **the tree
and the screenshot together** by default, so you can both dispatch by
`element_index` and ground on pixels from one call — no config change,
no mode flip. When you're just re-indexing before an element ax action
and don't need fresh pixels, pass `include_screenshot:false` to skip
the grab (a perf knob, not a modality choice).

The response carries:

- `tree_markdown` — every actionable element tagged `[N]`. That `N`
  is the `element_index`. The tree can be very large (Finder is
  ~1600 elements, ~190 KB); when it exceeds token limits the MCP
  harness saves it to a file and returns the path. Use `Bash` +
  `jq -r '.tree_markdown'` + `grep` to pull the section you need.
- `effect` / `escalation` / `degraded` — the verify-then-escalate
  signals (see the behavior matrix above): `degraded: true` means the
  tree came back empty (non-AX surface), so you act by **`px`** off the
  screenshot in the same response.
- `screenshot_file_path` — present when the screenshot was written to
  disk instead of inlined (you passed `screenshot_out_file`, or the
  context-saving CLI path); otherwise the frame is inlined.
- `screenshot_width` / `_height` / `_scale_factor` — dimensions of
  the captured image. Present whenever a screenshot was taken (i.e.
  unless you passed `include_screenshot:false`).

**Getting the screenshot as a file (CLI and context-constrained agents):**

```bash
# write to file — stdout stays readable (AX/UIA tree / summary only, no base64)
cua-driver get_window_state '{"pid":N,"window_id":W,"screenshot_out_file":"/tmp/shot.jpg"}'

# CLI --screenshot-out-file flag is equivalent
cua-driver get_window_state '{"pid":N,"window_id":W}' --screenshot-out-file /tmp/shot.jpg
```

Pass `screenshot_out_file` when using `get_window_state` via CLI or
from an agent whose context window can't absorb ~31 KB of inline
base64 (e.g. OpenCode with a local Ollama model). The MCP image
content block is omitted from the response when this param is set —
the model receives only the tree and `screenshot_file_path`, then
reads the image from disk.

**The tree and the screenshot are complementary, not redundant — and
they come from the *same* call.** Each half carries signal the other
can't, which is exactly why you cross-check them:

- The **tree** tells you *what's clickable* — roles, labels,
  `element_index` handles, advertised actions, parent-child
  structure. This is the ground truth for an **element ax action**.
- The **screenshot** tells you *which one* — the tree often has many
  buttons with similar or empty labels ("Delete", "OK", anonymous
  UUID-labeled buttons, repeated static-text), and visual context
  disambiguates. Captions, colors, layout relationships visible in
  pixels often don't show up in the tree at all (especially in
  Chromium / Electron / web content) — and the screenshot is where you
  catch the tree *lying* (an `h:1`/off-viewport row, a Catalyst null
  value).

Default to dispatching by `element_index` (the **element ax action**) —
it's the verifiable, backgroundable rung. Do an **element px action**
(`x,y` off the same screenshot) when the tree can't disambiguate
(repeated/empty labels), when it's empty (`degraded` — non-AX
surface), when an action came back `suspected_noop`, or when the tree
disagrees with the pixels. You never re-capture to switch — the
screenshot is already there; you just change *how you address* the
target.

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
| Type at cursor | `type_text({pid, text, window_id, element_index})` (ax) or `type_text({pid, text, window_id, x, y})` (px) | ax focuses the element then writes via the platform's text-set primitive; **px** pixel-clicks `(x,y)` to focus the renderer, then types — the one-call fix for Chromium/Electron inputs the AX path can't reach |
| Set whole non-text control value | `set_value({pid, window_id, element_index, value})` | **AX-only by design** — dropdown/`AXPopUpButton`, checkbox, slider, stepper; **also the keyboard-commit workaround on minimized windows.** For text use `type_text`; to pixel-manipulate a control use `click`/`drag` |
| Scroll | `scroll({pid, direction, amount, by, window_id, element_index})` | synthesizes per-pid PageUp/PageDown/arrows |
| Focus + send key | `press_key({pid, key, window_id, element_index, modifiers})` (ax) or `press_key({pid, key, x, y})` (px) | ax `element_index` sets focus then posts the key; **px** pixel-clicks `(x,y)` to focus, then sends the key |
| Send key to pid | `press_key({pid, key, modifiers})` | no focus change; key goes to pid's current focus |
| Modifier combo | `hotkey({pid, keys})` (no focus) or `hotkey({pid, x, y, keys})` (px) | e.g. `["cmd","c"]` / `["ctrl","c"]`; posted per-pid, not HID tap. **px** pixel-clicks `(x,y)` to focus a field first, e.g. `["cmd","v"]` to paste into it |

**All keyboard/text primitives require `pid`.** There is no
frontmost-routed variant — every key goes to the named target via
the platform's per-pid event-post path, so the driver cannot leak
keystrokes into the user's foreground app.

**Why `element_index` is the primary path:** works on hidden /
occluded / off-desktop windows, no focus steal, stable across
rebuilds, labels tell you what you're clicking. Reach for pixel
coordinates only when the accessibility tree can't.

## Cross-platform parameter contract

The capture, dispatch, and addressing params — `session`,
`delivery_mode`, `capture_mode` (deprecated/ignored — see the behavior
matrix; still in the schema only so old callers don't error), `scope`,
`modifier`, `button`, `element_index`, `element_token` — are a **shared
schema contract**: identical *shape* (`type`/`enum`/`items`) on macOS,
Windows, and Linux.
They compose from canonical fragments in
`cua-driver-core::tool_schema` (+ `capture_mode`), and a CI gate
(`schema_consistency_test`) runs every tool's live `tools/list` through a
structural checker on each platform, so the three surfaces can't
silently drift. *Contributor note:* when you add or edit one of these
shared params on a tool, pull from the fragment — don't re-hand-write the
JSON, or the gate fails. (Descriptions may legitimately vary per tool;
the gate compares shape, not prose.)

Two consequences for callers:

- **`session` is accepted on every action and cursor tool, on all three
  platforms.** It's cursor-wired where the platform glides a cursor and
  schema-accepted everywhere else — so the same `session` you pass on
  macOS is no longer *rejected* by Windows/Linux, which previously
  refused unknown keys via `additionalProperties:false`.
- **`delivery_mode` (`"background"` default / `"foreground"`) is on the
  whole input family** — `click`, `double_click`, `right_click`, `drag`,
  `scroll`, `type_text`, `press_key`, `hotkey` — uniformly. The
  `foreground` rung briefly fronts the target, acts, then restores the
  prior frontmost: the explicit last resort when a background attempt
  didn't land. **`foreground` is a reaction, never a prediction.** Always
  fire the `background` default first and let the driver tell you it
  can't (a `background_unavailable` error or `escalation.recommended ==
  "foreground"`) — or observe a verified no-op — *before* you escalate.
  Do **not** reason "it's a GTK/Chromium/Electron app, so background will
  drop, so I'll front up-front": the toolkit lists in the tool schemas
  are the *driver's* internal detectors, not a checklist for you to front
  on a guess. (Concretely: GIMP's GTK toolbox accepts background pixel
  clicks fine — a preemptive foreground click there just steals the
  user's focus for nothing.) What each platform's *background* rung can
  actually carry differs (e.g. a Windows background click can't carry
  `modifier` state — see `WINDOWS.md`); the schema is uniform, the
  residual limits are per-OS.

**Required-set contract.** `click` requires nothing (`required:[]`),
`scroll` requires `["direction"]`, `zoom` requires
`["window_id","x1","y1","x2","y2"]` — same on every platform. `pid` is
**conditionally** required (needed unless a windowless desktop-scope
call) and validated in code with a clear error, NOT pinned in the schema
— so omitting `pid` for a desktop-scope action is no longer
schema-rejected.

Genuinely platform-specific params stay OUT of the shared contract by
design (launch-app identifiers, the Windows-only `debug_window_info`, the
macOS-only `check_permissions.prompt`). The per-OS files list the
residuals that matter when you drive on that platform.

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
after selection, buttons may become disabled, etc.). The re-snapshot
gives you both the tree and the screenshot, so you cross-check the tree
diff against the pixels in one call — and when you're only confirming a
tree change, `include_screenshot:false` skips the grab.

Switch to an **element px action** only on a real signal: the action
response carried `effect:"suspected_noop"`, the re-snapshot came back
`degraded` (empty tree → non-AX surface), the tree looks
unchanged/unreadable or disagrees with the screenshot, or
`escalation.recommended` points you there (`px`). That's the
verify-then-escalate ladder in the behavior-matrix section. If the tree
is unchanged AND the screenshot confirms nothing moved, the action
likely failed silently — **tell the user what you attempted and what
you observed**, don't paper over with "done" language (and consider
`delivery_mode:"foreground"` when `escalation.recommended ==
"foreground"`). Agents that skip this step report success on
silently-dropped actions — the single most common failure mode.

## Recording trajectories

Session-scoped action recording + replay, for demos, regressions,
and training data. Only invoke when the user explicitly asks to
record a session — the skill does not auto-enable this. CLI surface:
`cua-driver recording start|stop|status`; raw tools:
`start_recording` / `stop_recording`. Video capture (main display →
`recording.mp4`) is on by default; pass `record_video: false` to opt out.

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
| `The user doesn't want to proceed with this tool use. The tool use was rejected …` | The harness uses this *exact* string for BOTH a permission-prompt denial AND a manual interrupt (Esc / stop) of a running tool — they are indistinguishable from the tool result | Treat as "tool canceled, no result, await the user." Do NOT paraphrase ("you stopped me") — quote the literal message and name the canceled tool + its args, so the user can tell what was in flight vs. what landed |

Platform-specific errors (TCC dialogs on macOS, Session 0 / UAC
prompts on Windows, AT-SPI bus issues on Linux) live in their
respective companion files.

## Things to avoid

- **Never** reuse an `element_index` across a re-snapshot of the same pid.
- **Don't conflate the two addressing modes.** The tree gives you
  `element_index` handles; the screenshot (same call) gives you the
  pixel frame. An **element ax action** addresses by index, an
  **element px action** by `x,y`. Default to `element_index` and only
  do a px action on a real signal (`suspected_noop` / `degraded` /
  repeated labels / tree-disagrees-with-pixels). Don't pass an
  `element_index` you read off the screenshot, and don't pixel-click a
  coordinate you computed from the tree's (possibly lying) frame
  without checking it against the image.
- **Prefer accessibility actions over pixels.** `click({pid, x, y})`
  works for canvas / WebView regions, but it lands blindly on raw
  coordinates. Exhaust accessibility paths (menu bars, cmd-k palettes,
  toolbar items, keyboard shortcuts) before dropping to coordinates.
  (The AX path does **not** skip the agent-cursor overlay — it seeds and
  pulses the session cursor and draws a focus rect on the targeted
  element; it just doesn't play a long glide on the very first action.
  See "Agent cursor overlay" for the demo-recording caveat.)
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
