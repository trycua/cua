---
name: cua-driver
description: Drive a native GUI app (macOS, Windows, Linux) via the cua-driver CLI (default) or MCP server; snapshot its accessibility tree, act through snapshot-bound element tokens, native menu paths, exact window geometry, or pixel coordinates, and verify from fresh state. Use when the user asks you to operate, drive, automate, or perform a GUI task in a real application on the host, or to continue, resume, or recall recent Cua activity.
version: 0.24.0 # x-release-please-version
metadata:
  openclaw:
    requires:
      bins:
        - cua-driver
    envVars:
      - name: CUA_DRIVER_EMBEDDED
        required: false
        description: Set to 1 when a macOS host app launches the driver in embedded mode.
      - name: CUA_DRIVER_HOST_BUNDLE_ID
        required: false
        description: Bundle identifier of the macOS host app in embedded mode.
      - name: CUA_DRIVER_PATH
        required: false
        description: Optional path to a cua-driver binary used by an embedding host.
      - name: CUA_DRIVER_RS_ENABLE_WAYLAND
        required: false
        description: Set to 1 to enable the native Wayland backend.
      - name: CUA_DRIVER_RS_MCP_HTTP_PORT
        required: false
        description: Optional port for the local MCP HTTP endpoint.
      - name: CUA_DRIVER_RS_MCP_HTTP_TOKEN
        required: false
        description: Required host-generated bearer token when the local MCP HTTP endpoint is enabled.
    homepage: https://cua.ai/docs/cua-driver
---

# cua-driver

Orchestrates cross-platform app automation via `cua-driver`. Whenever
a user asks to drive a native app, follow the loop in this skill
rather than calling tools ad-hoc — the snapshot-before-action
invariant is not optional and silently breaks if you skip it.

## Consult recent Cua activity only for continuation

When both `history_status` and `history_query` are advertised and the user asks
to continue, resume, or recall prior Cua work, call `history_status` first. If
history is healthy and access is admitted, make one bounded initial
`history_query` before broad application or window discovery. Treat returned
metadata only as a lead and verify current state through the least intrusive
appropriate source. Content, geometry, arguments, results, and user intent
omitted from the metadata remain unknown.

Make another bounded query only when the initial slice exposes a relevant
session or sequence boundary; never broaden a query to reconstruct excluded
fields.

Continue without history when either tool is absent, access is denied, the
query is empty, or history is unhealthy. Do not query history for unrelated
tasks merely because the tools are advertised, and never mutate history
lifecycle or settings.

## Platform-specific reading — read this first

This file is the **cross-platform core**: snapshot invariant, CLI vs
MCP choice, tool surface naming, behavior matrix, canonical loop,
pixel-click contract, common failure modes. The platform-specific
material (forbidden-list, accessibility tree implementation, launch
semantics, click dispatch) lives in companion files in this same
directory:

- **macOS** — read `MACOS.md` (no-foreground contract, forbidden
  `open`/`osascript`/`cliclick` invocations, AXMenuBar navigation,
  SkyLight pixel-click dispatch).
- **Windows** — read `WINDOWS.md` (UIA tree vs AX, UWP /
  ApplicationFrameHost hosting, layered UIA+PostMessage click chain,
  Session 0 isolation, Windows-specific focus-steal vectors).
- **Linux** — read `LINUX.md` (X11 background input via AT-SPI +
  XSendEvent and compositor-specific Wayland capabilities).

Cross-cutting topics also have their own files:

- `BROWSER.md` — exact native-window binding, explicit browser preparation,
  typed Chromium/Electron page tools, input trust classes, and native
  fallbacks for browser chrome and unsupported engines.
- `RECORDING.md` — session recording + `replay_trajectory`.

Use whichever combination matches the host. When in doubt, run
`cua-driver doctor` — it reports the platform and the right entry
point.

## Start with the narrowest semantic route

Before opening or operating an application, name the desired postcondition and
use the first applicable route below. Verify the result in the same domain
before stopping or advancing:

0. **Caller-provided headless/background operation for a non-GUI outcome.**
   Prefer an exact application API/SDK, service or database client, CLI, or
   filesystem operation over imitating a user. This includes batch-safe file
   moves, renames, copies, directory creation, archive extraction, data
   conversion, and process inspection. Read the resulting semantic state back;
   a zero exit status alone is not proof.
1. **Typed Cua operation for an application or window outcome.** Use
   `set_window_frame` for exact geometry, `invoke_menu` for a known native
   application-menu path, typed browser tools for supported page content, and
   clipboard tools for clipboard state. Verify with
   `list_windows`, `get_browser_state`, or `clipboard_read`, respectively.
2. **Background accessibility action.** Use a fresh AX/UIA/AT-SPI target.
3. **Background pixel action.** Use the pixels from the same state snapshot.
4. **Foreground delivery.** Retry only the action that evidence says could not
   land in the background.
5. **Desktop fallback.** Select an exact desktop target for that call only.
   Later calls may return to an exact window target in the same session.

Use Cua Driver when the outcome lives in an application's UI or window state,
or when the user explicitly asks to operate that GUI. Once the task crosses
that boundary, do not replace Cua's targeted and verified actions with shell
scripts that mutate the app UI. A shell is a capability of the calling agent,
not of the Cua Driver MCP server; an MCP-only client must not assume one exists.

### Filesystem outcomes and GUI fallbacks

When the requested outcome is a filesystem change and the caller has a
headless filesystem or command capability, keep it on rung 0. Enumerate the
exact source set, decide the destination-conflict policy before changing
anything, perform one batch-safe operation, then independently read back both
source and destination manifests. Do not open a file manager merely to mimic a
move, copy, or rename that the caller can execute and verify directly.

If the caller has no such capability, use the file manager as a GUI fallback
and keep each claim narrow:

1. After entering an inline rename and setting its value, commit it with the
   platform's confirmation key, then take a fresh snapshot. Value readback from
   the inline editor proves only that the editor changed; it does not prove the
   filesystem rename committed.
2. For a multi-selection, use the platform modifier (`cmd` on macOS, `ctrl` on
   Windows/Linux). On macOS and Windows, issue that modified click with
   `delivery_mode:"foreground"` so the target observes physical modifier state;
   a refused background attempt is an escalation signal, not a failed action to
   trust or repeat. Re-snapshot before the next operation. Continue only when
   every intended item is selected and the prior selection was preserved.
3. After a cross-window drag or paste, verify the destination contains the
   complete expected set and the source reflects copy-versus-move semantics.
   A delivered drag, keypress, or menu action is not file-operation proof.
4. If a destination conflict presents an unrecognized policy or ambiguous
   partial result, stop that GUI path and surface the unresolved state instead
   of retrying blindly.

### Clipboard outcomes and GUI fallbacks

When the requested postcondition is an exact value on the system clipboard,
rather than the literal gesture of selecting and copying it, keep the operation
semantic. Read the value from the narrowest typed source, call
`clipboard_write`, then prove the real clipboard state with `clipboard_read`.
For browser content, this means reading the page with `get_browser_state` and
writing the exact observed text; a passive page-text ref does not need to be
clicked first.

Use visual selection followed by the platform copy hotkey only when the user
explicitly asks for that gesture, the source cannot expose the value
semantically, or direct clipboard tools are unavailable. Treat that as a GUI
fallback: re-snapshot before acting, verify the selected range when the
application exposes it, and escalate only the delivery step that cannot land
in the background.

## The no-foreground principle (window phase)

During window-targeted background actions, **the user's frontmost app MUST NOT
change.** Every platform
has its own list of forbidden commands:

- macOS: any `open` invocation, any `osascript` that mutates GUI
  state, `cliclick`, `cghidEventTap` writes targeting another app's
  window. Full list in `MACOS.md`.
- Windows: any `Start-Process` that triggers a `ShowWindow`/`SetForegroundWindow`
  on the target, `WScript.Shell.AppActivate`, attaching to the
  foreground thread for input forwarding. Full list in `WINDOWS.md`.

If you reach for a command that says "activate", "foreground",
"raise", or "make key", stop and translate to the cua-driver tool
that does the same intent without focus-stealing.

`delivery_mode:"foreground"` is a user-visible takeover boundary, not an
automatic retry. Use it only when the user already authorized foreground
control for this workflow or after asking for approval. It may change focus,
workspace, and the compositor cursor while the action runs; restoration is
best-effort. If background delivery is unavailable and foreground control is
not authorized, stop with the driver's refusal instead of silently escalating.

A desktop target is an explicit per-call choice to operate the visible desktop
and therefore uses foreground/system input. Use it only after the narrower
window ladder has been attempted and verified. Permission policy must still
admit the display resource. Never infer desktop permission from a failed action
or a public session label.

## GUI transport defaults — prefer cua-driver over GUI shell shims

**Default transport is the `cua-driver` CLI for one-off calls** — `Bash`
shelling out to `cua-driver <tool-name> '<JSON-args>'`. Each CLI invocation
owns a disposable transport session that is cleaned up after its response.
Use one persistent `cua-driver mcp` connection for a multi-call GUI workflow
that needs shared cursor, recording, browser, or named-session state. A public
session label is not a credential and a later one-shot process cannot adopt
the previous process's lifecycle merely by repeating that label.

CLI wins for isolated inspection and management because it picks up rebuilds
instantly, failures are easier to diagnose, and there's no per-tool
schema-load overhead. Persistent MCP wins for an ordered action loop.

Every reference to `click(...)`, `get_window_state(...)` etc. in this
skill means `cua-driver click '{...}'` — translate to MCP form only
when MCP is requested.

### Claude Code computer-use compatibility flag

For normal Claude Code use, keep the default CLI or `cua-driver` MCP
server path above. If the user explicitly wants Claude Code's
vision/computer-use-style flow, they can register:

```bash
cua-driver mcp-config --client claude   # then paste + run the printed line
```

The compatibility flag is retained for old setup snippets, but the standalone
`screenshot` tool was removed. It does not add or replace tools. Use
`get_window_state({pid, window_id})` for a window-local accessibility snapshot
and PNG, or `get_desktop_state()` for an explicitly authorized desktop capture.

## Using cua-driver from the shell

Tool names are `snake_case`, management subcommands are
`kebab-case` — no ambiguity. Tools invoked as `cua-driver
<tool-name> '<JSON-args>'`. Management subcommands:

- `cua-driver serve` — start an explicit persistent service when short-lived
  clients must share runtime state or a platform identity. Bare MCP owns its
  runtime directly on Windows/Linux and uses the signed app service on macOS;
  `cua-driver mcp --socket <endpoint>` selects a service explicitly.
  One-shot CLI tool calls still use the service path. macOS users: see
  `MACOS.md` for the LaunchServices-routed launch form.
- `cua-driver stop` / `status`
- `cua-driver list-tools`, `describe <tool>`
- `cua-driver recording start|stop|status` — see `RECORDING.md`
- `cua-driver check-update [--json] [--no-cache]` — read-only "is a newer release available?" probe. Same payload as the `check_for_update` MCP tool; pair with `cua-driver update --apply` to install.

Canonical multi-step workflow within one persistent MCP connection (example
shape — platform-specific launch idioms in the per-OS companion file):

```bash
# Start the service once, then connect one MCP client with `cua-driver mcp`.
# The calls below are tool calls on that same connection, not separate shell
# invocations of `cua-driver <tool>`.
launch_app({"bundle_id":"..."})
# → {pid: 844, windows: [{window_id: 10725, ...}]}
get_window_state({"pid":844,"window_id":10725})
# Use the returned structuredContent.elements[].element_token:
click({"pid":844,"element_token":"s0000002a:14"})
verify_state({"pid":844,"window_id":10725,"expect":[{"element":{"selector":{"label_contains":"Saved"},"exists":true}}]})
```

For Chromium page content, keep the same native window selection but switch to
the browser capability loop: use one lifecycle session, bind `(pid, window_id)` with
`get_browser_state`, snapshot the returned tab, then use `browser_click`,
`browser_type`, or `browser_navigate`. Read `BROWSER.md` before using this
route. Browser target ids, tab ids, and refs are session-scoped and stale refs
must be replaced by a fresh snapshot.

## Agent cursor overlay

Visual cursor overlay for demos and screen recordings. It initializes on the
first cursor-bearing action, including `move_cursor`, and follows the
transport's implicit or named lifecycle session. Toggle a named cursor with
`set_agent_cursor_enabled` to hide or re-show it. The embedded
`cua.default` theme uses a session-colored pointer over a larger,
cursor-shaped glow in the same session color. The glow fades to transparent
around the full silhouette. Action marks use the same
session-colored center and white-outline treatment, plus a tighter, softer
glow. This pairing preserves contrast across varied backgrounds. It provides animations for
idle, observe, click, drag, scroll, text, key, navigation, app, transfer,
recording, and system activity. Motion knobs:
`set_agent_cursor_motion` takes any subset of `start_handle`,
`end_handle`, `arc_size`, `arc_flow`, `spring` — tuneable at runtime,
persisted to config.

**Agent-control safety.** Keep this overlay enabled whenever the agent is
controlling pointer or keyboard input. Cursor-bearing and keyboard actions
automatically re-show their session cursor, even if it was hidden while idle.
On Linux, ordinary `move_cursor({x,y})` moves only this synthetic cursor.
`move_cursor({x,y,scope:"desktop"})` is the explicit escape hatch that moves
the user's compositor cursor and must not be used unless the user asked for
desktop-pointer control. Raw Wayland input that requires
`delivery_mode:"foreground"` crosses the same user-visible takeover boundary.

Delivery and target context is shown as host-owned chips inside the session
badge. Themes own the twelve action animations only. The session name and
context chips fade independently, so an active tool can show its execution
context without revealing a session name that has already faded.

**Per-session cursors.** Each MCP session automatically owns its own
cursor, keyed by the session's id (the proxy mints one session id per
MCP connection and the daemon scopes the cursor, config overrides, and
recording to it). The CLI and SDK contracts take the declared `session`
explicitly. Cursor-theme controls no longer accept `cursor_id` or the legacy
shape/color/image fields. Input-delivery tools may still use `cursor_id` to
name a virtual pointer; it never selects artwork. The default cursor is Cua
blue, while each named session receives a stable fill from the built-in
palette. Select only preinstalled
themes with `set_agent_cursor_theme`; theme source paths and inline animation
data are never accepted through an agent tool. Use the trusted local
`cua-driver cursor-theme` workflow to validate, compile, preview, install,
list, or remove custom themes.

**Visibility caveat (AX runs).** On a pure accessibility-action run
(clicking by `element_index`), the first action **seeds the cursor
on-screen a short distance from the target and plays a brief glide +
pulse** — not the long Bezier sweep a cursor already on-screen would
trace from its previous spot. It's subtle and easy to miss in a
recording. If you want a clearly _gliding_ cursor for a demo or screen
recording, do a pixel click (`click({pid,x,y})`) or a `move_cursor`
first to put the cursor on-screen; subsequent AX actions then glide the
full path normally.

Pixel `click` already glides the overlay. Do not call `move_cursor`
immediately before `click` on the same target; that plays two glides.
Use `move_cursor` to place the overlay without clicking, or as the
one-time seed above before AX actions.

Requires a suitable UI event loop. Service and private-worker runtimes provide
one. On macOS, a same-process SDK runtime or `cua-driver mcp --direct` without
a certified host main-thread adapter returns a structured
`facility_unavailable` result for overlay operations; do not treat that as a
successful cursor move. One-shot CLI adapters do not own an overlay
themselves.

## The core invariant — snapshot before and verify after every action

**Every action MUST be bracketed by observation for the session's effective
scope.** Use `get_window_state(pid, window_id)` before a window action (or
`get_desktop_state(session)` in desktop scope), then use `verify_state` for an
expressible window-scoped postcondition. In effective desktop scope,
`verify_state` is intentionally refused with `window_scope_disabled`; verify
with a fresh `get_desktop_state` result and agent-owned visual/semantic reading.

- **Before** — the pre-action snapshot resolves the `element_index`
  you're about to use. Indices from previous turns are stale; the
  server replaces the element index map on every snapshot, keyed
  on `(pid, window_id)`. Indices from turn N don't resolve in turn
  N+1, and indices from window A don't resolve against window B of
  the same app. Skip this and element-indexed actions fail with
  `No cached AX state`.
- **After** — `verify_state(pid, window_id, expect)` checks a bounded,
  deterministic postcondition. Results are `satisfied`, `unsatisfied`, or
  `unknown`; `unknown` never means success. Set `include_screenshot:true` when
  the outcome also needs visual reading. The driver returns that final image
  without interpreting it. A multimodal agent harness reads the image and owns
  the stop/retry/ladder decision.

`unknown_reason` distinguishes invalid/unsupported predicates, untrusted web
content, ambiguous matches, missing targets, unavailable observations, and
`stability_unproven`. A positive final sample that was not observed for the
requested consecutive sample count is `stability_unproven`, not success.
Negative element existence is conservative: when an accessibility projection
cannot prove its search domain exhaustive, absence remains `unknown`.

Do not make the driver invent task meaning or retry actions automatically.
For postconditions not expressible by `verify_state`, take a fresh state
snapshot and let the agent judge the tree and/or image explicitly. This applies
to pixel clicks and desktop actions too.

### Read action facts without confusing them with task success

A successful action returns `effect` and `route`, with optional typed
`delivery`, `evidence`, and `escalation`. These fields describe the actuator;
they do not declare the user's task complete.

- `confirmed` means the driver has publishable value readback or window-change
  evidence for that action.
- `partial` means only `delivery.delivered_count` was delivered.
- `unverifiable` means the driver cannot prove the effect.
- `suspected_noop` means available evidence suggests no useful change.
- `refused` means the selected route deliberately did not deliver.

The route vocabulary is intentionally cross-platform:
`accessibility`, `synthetic_events`, `global_input`, `dom`, and
`trusted_input`. Do not branch on private OS transport names.

An optional escalation is a harness instruction, never an automatic retry:

- `pixel`: refresh visual state and choose an exact pixel target;
- `foreground`: explicitly select foreground delivery if the authorization
  stack admits the tool and exact target;
- `page`: bind the native window to a supported browser page route;
- `session`: a legacy compatibility signal from an older capture-scope daemon;
  current callers choose a desktop target on the specific action instead.

Branch on the closed reason vocabulary:
`route_unavailable`, `delivery_failed`, `effect_unconfirmed`,
`suspected_noop`, and `permission_required`.

After any action, keep using `verify_state` or a fresh state snapshot for the
actual task postcondition. The multimodal harness owns visual reading and the
decision to stop, retry, or advance the ladder.

## Choose the target on each action

A session owns lifecycle, cursor, recording, cleanup, and telemetry state. It
does not store the current capture modality. Select an exact target on each
action:

```jsonc
{"target":{"kind":"window","pid":844,"window_id":10725}}
{"target":{"kind":"desktop","display_id":"primary"}}
```

The window target uses window-local coordinates and the background/foreground
delivery ladder. The desktop target uses screen coordinates and foreground
delivery. A desktop action does not disable window tools for later calls.

`start_session` is optional. For a multi-call run, prefer a short public
`session` label and pass the same label on every call that accepts it. The label
is call-scoped: if a later call omits it, that call uses the authenticated
transport's implicit session instead. Unnamed calls on one transport reuse that
implicit identity. The default idle TTL is five minutes. Call
`start_session(session)` to name or configure a run before acting, or to revive
an ended name.

Do not use `config set capture_scope` or `set_config`; that key is retired and
stale values on disk are ignored. `start_session.capture_scope`,
`get_session_state`, and `escalate_session` are deprecated compatibility
surfaces. There is no `deescalate_session`. Reserved fields such as
`_session_id` are transport metadata and cannot create authority.

## Keep authorization separate from sessions

The trusted host selects one permission profile at startup. `standard` keeps
the normal profile behavior and residual approval requirements, `bounded`
requires a reviewed capability manifest and has no runtime approval path, and
`unrestricted` bypasses Cua approval prompts after explicit risk acceptance.
Hard invariants plus managed and user policy remain binding in every profile.

An optional capability manifest is a deny-by-default ceiling in `standard` and
`unrestricted`; `bounded` requires one. It can remove tools or typed resources
from the selected profile, but it cannot grant a tool, resource, or approval
bypass that another authorization layer denies. Approval is considered only
after the tool and every adapter-attested resource are inside manifest scope.

Use the canonical startup pair together:

```bash
cua-driver mcp \
  --permission-mode standard \
  --capability-manifest ./capabilities.yaml \
  --approve-capability-manifest
```

Capability manifest v3 omits file-level `mode` and `ask.tools`. Its
`allow.tools` list is nonempty. Lifetime fields are optional in `standard` and
`unrestricted`; `bounded` requires both `expires_after` and `idle_timeout`.
The older `--session-policy` names remain compatibility aliases and must not be
used in new configurations.

Starting, ending, naming, reconnecting, or omitting a session never changes
permission authority. A public session label is lifecycle metadata, never a
grant, caller identity, or bearer credential.

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

> **`capture_mode` is DEPRECATED and ignored.** It is still _accepted_
> on `get_window_state` so old callers don't error, but it has **no
> effect** — both the tree and the screenshot come back regardless of
> what you pass (`ax`, `vision`, `som`, anything). There is no
> `ax`/`vision`/`som` capture choice anymore. Drop the word "vision"
> for perception entirely. (The tool named `screenshot` is separate —
> raw PNG, no AX walk — and unrelated.)

### The modality is chosen at ACTION time — `ax` vs `px`

You don't pick a capture mode; you pick **how you address the target**
on the action call, and that one choice selects the rung:

- **element ax action** — pass `element_token` (preferred), or the exact
  `element_index` + `snapshot_id` pair from the same response.
  Dispatches through the **accessibility rung**: AXPress (macOS) / UIA
  Invoke (Windows) / AT-SPI `doAction` (Linux). Backgroundable,
  z-order-independent, and the only **driver-verifiable** rung.
- **element px action** — pass `x`, `y`. Dispatches through the **pixel
  rung**, reading the coordinate straight off the screenshot that's
  already in the `get_window_state` response. Best-effort; the caller
  confirms the effect.

`ax`↔`element_index`, `px`↔pixel `x,y`. We retired the word "vision"
for the _dispatch_ path — it conflated perception with dispatch.
Perception is always both; dispatch is `ax` or `px`.

**The keyboard family has both forms too.** `type_text`, `press_key`,
and `hotkey` take a snapshot-bound element target (ax) **or** `x,y` (px) — mutually
exclusive, same as the pointer tools. The px form **pixel-clicks at
`(x,y)` to establish real renderer focus, then delivers the
keystroke(s)** to the now-focused element (it reuses `click`'s
coordinate translation + `delivery_mode`). That gives e.g.
`type_text({pid, window_id, x, y, text})` as a one-call focus-then-type
for Chromium/Electron inputs the AX path can't reach, and
`hotkey({pid, x, y, keys:["cmd","v"]})` to paste into a specific field.

**Typing default (the ladder).** Call `type_text` directly with
`element_token` (ax) — it targets the field, no pre-click. On
Electron/Catalyst the AX layer echoes the write without rendering it,
so the driver returns `effect:"unverifiable"` with
`escalation.target:"pixel"` there (never a false `effect:"confirmed"`) —
follow it, and cross-check the
screenshot in the response (the only ground truth). Escalate to the px
form — `type_text({pid, window_id, x, y, text})` — which pixel-clicks
to focus, then types. **If the target control is closed** (a search
button, a collapsed field), AX-press to open it first (AX actions work
in the background): a px focus-click won't reliably open _and_ focus a
closed control, so the text leaks into whatever's already focused.
Escalate to `delivery_mode:"foreground"` only if it still drops.

**`set_value` stays AX-only by design** — use it when the intent is to
replace a control's whole value: dropdowns, checkboxes, sliders, steppers,
and native text fields such as Finder's inline rename editor. Use
`type_text` when the intent is to insert text at the current selection or
cursor. Its pixel counterpart is a `click`/`drag` on the control, not a
"set value at a pixel." So: insert text → `type_text` (ax+px); replace a
surfaced native value → `set_value`; pixel-manipulate a control →
`click`/`drag`.

**Action responses carry closed action facts**

Use the `effect`, `route`, optional `delivery`, `evidence`, and
`escalation` rules in “Read action facts without confusing them with task
success” above. The old `verified`, `path`, coordinates, scope, and
`escalation.recommended` response fields no longer exist.
The full wire contract and 0.14 migration notes are in
`../../../docs/action-result-contract.md`.

A successful accessibility value write can still return
`effect:"unverifiable"` when the provider publishes its new value only after
the action call unwinds. Take a fresh snapshot before retrying; an immediate
retry can duplicate text. An explicit pixel escalation is reserved for a web
surface whose accessibility layer echoed the write without proving that the
renderer observed it.

`get_window_state` itself, when the AX tree comes back empty (a non-AX
surface like Electron/Chromium/canvas), returns `degraded: true`
plus an observation-specific escalation hint — normally pointing at pixels (you
still have the screenshot from the same call to click off).

**Platform nuance for action escalation.** On **Wayland** an unfocused
window cannot be pixel-targeted in the background (libei →
`background_unavailable`), so the action target is
**`foreground`, not `pixel`**. macOS, X11, and most Windows surfaces
can pixel-target in the background, so they target `pixel`. See
`LINUX.md` / `WINDOWS.md`.

## The verify-then-escalate ladder (algorithm)

Every snapshot already hands you both the tree and the screenshot, so
verifying never means "go take a screenshot" — it means cross-check
the tree against the pixels you already have, and only change
_dispatch rung_ on a real signal. Walk the rungs:

```
# Routes 0–1 — resolve non-GUI, exact geometry, and supported page outcomes first
# Use a caller-provided semantic operation for a non-GUI outcome, then read it back.
# For exact window geometry: set_window_frame(...), then list_windows(...) readback.
# For a known native menu command: invoke_menu(pid, window_id, path), then verify its effect.
# For supported page content: get_browser_state(...), typed browser action, refresh refs.
# Continue below only when the postcondition actually requires native UI interaction.

# Route 2 — element AX/UIA/AT-SPI action, backgrounded
get_window_state(pid, window_id)            # tree + screenshot, both, always
resp = click(pid, element_token)            # or type_text / set_value / press_key
check = verify_state(                       # bounded structured read-back
    pid, window_id,
    expect=[...],
    include_screenshot=true                 # optional evidence for multimodal harness
)

if check.status == "satisfied":
    done                                    # driver-verified

if check.status == "unknown" and check has an image:
    harness reads the image                  # model-owned visual interpretation
    if visual outcome is satisfied: done

# escalate only on a real signal
if resp.effect == "suspected_noop"
   or resp.escalation.target == "pixel"
   or get_window_state.degraded            # empty tree → non-AX surface
   or check.status != "satisfied"
   or the tree looks wrong vs the screenshot:   # e.g. an h:1 / off-viewport row

    # Route 3 — element px action off the SAME screenshot
    pick the target pixel from the screenshot already in the response
    click(pid, x, y)                        # background pixel — still no foreground
    verify_state(..., include_screenshot=true)
    if it landed: done

# Route 4 — background delivery was dropped (insert/click never arrived)
if resp.escalation.target == "foreground"
   or the px action still did nothing:
    require existing user authorization for visible foreground control
    otherwise stop and ask; do not retry automatically
    re-call the same action with delivery_mode:"foreground"
    # on Wayland this is the ONLY escalation — px-bg can't target an
    # unfocused window there; see LINUX.md
    verify again

# Route 5 — per-call desktop fallback
# Reach this only after semantic, AX, window-pixel, and foreground-window
# delivery have all been exhausted and verified ineffective.
get_desktop_state()                         # full primary display
desktop_action(target={kind:"desktop", display_id:"primary"}, ...)
get_desktop_state()                         # verify in the same coordinate frame
```

The two ideas to hold onto: (1) the AX tree **lies** on canvas / web /
Catalyst / virtualized surfaces, so an unchanged-or-bogus tree plus
`suspected_noop`/`degraded` — or a tree that simply disagrees with the
screenshot — is your cue to do an **element px action** off the
screenshot you already have; (2) `px` is a _conscious_ switch to the
pixel addressing path, not a different capture.

**Window state → what works**

| state                      | `get_window_state`                                                                             | element-index click (AX/UIA) | `press_key` commit                                    | pixel click                    |
| -------------------------- | ---------------------------------------------------------------------------------------------- | ---------------------------- | ----------------------------------------------------- | ------------------------------ |
| frontmost                  | ✅                                                                                             | ✅                           | ✅                                                    | ✅                             |
| backgrounded / visible     | ✅                                                                                             | ✅                           | ✅                                                    | ✅                             |
| **minimized**              | ✅                                                                                             | ✅ (actions fire in place)   | ❌ silent no-op — use `set_value` or click equivalent | ❌ no on-screen bounds         |
| hidden                     | ✅                                                                                             | ✅                           | depends                                               | ❌                             |
| on another desktop / Space | ⚠️ tree may be stripped on some apps — response carries `off_space: true` so you can detect it | ✅                           | ✅                                                    | ❌ not in current-desktop list |

**Critical cell — minimized + keyboard commit.** The keystroke
reaches the app but accessibility focus doesn't propagate to renderer
focus on a minimized window. Workarounds in order of preference:
`set_value` to write the field's entire value directly, or
element-index-click a commit-equivalent button (Go, Submit,
checkbox). Tell the user the window needs to un-minimize only as a
last resort.

## The canonical loop

```
# for multi-call work, repeat the same session label on every call that accepts it
launch_app(target, session)
  → pick window_id from the returned `windows` array
    (or call list_windows(pid) separately)
  → get_window_state(pid, window_id)
    → [act]  # pass target={kind:"window", pid, window_id}
  → verify_state(pid, window_id, expect)  # structured check; optional image
end_session(session?)             # optional explicit cleanup
```

An existing-profile `end_session` restores the Chromium browser's
remote-debugging toggle when Cua enabled it and no other Cua session still uses
that browser process and dismisses the exact native consent prompt. If exact
cleanup cannot be proven, session cleanup fails closed and a later
`end_session` retries it.

For screen-absolute work, replace the window portion with
`get_desktop_state() → action(target={kind:"desktop",display_id:"primary"}, ...)
→ get_desktop_state()`. Desktop actions use coordinates from that exact
full-display image.

`launch_app` now returns a `windows` array alongside the pid, so the
common case collapses to two calls (`launch_app` → `get_window_state`)
without a separate `list_windows` hop.

**Prefer a named session for multi-call work.** Choose a short label (for
example, `session: "research-1"`) and pass the same value on every call that
accepts it. Passing it once is not sticky: a later call that omits `session`
uses the transport's implicit session. Call `start_session(session)` when you
need to name or configure the run before acting, or to revive a name after
`end_session`. For one-off or deliberately unlabeled work, omission is valid
and the transport still gets one private lifecycle identity and visible agent
cursor. A public label makes inspection and cleanup easier, but it is not a
credential. End with `end_session` when useful; transport close or the
five-minute idle TTL also reclaims it.

**Concurrent runs/subagents:** each transport gets its own implicit session.
Also,
`launch_app` is idempotent — two runs that
launch the same app get the **same** instance (and on single-instance apps
like Calculator, the same window), so they clobber each other. Give each run
its **own connection** (for independent lifecycle/cursor ownership) AND pass
`creates_new_application_instance: true` to `launch_app` (→ its own window).
The element cache is keyed on `(pid, window_id)` and the cursor on the private
lifecycle session, so distinct instances and transports keep the runs isolated.

**Parallelism vs. ordering.** Distinct sessions give distinct _cursors_, not
distinct _connections_. Subagents that share one `cua-driver mcp` (stdio)
connection have their tool calls **serialized** by the transport — they take
turns, not run in parallel. That's not a correctness problem (session + window
isolation means they can't collide), just a throughput one. For genuinely
parallel agents, give each its **own connection**: separate `cua-driver mcp`
processes, or point each agent's MCP client at the daemon's HTTP endpoint.
Set `CUA_DRIVER_RS_MCP_HTTP_PORT` and a host-generated
`CUA_DRIVER_RS_MCP_HTTP_TOKEN` of at least 32 characters, then send
`Authorization: Bearer <token>` to `POST http://127.0.0.1:<port>/mcp`. The daemon
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

### Snapshot and act with a snapshot-bound target

Call `get_window_state({pid, window_id})` with the `window_id` from
`launch_app`'s `windows` array (or a fresh `list_windows({pid})` if
you're interacting with a long-lived process). It returns **the tree
and the screenshot together** by default, so you can both dispatch by
`element_token` and ground on pixels from one call — no config change,
no mode flip. When you're just re-indexing before an element ax action
and don't need fresh pixels, pass `include_screenshot:false` to skip
the grab (a perf knob, not a modality choice).

The response carries:

- `tree_markdown` — every actionable element tagged `[N]`; the structured row
  with the same `element_index` carries its opaque `element_token`. The tree can be very large (Finder is
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
cua-driver get_window_state '{"pid":N,"window_id":W,"screenshot_out_file":"/tmp/shot.png"}'

# CLI --screenshot-out-file flag is equivalent
cua-driver get_window_state '{"pid":N,"window_id":W}' --screenshot-out-file /tmp/shot.png
```

Pass `screenshot_out_file` when using `get_window_state` via CLI or
from an agent whose context window can't absorb ~31 KB of inline
base64 (e.g. OpenCode with a local Ollama model). The MCP image
content block is omitted from the response when this param is set —
the model receives only the tree and `screenshot_file_path`, then
reads the image from disk.

**The tree and the screenshot are complementary, not redundant — and
they come from the _same_ call.** Each half carries signal the other
can't, which is exactly why you cross-check them:

- The **tree** tells you _what's clickable_ — roles, labels,
  snapshot-bound element handles, advertised actions, parent-child
  structure. This is the ground truth for an **element ax action**.
- The **screenshot** tells you _which one_ — the tree often has many
  buttons with similar or empty labels ("Delete", "OK", anonymous
  UUID-labeled buttons, repeated static-text), and visual context
  disambiguates. Captions, colors, layout relationships visible in
  pixels often don't show up in the tree at all (especially in
  Chromium / Electron / web content) — and the screenshot is where you
  catch the tree _lying_ (an `h:1`/off-viewport row, a Catalyst null
  value).

Default to dispatching by `element_token` (the **element ax action**) —
it's the verifiable, backgroundable rung. Do an **element px action**
(`x,y` off the same screenshot) when the tree can't disambiguate
(repeated/empty labels), when it's empty (`degraded` — non-AX
surface), when an action came back `suspected_noop`, or when the tree
disagrees with the pixels. You never re-capture to switch — the
screenshot is already there; you just change _how you address_ the
target.

Reach for pixel coordinates only when the target is a canvas /
video / WebGL / custom-drawn surface that isn't in the tree
(see "Pixel-coordinate clicks" below).

The `actions=[...]` list on each element is **advisory**, not
authoritative. cua-driver does not pre-flight check against it —
`click({pid, element_token})` always attempts the default action (or
the action you pass) and surfaces whatever the target returns. **Try
the click first** — pivot only on the returned error code.

### Tool dispatch table

Every row assumes a fresh `get_window_state`. Prefer its opaque
`element_token`. If a client uses the visible integer instead, it must send
the response's `snapshot_id` with `element_index`; bare indices fail closed in
0.17. Pixel-only forms remain independent of snapshot handles.

| Intent                           | Tool                                                                                                            | Notes                                                                                                                                                                                                                 |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| List an app's windows            | `list_windows({pid})`                                                                                           | returns `window_id`, `title`, `bounds`, `z_index`, `is_on_screen`, `on_current_space`. Already included in `launch_app`'s response — only call this for long-lived pids                                               |
| Set an exact window frame        | `set_window_frame({pid, window_id, x, y, width, height})`                                                       | uses the platform window manager and returns `confirmed` only after geometry readback; inspect `list_windows` again before continuing when the result is not confirmed                                                |
| Invoke a native application menu | `invoke_menu({pid, window_id, path:["Window","Arrange","Left"]})`                                              | resolves exact immediate-child labels from live native state at every hop; refuses missing, ambiguous, or disabled segments and never falls back to pixels; verify the command's semantic effect afterward          |
| Snapshot a window                | `get_window_state({pid, window_id})`                                                                            | returns `tree_markdown` + `screenshot_*`; populates the `(pid, window_id)` element_index cache                                                                                                                        |
| Verify a postcondition           | `verify_state({pid, window_id, expect, include_screenshot?})`                                                   | polls bounded structured predicates; returns `satisfied`, `unsatisfied`, or `unknown`. Optional final image is interpreted by the agent harness, never by the driver                                                 |
| Left click                       | `click({pid, element_token})` or `click({pid, window_id, element_index, snapshot_id})`                          | default `action: "press"`. Pixel form: `click({pid, x, y})` (window_id optional) — `modifier: ["cmd"\|"ctrl"]`                                                                                                        |
| Double-click / open              | `double_click({pid, element_token})`                                                                            | Default action when the element advertises one (Open on Finder items / openable rows), else stamped pixel double-click at the element's center                                                                        |
| Right click / context menu       | `right_click({pid, element_token})` or `click({pid, element_token, action:"show_menu"})`                       | Browser page content should use the typed route where available; see `BROWSER.md`                                                                                                                                     |
| Type at cursor                   | `type_text({pid, text, element_token})` (ax) or `type_text({pid, text, window_id, x, y})` (px)                  | ax focuses the element then writes via the platform's text-set primitive; **px** pixel-clicks `(x,y)` to focus the renderer, then types — the one-call fix for Chromium/Electron inputs the AX path can't reach       |
| Set whole non-text control value | `set_value({pid, element_token, value})`                                                                         | **AX-only by design** — dropdown/`AXPopUpButton`, checkbox, slider, stepper; **also the keyboard-commit workaround on minimized windows.** For text use `type_text`; to pixel-manipulate a control use `click`/`drag` |
| Scroll                           | `scroll({pid, direction, amount, by, element_token})`                                                           | synthesizes per-pid PageUp/PageDown/arrows                                                                                                                                                                            |
| Focus + send key                 | `press_key({pid, key, element_token, modifiers})` (ax) or `press_key({pid, key, x, y})` (px)                    | ax targets the element before posting the key; **px** pixel-clicks `(x,y)` to focus, then sends the key                                                                                                               |
| Send key to pid                  | `press_key({pid, key, modifiers})`                                                                              | no focus change; key goes to pid's current focus                                                                                                                                                                      |
| Modifier combo                   | `hotkey({pid, keys})` (no focus) or `hotkey({pid, x, y, keys})` (px)                                            | e.g. `["cmd","c"]` / `["ctrl","c"]`; posted per-pid, not HID tap. **px** pixel-clicks `(x,y)` to focus a field first, e.g. `["cmd","v"]` to paste into it                                                             |

`list_windows.z_index` uses one portable convention: higher integer
values are closer to the front. Select a frontmost candidate with the
maximum non-null value. If all values are `null` (as they can be on
native Wayland), use an explicit fallback; never treat `null` as zero
or infer stacking from array order. The `windows` records returned by
`launch_app` use the same convention.

In effective desktop scope, the foreground/system equivalents omit
`pid`/`window_id` and pass `scope:"desktop"`: `click`, `scroll`, `drag`,
`move_cursor`, `type_text`, `press_key`, and `hotkey`. Coordinates are
screen-absolute pixels from the latest `get_desktop_state` image.

**Window-scope keyboard/text primitives require `pid`.** They use the named
target's per-pid event-post path. Only a strict/effective desktop session may
omit `pid`, and it intentionally routes keyboard input to the current
foreground application.

**Why the snapshot-bound element target is the primary path:** works on hidden /
occluded / off-desktop windows, avoids focus steal, and fails closed after a
tree rebuild instead of silently retargeting a reused index. Labels tell you
what you're clicking. Reach for pixel
coordinates only when the accessibility tree can't.

## Cross-platform parameter contract

The capture, dispatch, and addressing params — `session`,
`delivery_mode`, `capture_mode` (deprecated/ignored — see the behavior
matrix; still in the schema only so old callers don't error), `scope`,
`modifier`, `button`, `element_index`, `snapshot_id`, `element_token` — are a **shared
schema contract**: identical _shape_ (`type`/`enum`/`items`) on macOS,
Windows, and Linux.
They compose from canonical fragments in
`cua-driver-core::tool_schema` (+ `capture_mode`), and a CI gate
(`schema_consistency_test`) runs every tool's live `tools/list` through a
structural checker on each platform, so the three surfaces can't
silently drift. _Contributor note:_ when you add or edit one of these
shared params on a tool, pull from the fragment — don't re-hand-write the
JSON, or the gate fails. (Descriptions may legitimately vary per tool;
the gate compares shape, not prose.)

Two consequences for callers:

- **`session` is accepted on every action and cursor tool, on all three
  platforms.** It's cursor-wired where the platform glides a cursor and
  schema-accepted everywhere else — so the same `session` you pass on
  macOS is no longer _rejected_ by Windows/Linux, which previously
  refused unknown keys via `additionalProperties:false`.
- **`delivery_mode` (`"background"` default / `"foreground"`) is on the
  whole input family** — `click`, `double_click`, `right_click`, `drag`,
  `scroll`, `type_text`, `press_key`, `hotkey` — uniformly. The
  `foreground` rung briefly fronts the target, acts, then restores the
  prior frontmost: the explicit last resort when a background attempt
  didn't land. **`foreground` is a reaction and a user-authorization gate,
  never a prediction.** Always
  fire the `background` default first and let the driver tell you it
  can't (a `background_unavailable` error with
  `escalation.recommended == "foreground"`, or a successful action result
  with `escalation.target == "foreground"`) — or observe a confirmed no-op —
  _before_ you consider escalation. Then use it only when the user already
  authorized visible foreground control or after asking for approval.
  Do **not** reason "it's a GTK/Chromium/Electron app, so background will
  drop, so I'll front up-front": the toolkit lists in the tool schemas
  are the _driver's_ internal detectors, not a checklist for you to front
  on a guess. (Concretely: GIMP's GTK toolbox accepts background pixel
  clicks fine — a preemptive foreground click there just steals the
  user's focus for nothing.) What each platform's _background_ rung can
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
macOS-only status-only `check_permissions.prompt`). The per-OS files list the
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

For Chromium-family browsers and Electron, use the exact, session-scoped
browser capability workflow in **`BROWSER.md`**. It keeps native
`(pid, window_id)` selection as the entry point, makes setup explicit through
`browser_prepare`, and distinguishes trusted browser input from an explicitly
requested synthetic DOM event.

Use the native `get_window_state` and AX/PX action ladder for browser chrome,
permission prompts, downloads, file pickers, Safari, Firefox, Tauri, and any
embedded webview for which exact browser binding is unavailable. The legacy
`page` tool remains a compatibility surface; do not use it as the starting
point for new browser workflows.

## Verify after every action — mandatory

**Always** verify after an action. Prefer
`verify_state({pid, window_id, expect})` for structured state such as a
window's existence/bounds or a semantic element's existence, value, enabled
state, or selected state. Use its bounded poll and stable-sample requirement
instead of hand-written sleeps. `unknown` means the driver could not establish
the predicate; it is not success. Once a session has effective desktop scope,
use a fresh `get_desktop_state(session)` result instead—window-scoped
`verify_state` is denied by that capture policy.

Pass `include_screenshot:true` when visual evidence is useful. The same result
then contains a fresh final window image. The driver still evaluates only the
structured predicates; the multimodal agent harness reads the pixels and
decides whether to stop, retry, or advance the ladder. For a postcondition not
expressible by the tool, explicitly take a fresh `get_window_state` snapshot
and have the harness judge its tree and image.

Switch to an **element px action** only on a real signal: the action
response carried `effect:"suspected_noop"`, verification returned
`unsatisfied`/`unknown`, the snapshot came back `degraded` (empty tree →
non-AX surface), the tree looks unchanged/unreadable or disagrees with the screenshot, or
`escalation.target` points you there (`pixel`). That's the
verify-then-escalate ladder in the behavior-matrix section. If the tree
is unchanged AND the screenshot confirms nothing moved, the action
likely failed silently — **tell the user what you attempted and what
you observed**, don't paper over with "done" language (and consider
`delivery_mode:"foreground"` when `escalation.target ==
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

| Error text                                                                         | Meaning                                                                                                                                                                          | Fix                                                                                                                                                                                                                  |
| ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `No cached AX state for pid X window_id W`                                         | You either skipped `get_window_state` this turn, or passed a different `window_id` to the click than the one the snapshot cached against                                         | Call `get_window_state({pid: X, window_id: W})` first — the same window_id you intend to click in                                                                                                                    |
| `snapshot_id_required` / `stale_element_token`                                    | A bare index was supplied, or a newer snapshot superseded this target                                                                                                            | Re-run `get_window_state`; use the new `element_token`, or send its `snapshot_id` with the matching integer                                                                                                          |
| `window_id W belongs to pid P, not …`                                              | Passed a window_id that's owned by a different process                                                                                                                           | Use `list_windows({pid: X})` to enumerate this pid's own windows                                                                                                                                                     |
| `ambiguous_window_target`                                                          | A PID-only window action matched multiple eligible top-level windows                                                                                                              | Use the returned candidates or `list_windows({pid: X})`, select the intended sibling, and retry with its explicit `window_id`                                                                                       |
| `AX action … failed with code …` / `UIA invoke failed`                             | Element doesn't support the default action                                                                                                                                       | Try `show_menu`, `confirm`, `cancel`, `pick`, or fall through to a pixel click on the element's center                                                                                                               |
| `The user doesn't want to proceed with this tool use. The tool use was rejected …` | The harness uses this _exact_ string for BOTH a permission-prompt denial AND a manual interrupt (Esc / stop) of a running tool — they are indistinguishable from the tool result | Treat as "tool canceled, no result, await the user." Do NOT paraphrase ("you stopped me") — quote the literal message and name the canceled tool + its args, so the user can tell what was in flight vs. what landed |

Platform-specific errors (TCC dialogs on macOS, Session 0 / UAC
prompts on Windows, AT-SPI bus issues on Linux) live in their
respective companion files.

## Things to avoid

- **Never** reuse an element target across a re-snapshot of the same window.
  A new snapshot invalidates older tokens immediately. Bare `element_index`
  input is rejected; use `element_token` or `element_index` + `snapshot_id`.
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
