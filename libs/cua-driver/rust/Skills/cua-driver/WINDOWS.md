---
name: cua-driver-rs-windows
description: Drive a native Windows app (Win32, UWP, WebView2/Edge) via the cua-driver CLI or MCP server — snapshot its UIA tree, click/type/scroll by element_index or window-client pixels, verify via re-snapshot, all without bringing the target to the foreground. Use when the user asks you to operate, drive, automate, or perform a GUI task in a real Windows application on the host.
---

# cua-driver-rs — Windows

Orchestrates Windows app automation via the `cua-driver` Rust binary
(`cua-driver-rs` repo, ships as `cua-driver.exe`). Whenever a user
asks to drive a native Windows app, follow the loop in this doc
rather than calling tools ad-hoc — the snapshot-before-action
invariant is not optional and silently breaks if you skip it.

`SKILL.md` in this directory describes the macOS-flavored core
patterns; this file is the Windows-specific carve-out. Read both:
the snapshot invariant, MCP-vs-CLI choice, agent cursor overlay, and
recording flow are identical. The launch, click, and accessibility-
tree mechanics in this file replace the macOS ones.

## The no-foreground contract — read this first

**The user's frontmost app MUST NOT change.** Users pay for the right
to keep typing in their editor while an agent drives another app in
the background. Violate this rule and every other nice property the
driver gives you (no cursor warp, no taskbar flash, no window
restore-and-raise) stops mattering — you just shipped a `SendInput`
wrapper with extra steps.

### Cursor feedback for JS-driven browser actions: prefer `click_element`

The `page` tool gained a fourth action: `click_element`. It takes a CSS
selector, animates the agent cursor to the element's on-screen center,
fires a click-pulse, then runs `el.click()`. Prefer this over
`execute_javascript('document.querySelector(...).click()')` whenever you
want the user to see what the agent is doing — raw `execute_javascript`
will perform the click but leaves the cursor frozen.

```json
{ "action": "click_element", "pid": <int>, "window_id": <int>, "selector": "button.submit" }
```

Returns the resolved screen coords in `structuredContent` so callers can
chain subsequent operations against the same point.

### How the contract is enforced per call: the `delivery_mode` field

Every Windows input tool (`click`, `double_click`, `right_click`,
`drag`, `scroll`, `press_key`, `hotkey`, `type_text`) accepts an
optional `delivery_mode` field — this mirrors the macOS `delivery_mode`
surface (same name, same two values). The default is `"background"` —
strict no-foreground:

| `delivery_mode` | Behavior on Windows |
|---|---|
| `"background"` (DEFAULT) | Never fronts and **never raises/restacks** the target — macOS-aligned (mirrors CGEvent-to-pid). **Pixel clicks**: a UIA hit-test at the point first (accessibility-channel Invoke — works on UWP / WinUI3 / Win11 packaged apps, no flash); if that misses, coordinate-injected pen/touch, **but only when the target is the *visible* window at that point**; PostMessage for plain Win32. It returns a structured `background_unavailable` error — rather than raising or fronting — when the target is **occluded** at the point, or the event kind is known-dropped (Chromium DOM mouse + key-combos, GTK buttons, VCL/LibreOffice accelerators, terminal / WPF text with no `element_index`). **No foreground swap and no z-order raise, ever.** |
| `"foreground"` | SendInput with brief `SetForegroundWindow(target)` → restore. The explicit, agent-chosen rung where fronting IS allowed — required to reach occluded targets, Chromium DOM content, GTK buttons, VCL accelerators, WPF drag, terminals, and canvas / custom-drawn surfaces with no UIA peer. Implemented for **every** input tool — `type_text` (SendInput Unicode via `send_text_synthesized`) and `scroll` (SendInput wheel via `send_wheel_synthesized`) included. Flashes the target visible unless `bring_to_front` was called first. |

> **macOS is the source of truth — `background` never alters the screen.**
> Earlier Windows builds "cheated" in background with three tricks that this
> pass **removed**: (1) a z-order raise (`ZorderGuard`) to win the pointer
> hit-test on occluded windows, (2) a full focus-activate for WPF drags, and
> (3) a *cloaked* (hidden) focus-grab for keystrokes/text the target would
> otherwise drop. macOS does none of these (pure CGEvent-to-pid + focus
> suppression), so Windows now does none either: when strict no-front /
> no-raise delivery can't land, the tool returns `background_unavailable` and
> **the agent — not the driver — decides** whether to escalate to
> `delivery_mode:"foreground"` (the rung where fronting is explicitly opted
> into). A `background` call will never raise, restack, flash, or steal focus.

> **Removed: the legacy `"auto"` mode.** Earlier builds had a third
> Windows-only `dispatch:"auto"` mode (silent SendInput fallback on
> known-problematic targets). It was removed in the macOS-alignment pass
> because it could front the target *without the caller opting in* —
> breaking the no-foreground contract macOS guarantees. Any unrecognised
> value (including a stray `"auto"`) now resolves to `"background"`. If you
> have notes/snippets that pass `dispatch:"auto"`, switch to an explicit
> `delivery_mode:"foreground"` for the cases that need fronting.

### Always try `delivery_mode:"background"` first

The hit-test fallback above means **`delivery_mode:"background"` is the correct
default even when the target is a XAML host whose input stack drops raw
PostMessage** (UWP Calculator, Win11 Notepad, WinUI3 apps, etc.). cua-driver
turns the pixel coord into a UIA Invoke at that point and delivers
through the accessibility channel — no flash, no focus steal. Only
escalate to `delivery_mode:"foreground"` when you actually see a
`background_unavailable` structured error, and only with the
`bring_to_front` flow described below so the agent pays the flash cost
once instead of per-call.

Empirical: pixel-clicks via `delivery_mode:"background"` against the UWP
Calculator on Win11 (Number-pad buttons + operators) consistently
resolve through `try_invoke_in_window_at_point` and produce
`"✅ Performed UIA Invoke at (sx,sy) for pid X."` with zero visible
flash. `delivery_mode:"foreground"` on the same coords also works but
flashes the Calculator window foreground for ~40 ms — it's the
costlier path; only use it for surfaces with no UIA peer.

`background_unavailable` error shape:

```json
{
  "isError": true,
  "structuredContent": {
    "code": "background_unavailable",
    "target_class": "Chrome_WidgetWin_1",
    "event_kind": "mouse_click",
    "escalation": { "recommended": "foreground", "reason": "occluded / known-dropped event kind" },
    "suggestion": "Either call bring_to_front then retry with delivery_mode:\"foreground\", or accept the foreground swap by setting delivery_mode:\"foreground\" directly."
  }
}
```

The `escalation` field is the same machine-readable hint the action
responses carry (see `SKILL.md` → behavior matrix). On Windows the
recommendation is `"foreground"` — the dropped event needs the fronting
rung. (Contrast macOS / X11, where a background px click can still land
in the background, so there the hint is `px`.)

The recommended flow when an agent gets that error:

1. `bring_to_front(pid)` — activates the target ONCE (visible flicker).
2. Subsequent input calls with `delivery_mode:"foreground"` deliver via
   SendInput WITHOUT a per-call flash (the SetForegroundWindow swap
   inside SendInput is a no-op because the target is already frontmost).
3. When done, leave the target as the user's foreground or call
   `hotkey({pid: original_fg_pid, keys: ["alt","tab"]})` to put their
   prior window back. **There is no "restore" tool** — you brought
   the target forward deliberately; restoring is your responsibility.

The `bring_to_front` tool uses an `AttachThreadInput` trick to *attempt*
the foreground swap even when the daemon isn't at UIAccess integrity (the
same trick that powers `send_key_synthesized`). Returns
`{previous_fg_hwnd, now_fg_hwnd, landed_on_target}` — **check
`landed_on_target`**. Without UIAccess, Windows' foreground-lock can still
reject the swap (and a subsequent `delivery_mode:"foreground"` call will
bail with the "Foreground swap … was rejected by Windows" diagnostic
rather than landing input on the wrong window). When that happens the
target genuinely cannot be driven by SendInput/keystrokes in this session:
spawn the `cua-driver-uia` worker (UIAccess-manifested PE), or — for tasks
that produce a file — generate the document and `launch_app` it instead of
driving the GUI (e.g. building a spreadsheet and opening it in LibreOffice
Calc rather than typing into the grid, which is dropped on the VCL
background path).


Before running any shell command, ask: **"does this raise, activate,
foreground, or steal focus from any app?"** If yes, don't run it.
Every one of the commands below activates the target on Windows and
is therefore forbidden unless the user **explicitly** asked for
frontmost state:

- **`Start-Process <exe>` / `Start-Process <url>` / `Start-Process
  -FilePath ...`** — defaults to launching with `SW_SHOWNORMAL` which
  *activates* the new window. Windows treats new processes as
  user-initiated foreground apps. The CmdLine flag `-WindowStyle Hidden`
  helps but does not block activation for apps that call
  `SetForegroundWindow` themselves on startup (Edge, most browsers,
  Office, most installers). **Never use `Start-Process` to launch a
  visible app.** Use `launch_app` — it goes through
  `SW_SHOWNOACTIVATE` and wraps an internal `VK_NONAME keybd_event`
  trick that the OS treats as "user activity from another window," so
  the target's own `SetForegroundWindow` call is denied per Windows
  focus-stealing prevention rules.
- **`& "C:\path\to\app.exe"` from PowerShell** — same as
  `Start-Process` on activation. PowerShell's `&` invocation operator
  spawns the process with foreground intent.
- **`cmd /c start <thing>` and `start /b <thing>`** — `start` on
  Windows is the equivalent of macOS's `open`: it goes through the
  shell's protocol-handler / file-association lookup and activates
  the resulting process. `start /min` helps for taskbar minimization
  but still activates the new window before minimizing it (flash
  visible to the user). Forbidden for the same reason.
- **`explorer.exe shell:AppsFolder\<AUMID>` / `explorer.exe ms-edge:
  <url>`** — these are the Windows-shell equivalents of `open -a` /
  `open <url>` on macOS. They go through `IApplicationActivationManager`
  with the wrong activation kind and foreground the target. Use
  `launch_app({aumid})` or `launch_app({urls})` instead — those route
  through `IApplicationActivationManager::ActivateApplication` with
  the correct flag combination that respects `SW_SHOWNOACTIVATE`.
- **`SetForegroundWindow(hwnd)` / `BringWindowToTop(hwnd)` /
  `SwitchToThisWindow(hwnd, fAltTab)` / `ShowWindow(hwnd, SW_SHOW)` /
  `SetActiveWindow(hwnd)`** — all foreground the named window by
  definition. Never call these from Bash via PowerShell add-types or
  C# inline. If you find yourself doing this to "make a click land,"
  the click is already wrong: re-read "Click semantics" below.
- **`AttachThreadInput` + `SetForegroundWindow` trickery** — the
  classic Windows focus-bypass hack. Even when it works it's a
  visible focus pop and a UIPI/UIAccess violation. Forbidden.
- **`SendInput(MOUSEEVENTF_*)` over the user's real cursor** — moves
  the cursor and synthesizes input. The cursor warps to coordinates,
  any handler in the topmost window at those coordinates fires, and
  for most apps the receiving window activates because input arrived
  from the OS-trusted pipeline. Use `click({pid, x, y})` or
  `click({pid, element_index})` instead — both route per-pid and
  never touch the OS cursor.
- **`SendInput(KEYBDINPUT)` with no target HWND** — same idea: goes
  to the focused window, not your target. Use `hotkey({pid, keys:
  [...]})` which uses `PostMessage(WM_KEYDOWN/UP)` to the named pid's
  focused window.
- **Keyboard shortcuts that semantically mean "focus here" —
  Chromium / Edge / Firefox `Ctrl+L` (focus address bar),
  Explorer's `Ctrl+L` / `Alt+D` (focus path bar), `F6` (focus
  cycle).** These aren't pure key events — the receiving app
  interprets "user wants to type here" as activation intent and
  raises its window to be key. Even when delivered to a backgrounded
  pid via `hotkey`, the downstream app pulls focus. **For omnibox
  navigation specifically**, the correct path is `launch_app({path:
  "...msedge.exe", urls: ["https://…"]})` (or `{aumid:
  "Microsoft.MicrosoftEdge.Stable_…!App", urls: [...]}`) — no
  omnibox dance, no `Ctrl+L`, no focus-steal. The browser opens the
  URL in a new window without activating it.
- **Tab-switching shortcuts in browsers (`Ctrl+1..9`, `Ctrl+Tab`,
  `Ctrl+Shift+Tab`, `Ctrl+PageUp/Down`)** are visibly disruptive
  even when delivered to a backgrounded pid. The app's key handler
  processes the shortcut, the window re-renders the new tab's
  content, the user sees their tabs flipping. Same as macOS:
  there is no UIA-only workaround — page content (HTML, DOM,
  WebView2's accessible tree) populates only for the focused tab;
  inspecting a background tab requires activating it, which is the
  visible flip.

  **Prefer the windows-over-tabs pattern**: for each URL you need to
  drive backgrounded, use `launch_app({urls: [url]})` — Chromium-
  family browsers open each URL in a new **window**. Each window has
  its own `window_id`, its own UIA tree, and can be inspected /
  interacted with via `element_index` without activating or switching
  anything. Tabs are a UX grouping for humans; cua-driver-rs
  workflows should default to windows.
- **Win+key shortcuts owned by the shell** — `Win+E` (Explorer),
  `Win+R` (Run), `Win+S` / `Win+Q` (Search), `Win+number` (taskbar
  pinned-app activation), `Win+Tab` (Task View), `Alt+Tab` (window
  switcher), `Win+D` (show desktop), `Win+L` (lock), `Win+M` /
  `Win+Up/Down` (minimize / maximize). All hard-coded to the shell
  and visibly disruptive — they bypass any per-pid routing. Forbidden
  in `hotkey` calls regardless of target pid.
- **`taskkill /F /IM <exe>` to close an app the user is using** —
  not a focus-steal but a data-loss vector. Use
  `hotkey({pid, keys:["alt","f4"]})` and let the app close cleanly,
  or ask the user first.

Reading state is fine. Listing windows, reading registry, querying
process info via `Get-Process`, calling `tasklist`, walking UIA trees
via `cua-driver get_window_state` — none of these change focus.
**Mutating state via shell shims is the line.**

**Corollary — the Win+Search rule.** Don't use Win+S/Win+Q "open Start
search, type query, press Enter" patterns to launch apps. They (a)
foreground the Search UI, (b) leave the Search UI populated with the
agent's typed text, (c) trigger the new app via the Start Menu's own
activation path which `SW_SHOWNORMAL`s it. Use `launch_app({name})` /
`{aumid}` / `{path}` instead.

**"Open \<app\>" in user speech means launch, not activate.**
`cua-driver launch_app` is the one correct path for process startup —
it's idempotent (no-op on a running app), returns the pid, and
internally uses `SW_SHOWNOACTIVATE` plus the AppX-broker activation
flow for packaged apps so the target's window comes up without
becoming foreground. The macOS `FocusRestoreGuard` is replaced on
Windows by the activation-deny dance: the launcher pumps a
`VK_NONAME` keystroke before activating, which Windows interprets as
"another app just had user activity" and rejects the target's own
`SetForegroundWindow` call per [Windows focus-stealing prevention
rules](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow).

**`launch_app` foreground-restore behaviour (Windows).** In addition to
`SW_SHOWNOACTIVATE`, `launch_app` captures `GetForegroundWindow()` before
the spawn and schedules a post-spawn polling restore (every 100 ms for up
to 3 s) that flips the foreground back to the prior window once the
spawned app has actually activated. This mirrors the macOS
`FocusRestoreGuard`. For `urls`-only invocations (open these links in the
default browser, no app-identifying field) the restore is **skipped**,
because the user explicitly asked for the page to come up. The restore is
best-effort: `SetForegroundWindow` from non-UIAccess processes is subject
to Windows' foreground-lock and may silently no-op — failures are logged
at `tracing::trace!` and not surfaced as errors.

**`hotkey` modifier dispatch on Chromium/Electron (Windows).** When
the target is detected as a Chromium-family window (`Chrome_WidgetWin_*`
class — covers every Chromium browser AND every Electron app: Chrome,
Edge, Brave, Arc, Vivaldi, Slack, VS Code, Discord, Teams, Notion),
`hotkey` with modifiers routes through `PostMessage(WM_KEYDOWN/UP)`
instead of `SendInput` — no foreground swap. Chromium's
`Browser::HandleKeyboardEvent` reads modifier state from the WM_KEYDOWN
LPARAM bits, NOT from `GetKeyState`, so PostMessage delivery works for
real accelerators (Ctrl+T, Ctrl+W, Ctrl+L, Ctrl+Shift+B, etc.). The
SendInput-swap path (`send_key_synthesized`) remains the dispatch for
**non-Chromium Win32 + modifiers** — classic apps (LibreOffice, FAR,
classic Notepad) use `TranslateAccelerator` which requires the system
modifier state updated, and PostMessage can't do that.

**`modifier` on a *background* click is a Windows residual.** A
backgrounded click delivers through UIA `Invoke` or `PostMessage`, and
neither carries live keyboard state — so a `modifier` (Ctrl/Shift/etc.)
passed alongside a `delivery_mode:"background"` click **is not honored**
on Windows. The `modifier` *param* is part of the shared schema and is
accepted everywhere; it only takes effect on the SendInput rung, i.e. a
`delivery_mode:"foreground"` (or `bring_to_front`-then-foreground) click,
where SendInput sets real modifier state. If you need a modifier-click on
Windows, escalate that one action to `foreground`.

### Cross-platform schema residuals (Windows)

The capture/dispatch/addressing params are a shared cross-platform
contract (see `SKILL.md` → *Cross-platform parameter contract*). Three
Windows-relevant notes:

- **`session` is now accepted on every action/cursor tool.** Earlier
  Windows builds rejected `session` via `additionalProperties:false` (it
  was effectively a macOS-only key); the shared contract makes it
  uniformly schema-accepted — cursor-wired where a cursor glides,
  accepted elsewhere.
- **`debug_window_info` is a Windows-only tool** (window-handle / class /
  rect / z-order diagnostics for triaging the click chain). It is
  deliberately not part of the cross-platform surface — there is no macOS
  or Linux counterpart.
- **`launch_app` identifiers are platform-specific.** Windows takes
  `aumid` / `launch_path` / `path` / `start_minimized` (plus `bundle_id`
  overloaded for AUMIDs); macOS takes `bundle_id` / `urls`. `name` is the
  portable fallback. See the AUMID section below.

**Chromium pixel-click foreground polling restore.** `click({pid, x, y})`
on a Chromium target falls through to `send_click_synthesized` (SendInput
+ brief foreground swap) because Chromium's input thread filters by
queue-origin and PostMessage-delivered clicks don't fire DOM events. The
synchronous restore inside `send_click_synthesized` covers the
immediate swap; an additional polling guard (same shape as `launch_app`'s
`FocusRestoreGuard`) catches the **asynchronous** Chromium re-activation
that can happen as the renderer's input handler processes the click
(focus().activate() / WebContents::Activate() — 100-500 ms later). The
guard is gated on `GetWindowThreadProcessId(fg_now) == pid` so user
Alt-Tabs are respected.

## Defaults — always prefer cua-driver over shell shims

**Default transport is the `cua-driver` CLI** — `Bash` shelling out
to `cua-driver <tool-name>` with JSON piped via stdin (avoids
PowerShell 5.1's argv quoting quirks for strings containing both
quotes and spaces). MCP tools (prefix `mcp__cua-driver__*`) only when
the user explicitly asks for them. CLI wins because it picks up
rebuilds instantly, failures are easier to diagnose, and there's no
per-tool schema-load overhead.

Every reference to `click(...)`, `get_window_state(...)` etc. in this
doc means `cua-driver <name>` with JSON piped via stdin — translate
to MCP form only when MCP is requested.

### CLI argument plumbing on Windows

Three equivalent shapes for passing JSON to `cua-driver <tool>`:

1. **Stdin pipe (recommended)** — avoids PS quoting bugs entirely:
   ```powershell
   '{"pid":1234,"text":"hello world"}' | & cua-driver call type_text
   ```
2. **Positional with escaped quotes** — works for JSON without spaces
   in string values:
   ```powershell
   & cua-driver call list_windows '{\"app_name\":\"Calculator\"}'
   ```
   (Windows PowerShell 5.1 mangles `{"x":"with space"}` when both `"`
   and ` ` appear unquoted in argv. Use stdin for those.)
3. **`--%` stop-parser directive** (PowerShell 5.1 specific):
   ```powershell
   & cua-driver call type_text --% {"pid":1234,"text":"hello world"}
   ```

Stdin is the only path immune to all PS quoting edge cases. Prefer it.

### Intent → tool mapping

If you find yourself reaching for the right column, something has
gone wrong — re-read "The no-foreground contract" above.

| Intent | Use | Don't use |
|---|---|---|
| Open / launch a Win32 app | `launch_app({path: "C:\\Program Files\\…\\foo.exe"})` or `{name: "foo"}` | `Start-Process`, `cmd /c start`, `& "C:\\path\\foo.exe"` |
| Open / launch a UWP / packaged app | `launch_app({aumid: "Microsoft.Foo_8wekyb3d8bbwe!App"})` | `explorer.exe shell:AppsFolder\\<AUMID>`, Start Menu typing |
| Open a URL in the default browser | `launch_app({urls: ["https://example.com"]})` | `Start-Process "https://…"`, `explorer.exe ms-edge:…`, `cmd /c start "" "https://…"` |
| Find a pid | `list_apps` or `launch_app`'s return | `Get-Process`, `tasklist`, Win+S typing |
| Enumerate an app's windows | `list_windows({pid})` — or read the `windows` array `launch_app` already returns | `Get-Process \| Where-Object { $_.MainWindowHandle }` |
| Click / type / scroll / keys | `click`, `type_text`, `scroll`, `press_key`, `hotkey` | `SendInput`, `cliclick`-style C# add-types, AutoHotkey scripts |
| Drag / drag-and-drop | `drag({pid, from_x, from_y, to_x, to_y})` | `SendInput` with `MOUSEEVENTF_MOVE`, mouse_event |
| Screenshot | `screenshot` or the PNG in `get_window_state` | `[System.Windows.Forms.Screen]::CopyFromScreen`, `nircmd savescreenshot` |
| Quit an app | ask the user first, then `hotkey({pid, keys:["alt","f4"]})` | `taskkill /F`, `Stop-Process -Force`, `Get-Process \| Stop-Process` |
| Hand a file/URL to an app | `launch_app({urls:[<path>]})` (default app) or `{path: "...exe", args:[<file>]}` (specific app) | `& "app.exe" "file"`, `Invoke-Item`, shell associations |

### The narrow carve-out

The **only** legitimate use of `SetForegroundWindow` or
`Start-Process` with a foreground app is when the user **explicitly**
asked for frontmost state ("bring Edge to the front", "make
Calculator visible", "I want to see it"). Reaching for it because a
tool call returned something confusing is wrong — diagnose first.

When a cua-driver call surprises you, diagnose cua-driver first:

- **`Posted click to pid X` instead of `Performed UIA Invoke ...`?**
  The (x,y) UIA hit-test didn't find an `InvokePattern`-bearing
  element at that pixel inside the target HWND, so it fell through
  to `PostMessage(WM_LBUTTONDOWN)`. For UWP / WebView2 surfaces,
  PostMessage silently no-ops — re-snapshot via
  `get_window_state(pid, window_id)` and use `element_index` so the
  daemon can invoke the cached UIA element by identity instead of by
  point.
- **`Invalid element_index` / `No cached UIA state`?** You either
  skipped `get_window_state` this turn or passed a different
  `window_id` than the one the snapshot cached against. The cache is
  keyed on `(pid, window_id)` — indices don't carry across windows of
  the same app. Re-snapshot with the same window_id you're about to
  click in.
- **`Invalid window handle (0x80070578)`?** The HWND you passed is
  stale (window closed, recreated). Re-resolve via `list_windows`.
- **Empty `tree_markdown` / sparse UIA tree?** Some apps populate
  their UIA tree lazily on first call; retry `get_window_state`
  once. If still empty, the app has no UIA provider — fall back to an
  element px action (x,y clicks off the screenshot) on visible content
  (acceptable for exploration; pair with screenshots).
- **Empty tree, or a snapshot with no image?** `get_window_state`
  returns **both** the UIA tree and a screenshot by default — there is no
  capture mode to pick. If the tree came back empty, the response is
  `degraded` (no UIA provider — retry once for lazy trees, see the note
  above); act by **px** off the screenshot in the same response. The
  `capture_mode` param is **deprecated and ignored** — it's still accepted
  so old callers don't error, but both the tree and the image come back
  regardless of what you pass.
- **`get_desktop_state` returns `desktop_scope_disabled`?** That's
  intended: full-display capture is a **desktop-scope** operation, gated
  on the global `capture_scope`. It's `"window"` by default — so to verify
  a specific window use `get_window_state(pid, window_id)` (works
  backgrounded), and only use `get_desktop_state` after
  `set_config capture_scope=desktop` (the same opt-in that enables
  window-less screen-absolute `click`/`scroll`). Don't reach for
  `get_desktop_state` as a casual screenshot — it's the capture surface for
  desktop-scope coordinate loops, not window inspection.
- **`Calc display stuck at 0 after my clicks`?** Almost always
  means UWP and you're on the PostMessage path. UWP processes
  pointer input via `Windows.UI.Input`, NOT through HWND message
  queues — PostMessage(WM_LBUTTONDOWN) gets ignored. Use
  `element_index` instead of (x,y) for UWP targets.

Only after those are ruled out should you fall through to the
activate fallback. Always name the focus steal in your response
("I'll briefly bring Edge to the front because …").

### Self-check pattern

Before every `Bash` call whose command line touches any Windows app
(launching, opening, clicking, typing, scripting, screenshotting),
run the self-check:

1. **Does this command foreground the target?** If yes — stop and
   translate to the cua-driver equivalent from the mapping table.
2. **Does this command move the user's real cursor?** (`SendInput`,
   `SetCursorPos` from inline C#, AutoHotkey scripts, `nircmd
   sendmouse`.) If yes — stop; use `click({pid, x, y})` which routes
   per-HWND via PostMessage / per-element via UIA Invoke and never
   warps the cursor.
3. **Does this command bypass cua-driver entirely?** (PowerShell
   GUI scripts, AutoHotkey, `SendKeys`, AppleScript-equivalent
   automation tools.) If yes — stop; find the cua-driver tool that
   does the intent.

If all three are "no," the command is safe. If you can't answer,
default to stop and ask rather than proceed. A single `Start-Process`
run by accident steals the user's foreground and kills the trust
your prior tool calls earned.

## Prerequisites — check before starting

1. **`cua-driver` is on `$PATH`** — `Get-Command cua-driver` or
   `where.exe cua-driver`. Install location:
   `%LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin\cua-driver.exe`,
   added to the user PATH by the install script.
   If missing, point the user at:
   ```powershell
   irm https://cua.ai/driver/install.ps1 | iex
   ```
   and stop.
2. **The daemon must run in an interactive session (Session 1+),
   NOT Session 0.** Windows isolates services into Session 0 with no
   desktop. UIA enumeration, screenshot via PrintWindow, and
   `IApplicationActivationManager` all silently return empty /
   timeout in Session 0. Check:
   ```powershell
   Get-Process cua-driver | Select Id,SessionId
   ```
   `SessionId == 0` is broken. The autostart Scheduled Task uses
   `LogonType=Interactive` so the daemon lands in the user's logon
   session. If you started the daemon via SSH-into-Windows, that
   session is usually Session 0 — kick the autostart task instead:
   ```powershell
   schtasks /Run /TN cua-driver-serve
   ```
3. **Run `cua-driver doctor`** — reports session ID, COM apartment
   status, UIA reachability, install paths, version. If anything reads
   `false` / `error`, fix that before tool-calling.
4. **Permissions** — Windows has no TCC equivalent. cua-driver-rs
   needs:
   - No admin elevation for normal use (UIA, PostMessage, UWP
     activation all work from a standard user token).
   - UAC elevation **only** for `autostart` registration if a
     system-wide scheduled task is requested (per-user task is
     non-elevated and the default).
   - SmartScreen: on a fresh install, Windows Defender SmartScreen
     may flag the unsigned binary on first run. Click "More info →
     Run anyway" once.

## Using cua-driver from the shell

Tool names are `snake_case`, management subcommands are
`kebab-case` — no ambiguity. Tools invoked as `cua-driver call
<tool-name>` with JSON via stdin or positional arg. Management
subcommands:

- **`cua-driver serve`** — start persistent daemon (**required** for
  `element_index` workflows; without it each CLI invocation spawns a
  fresh process and the per-pid element cache dies between calls).
  Normally not run manually — the autostart Scheduled Task fires it
  at every interactive logon. If you stopped it (`Stop-Process`),
  re-run with `schtasks /Run /TN cua-driver-serve`, not by spawning
  `cua-driver serve` from SSH (Session 0 problem).
- **`cua-driver stop`** / **`status`** — daemon lifecycle.
- **`cua-driver doctor`** — full diagnostics.
- **`cua-driver list-tools`** / **`describe <tool>`** — tool surface
  discovery.
- **`cua-driver autostart {enable|disable|status|kick}`** — manage the
  Scheduled Task that auto-starts the daemon at logon. `enable`
  registers it (idempotent — replaces existing). `kick` runs it
  immediately without waiting for a fresh logon.
- **`cua-driver recording start|stop|status`** — see `RECORDING.md`.
  **Note: recording is currently macOS-only on the Rust port. The
  command is registered but returns "not yet supported" on Windows.**

Canonical multi-step workflow:

```powershell
# Daemon is already running via Scheduled Task.
# Launch UWP Calculator without focus-stealing.
'{"aumid":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"}' | & cua-driver call launch_app
# → {pid: 6004, windows: [{window_id: 459672, ...}]}

# Snapshot the UIA tree.
'{"pid":6004,"window_id":459672}' | & cua-driver call get_window_state
# Returns: tree_markdown with [N] element indices, screenshot, dimensions.

# Click element [22] (the "Equals" button per the tree).
'{"pid":6004,"window_id":459672,"element_index":22}' | & cua-driver call click
# → "✅ Performed UIA Invoke on [22] ..."

# Re-snapshot to verify the action landed.
'{"pid":6004,"window_id":459672}' | & cua-driver call get_window_state
```

## The core invariant — snapshot before AND after every action

**Every action MUST be bracketed by `get_window_state(pid, window_id)`**:

- **Before** — the pre-action snapshot resolves the `element_index`
  you're about to use. Indices from previous turns are stale; the
  server replaces the element index map on every snapshot, keyed
  on `(pid, window_id)`. Indices from turn N don't resolve in turn
  N+1, and indices from window A don't resolve against window B of
  the same app. Skip this and element-indexed actions fail with
  `Invalid element_index`.
- **After** — the post-action snapshot verifies the action actually
  landed. Without it you can't tell a silent no-op from a real
  effect. The UIA tree change (new value, new window, disappeared
  menu, disabled button, etc.) is your evidence that the action
  registered. **Especially important on Windows** because the
  layered click path can return "✅ Posted click to pid X" even when
  the click did nothing (UWP target, PostMessage silently no-ops):
  the success message reports the mechanism, not the outcome. Only
  the re-snapshot tells you if the state changed.

## Click semantics on Windows

Two click addressing modes, both gated by `pid`:

### `element_index` mode (preferred)

```json
{"pid": 6004, "window_id": 459672, "element_index": 22}
```

Looks up the cached UIA element from the last `get_window_state`,
fires `IUIAutomationInvokePattern::Invoke()` on it directly.

Properties:
- **No mouse cursor moves.** The click is a UIA RPC, not an input
  event. The user's cursor stays where it is.
- **No window activates.** UIA Invoke does not foreground the
  target.
- **Z-order is irrelevant.** Works on backgrounded, occluded,
  minimized-but-not-iconic, and cross-virtual-desktop windows.
- **Cross-process boundaries work.** UIA marshals across the
  `ApplicationFrameHost.exe` → inner UWP process boundary
  (CalculatorApp.exe, Notepad-Win11.exe, etc.) — this is how it can
  click Win11 Calc buttons even though they live in a different
  process from the AppFrame HWND.
- **Falls back to `PostMessage(WM_LBUTTONDOWN/UP)` to the deepest
  child HWND** when the cached element doesn't expose
  `InvokePattern` (most edit fields, custom-drawn widgets,
  non-actionable elements). The fallback works for plain Win32 but
  silently no-ops on UWP. The success message tells you which path
  ran: `"✅ Performed UIA Invoke on [N] ..."` vs `"✅ Performed
  PostMessage click on [N] ..."`.

This is the right path for **any** "click button N" / "click menu
item X" / "click checkbox Y" intent.

### `(x, y)` mode (element px action / pixel)

```json
{"pid": 6004, "window_id": 459672, "x": 446, "y": 671}
```

Window-client coordinates (origin at the top-left of the screenshot
the agent saw). The driver:

1. Converts to screen coords via `ClientToScreen(hwnd, ...)`.
2. **UIA hit-test in target HWND's subtree** — `ElementFromHandle`
   resolves the root, `FindAll(TreeScope_Subtree)` enumerates
   descendants, picks the smallest-area `InvokePattern`-bearing
   element whose `CurrentBoundingRectangle` contains the screen
   point, calls `Invoke()`. This is the only path that lands on UWP
   / WebView2 / DirectComposition surfaces.
3. **PostMessage fallback** — if step 2 returned false (no Invokable
   element under the pixel inside `hwnd`, or `hwnd` has no useful
   UIA tree at all), fires `PostMessage(WM_LBUTTONDOWN/UP)` against
   the deepest child HWND at the screen point. Covers plain Win32
   native controls.

Properties:
- **No real cursor movement.** The agent overlay glides + pulses
  for visual confirmation; the OS cursor is untouched.
- **No focus steal.** Both UIA Invoke and PostMessage are async per-
  pid / per-element; the target's window does not activate.
- **Z-order independent.** UIA hit-test honors the HWND-rooted
  subtree regardless of what's covering it on screen. PostMessage
  goes directly to the message queue.

Use this when the agent doesn't have a UIA snapshot in scope (zero-
shot from a screenshot), or when the element it wants doesn't appear
in the UIA tree (custom-drawn elements, canvas content, browser
DOM nodes inside a WebView2 viewport).

### What `(x, y)` mode does NOT solve

Apps with **no useful UIA tree** AND that **ignore `WM_LBUTTONDOWN`**
on the HWND queue — primarily DirectX / OpenGL / Vulkan-rendered
surfaces (games, custom renderers). The click chain falls all the
way through and the click no-ops. For those, the only options are:
- Bring the window to top first (focus steal — ask the user before
  doing this, and document why), then synthesize input
- Use the app's keyboard interface via `hotkey` if available

`SendInput` is **not** a silent-fallback option here — it would
steal focus from whatever the user is doing.

### Right-click and multi-click

`button: "right"` and `count > 1` **skip the UIA Invoke step** and
go directly through the PostMessage path. Reason: UIA has no clean
by-coord equivalent of `ShowContextMenu`, and `Invoke()` is single-
fire by definition. The success message will read
`"✅ Posted click/double-click/triple-click to pid X"` (PostMessage
path) regardless of the target's UWP-ness — this is expected and
**will not work for UWP context menus**. To open a UWP context menu,
prefer `hotkey({pid, keys: ["shift", "f10"]})` against the focused
UWP element.

## UWP / packaged apps — the AUMID layer

Modern Win11 apps (Calculator, Notepad, Settings, Photos, Edge UI
chrome, Microsoft Store) are **packaged apps** with an `App User
Model ID` (AUMID) rather than a plain `.exe` path. The AUMID looks
like `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` — package family
name + `!` + AppId.

Windows architectural quirks that matter:

1. **The `.exe` in `C:\Windows\System32\notepad.exe` is a 7 KB stub
   that exits immediately on Win11.** It exists for backward
   compatibility but the real Notepad lives in the
   `Microsoft.WindowsNotepad` AppX package. `Start-Process notepad`
   spawns the stub, which exits, which redirects through the AppX
   broker, which spawns the real process with a different pid. You
   end up holding a pid that's already gone. `launch_app` handles
   this transparently — it detects AUMID-looking strings and routes
   through `IApplicationActivationManager::ActivateApplication`,
   which returns the **real** packaged-process pid.
2. **UWP windows are hosted by `ApplicationFrameHost.exe`.** The
   outer top-level HWND (the one with the title bar, the one
   `EnumWindows` enumerates) is owned by `ApplicationFrameHost.exe`,
   pid varies, not the same as the UWP's own pid. The actual UWP
   content runs in a separate process (e.g. `CalculatorApp.exe`).
   `list_windows` reports the AppFrame's HWND because that's what
   `GetWindowRect`, `PostMessage`, `BitBlt` all target. UIA
   transparently crosses the boundary into the inner process when
   you walk the tree.
3. **AUMID resolution by name** — `launch_app({name: "calc"})` will
   first try a `shell:AppsFolder` lookup, matching against
   installed-package display names. On a hit it goes through the
   packaged-app path (real pid). Otherwise it falls back to
   `ShellExecuteEx`'s PATH search (which hits the stub `.exe`).
   **Prefer explicit `aumid` when you know it.**

### Known AUMIDs for common Win11 apps

```
Microsoft.WindowsCalculator_8wekyb3d8bbwe!App
Microsoft.WindowsNotepad_8wekyb3d8bbwe!App
Microsoft.MicrosoftEdge.Stable_8wekyb3d8bbwe!App
Microsoft.WindowsTerminal_8wekyb3d8bbwe!App
Microsoft.Paint_8wekyb3d8bbwe!App
Microsoft.WindowsCamera_8wekyb3d8bbwe!App
Microsoft.WindowsAlarms_8wekyb3d8bbwe!App
Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe!App
windows.immersivecontrolpanel_cw5n1h2txyewy!Microsoft.Windows.ImmersiveControlPanel  # Settings
```

To find an AUMID at runtime:

```powershell
Get-StartApps | Where-Object Name -like "*Calculator*"
```

## Web apps on Windows (Edge, Chrome, Firefox)

Browsers on Windows are mostly Win32 windows with browser-specific
chrome and a WebView2 / Chromium / Gecko surface inside. Click and
key handling rules:

### Launch

Use `launch_app` with `urls`:

```json
{"urls": ["https://example.com"]}
```

This opens the URL in the user's default browser, in a **new
window**, without activating it. For specific browsers:

```json
{"path": "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", "args": ["--new-window", "https://example.com"]}
```

or via AUMID:

```json
{"aumid": "Microsoft.MicrosoftEdge.Stable_8wekyb3d8bbwe!App", "args": ["https://example.com"]}
```

**Do NOT use** `Start-Process "msedge.exe" "url"` — it activates
the new window.

### Click and input

- **Click on hyperlinks, buttons, form controls inside the page** —
  pixel-click via `(x, y)` works. Chromium dispatches
  `WM_LBUTTONDOWN` to its internal renderer, which routes the click
  to the DOM. Validated: clicking the "Learn more" link on
  `example.com` navigates to `iana.org` (PostMessage fallback path,
  no focus steal). Edge title changes from "Example Domain" to
  "Example Domains" without bringing the browser to front.
- **Click on browser chrome (tabs, address bar, menus, bookmarks
  bar)** — UIA Invoke path works (the chrome is XAML/native, fully
  exposed via UIA). Prefer `element_index` here.
- **Type into web forms** — `type_text` via PostMessage WM_CHAR to
  the focused element. **The element must be focused first.** Click
  it (pixel or element_index) before typing.
- **Type into the URL bar without `Ctrl+L` (which would foreground
  the window)** — use `launch_app({urls: [...]})` to open a new URL
  in a new window. For navigation within an existing window, click
  the address bar via `element_index` (it appears in the chrome UIA
  tree as a Text Edit with name like "Address and search bar"),
  then `type_text` + `press_key({key: "enter"})`.

### Forbidden keyboard shortcuts in browsers

| Shortcut | What it does | Why forbidden | Alternative |
|---|---|---|---|
| `Ctrl+L` / `Alt+D` / `F6` | Focus address bar | Activates window (focus-steal semantics) | `element_index` click on address-bar element |
| `Ctrl+T` | New tab | Activates window | `launch_app({urls: [<url>]})` — opens in new window instead |
| `Ctrl+W` | Close tab | Activates window before closing | If the tab is in a backgrounded window, this is OK with hotkey to pid; otherwise close via UIA on the tab's close button |
| `Ctrl+1..9` / `Ctrl+Tab` | Switch tab | Visible flip, page content re-renders | Prefer windows-per-URL pattern (`launch_app({urls})`) |
| `Ctrl+Shift+T` | Reopen closed tab | Activates window | N/A — usually user intent, ask first |
| `Ctrl+N` | New window | New window comes to foreground | `launch_app({urls: ["about:blank"]})` |
| `Ctrl+Shift+N` | New incognito | Same as above + state mutation | ask user first |
| `F11` | Fullscreen | Visibly disruptive | Avoid |
| `F5` / `Ctrl+R` | Reload | OK if the agent owns this window | safe to use via `hotkey` |
| `Ctrl+F` | Find in page | Activates window + opens find bar | If the agent owns this window, OK |
| `Esc` | Close find bar / cancel | OK | safe |

### Tabs vs windows

Same rule as macOS: drive each URL in its own **window**, not as
tabs in a shared window. Each window has its own `window_id`, its
own UIA tree. Tab-switching within a window is a visible disruption
(see forbidden shortcuts above); window switching via `cua-driver`
is per-pid / per-HWND and invisible.

Tab-title enumeration (read-only) IS safe — walk a window's tab strip
in the UIA tree for `TabItem` elements and read their names. Tab
switching (activating one) is not.

### `page` tool — JS execution, text extraction, DOM query

The cross-platform `page` tool exposes four actions against the
browser instance identified by `(pid, window_id)`:

- **`get_text`** — `document.body.innerText` equivalent, sourced from
  the web `Document`'s UIA `TextPattern`.
- **`query_dom`** — CSS-selector → UIA `ControlType` match.  Supports
  simple tag selectors (`a`, `button`, `input`, `h1`-`h6`, `img`,
  `li`, `p`, `span`, `select`), `tag#id`, `[role=…]`.  **Does not**
  support `.class` or `[data-*]` (UIA has no class-list and no
  data-attribute exposure).
- **`execute_javascript`** — runs arbitrary JS in the active tab.
  Two-tier dispatch (see below).
- **`enable_javascript_apple_events`** — macOS-only; errors here.

#### `execute_javascript` dispatch

1. **Bookmark-URL UIA bypass (default)** — `cua-driver` looks for a
   bookmark named `cua-driver-eval` on the Favorites bar, edits its
   URL to the user's expression wrapped in a `try/catch` IIFE,
   invokes the bookmark via UIA `InvokePattern`, and reads the
   result back from `document.title` (the wrapper writes
   `CUA:<JSON>` or `CUA_ERR:<message>`).  Zero config required —
   no `--remote-debugging-port` flag, no companion extension.

   **Requirements:**
   - The `cua-driver-eval` bookmark **must exist** on the Favorites
     bar.  Any URL is fine; the driver overwrites it on first use.
     Automatic creation (drive the omnibox to `edge://favorites`,
     click "Add favorite", fill the dialog) is not yet wired up —
     create it manually.
   - **The Favorites bar must already be visible.** This is a
     one-time setup: press `Ctrl+Shift+B` once inside the browser
     and the setting persists across sessions. **If the bar is
     hidden, cua-driver now synthesizes `Ctrl+Shift+B` via
     `PostMessage(WM_KEYDOWN/UP)` with no foreground swap** —
     Chromium's `Browser::HandleKeyboardEvent` dispatches
     accelerators from the WM_KEYDOWN LPARAM bits without
     consulting `GetKeyState`, so PostMessage works without the
     `SetForegroundWindow` dance that previously violated the
     no-foreground contract. After a brief settle the path
     re-checks for the bar; if it's still hidden (locked-down
     browser policy / non-Chromium target) the call bails with a
     clear error and falls through to the CDP path.
   - The user's expression should be a single statement or block;
     `return` inside the IIFE is honored.  Bookmarks strip line
     breaks, so multi-line expressions are joined with spaces.

   **Known limitation — Chromium's window activation on Invoke.**
   When the bookmark is invoked via UIA `InvokePattern::Invoke`,
   Chromium activates the browser window because clicking a
   bookmark is a user-initiated navigation in Chromium's input
   model. The activation happens inside Chromium's window-aura
   layer, not in our UIA call. **Mitigation in place**: a polling
   foreground-restore guard runs immediately after the Invoke —
   same pattern `launch_app` uses (PR #1668) — capturing the
   user's foreground HWND beforehand and calling
   `SetForegroundWindow(prev)` once Chromium grabs foreground.
   The restore is gated on `GetWindowThreadProcessId(fg_now) ==
   browser_pid` so we never yank focus from a window the user
   legitimately Alt-Tabbed to. Without UIAccess (the daemon's
   normal integrity) Windows' foreground lock may deny the
   restore — in that case the browser dwell time is bounded to
   the ~600 ms poll budget instead of "until next user action".
   The `get_text` and `query_dom` actions don't share this issue
   (no Invoke → no activation).

2. **CDP fallback** — `Runtime.evaluate` via raw WebSocket against
   `--remote-debugging-port=N`.  Requires the browser launched with
   that flag and `CUA_DRIVER_CDP_PORT=N` exported before the daemon
   starts.

   Use this when the bookmark path can't be made to work (locked-
   down GPO disables Ctrl+Shift+B, user explicitly hides the
   Favorites bar in a fresh profile and won't summon it, etc.).

3. **Either succeeds** → the result is returned to the caller with a
   prefix indicating which path ran: `uia.bookmark_exec: <result>` or
   `cdp.runtime.evaluate.user_gesture: <result>`.

#### Why bookmark exec exists

Chromium's omnibox aggressively strips `javascript:` schemes when
the URL is pasted or `SetValue`-d via UIA, so the "omnibox
`javascript:` then Enter" trick is dead in modern Chrome / Edge.
The bookmark URL field doesn't apply the same scrub because
bookmarklets are a documented Web-platform feature dating back to
the late '90s — closing that path would break a long tail of
existing user data.  Empirically validated on Edge 148.0.3967.70
(see PR description for the commit landing this).

#### Concurrency

The bookmark-exec primitive holds a process-wide mutex.  Calls
serialise — concurrent invocations would race on the single
`cua-driver-eval` URL field and one caller would invoke another's
JS.  If you need parallel JS execution against multiple browser
instances, fall through to CDP (each browser instance gets its
own port, no shared state).

### WebView2 in non-browser hosts (Teams, VS Code, Outlook desktop)

These embed a WebView2 control inside a Win32 host. The HWND
hierarchy is `OuterHost > Chrome_WidgetWin_0 > Chrome_WidgetWin_1
... > WebView2 ...`. UIA Invoke at the page level works for some
controls; for arbitrary DOM nodes, fall back to pixel clicks. The
WebView2's underlying Chromium dispatches PostMessage to its
renderer, so the fallback path works for hyperlinks and buttons.

## Common failure modes (Windows-specific)

- **`Session 0` daemon** — `cua-driver doctor` reports
  `SessionId: 0`. UIA enumeration returns empty, screenshot
  returns blank. Fix: stop the daemon, kick the autostart task with
  `schtasks /Run /TN cua-driver-serve`.
- **Stale HWND** (`Invalid window handle 0x80070578`) — the window
  was closed, re-created (e.g. UWP shutdown-on-idle), or moved to
  a different desktop session. Re-resolve via `list_windows`.
- **Calc display stuck at "0" after pixel clicks** — the (x,y) UIA
  hit-test missed and PostMessage fell through (PostMessage is a
  silent no-op on UWP). Switch to `element_index` mode. Symptom:
  success messages say `Posted click to pid N` instead of
  `Performed UIA Invoke at (sx,sy) ...`.
- **LibreOffice (VCL) `type_text` / `hotkey` reported success but
  nothing happened** — VCL/SAL apps route accelerators through
  `TranslateAccelerator` (reads `GetKeyState`, which PostMessage doesn't
  update) and the Calc/Writer document grid only takes real keystrokes
  when a cell is in edit mode, so background `WM_CHAR` / key-combos are
  silently dropped. Two honesty mechanisms now cover this instead of a
  blind success:
    - **`hotkey` / `press_key`** (keystroke + key-combo): `delivery_mode:"background"`
      surfaces a `background_unavailable` error for VCL.
    - **`type_text`** does a **UIA read-back** and returns a three-way `verify`
      in structured output: `confirmed` (✅, value reflects the text),
      `unchanged` (📨, read OK but value didn't change → likely dropped, retry
      foreground), or `unreadable` (✅ "delivered, not verified"). **Pass an
      `element_index`** for reliable verification: the read-back then reads
      *that specific element* by handle (ValuePattern → TextPattern), which is
      **focus-independent** — it reaches `confirmed`/`unchanged` whether or not
      the target is foreground. (Verified live against the WPF harness: typed
      via element_index, read back `confirmed`, value independently present in
      the next snapshot — app never fronted.) **Without** an element_index it
      falls back to system-wide `GetFocusedElement`, which on Windows only
      resolves when the target is the **foreground** app (no per-app
      `AXFocusedUIElement` like macOS); a backgrounded target then reads
      `unreadable` even when the text actually landed — so `unreadable` is NOT a
      failure signal, verify via screenshot if it matters.
  Escalate to `delivery_mode:"foreground"` for both (SendInput Unicode /
  accelerator). **But** foreground needs the swap to actually land — if the
  daemon lacks UIAccess and `bring_to_front` returns `landed_on_target:false`
  (or it reverts before the next call), you can't drive it by input at all:
  produce the artifact and `launch_app` it (build the `.xlsx` / `.docx` and
  open it) rather than typing into the GUI.
- **Edge / Chrome shows tab switching even though I used pid-scoped
  hotkey** — `Ctrl+Tab` / `Ctrl+1..9` aren't pid-scopable; the
  receiver activates. Use the windows-per-URL pattern.
- **`Get-StartApps` returns no AUMID for an app I see in Start
  Menu** — the app might be a Win32 desktop app, not a packaged
  app. Use `{path: "..."}` instead of `{aumid}`. (Win11 Calculator
  IS packaged; Win10 classic Notepad was not.)
- **`launch_app` returns pid N but `list_windows({pid: N})` returns
  empty** — UWP cold-launch race: the AppFrame HWND hasn't
  materialized yet. Re-call `list_windows({pid: N})` after 500ms;
  for chronic cases, key off the app name in `list_windows({})`
  output.
- **JPEG screenshot has more compression than expected** — default
  quality on the MCP screenshot compat path is 85; for raw
  `cua-driver call screenshot`, defaults to PNG (no compression).
  Pass `{format: "jpeg", quality: 70}` to opt into compressed
  screenshots. The `max_image_dimension` config (default 2048)
  downscales via Lanczos3 before encoding.

## Diagnostics

`cua-driver doctor` reports:

- Daemon version and install paths
- Current session ID (must be ≥1)
- COM apartment status (STA / MTA / uninitialized)
- UIA reachability (can we connect to `CUIAutomation`?)
- AppX broker reachability (for packaged-app activation)
- PATH state (is `cua-driver` actually on PATH?)
- Autostart Scheduled Task status

Run it whenever a tool call returns unexpectedly. Most failures
trace back to one of these checks reading "false."

`cua-driver autostart status` reports whether the daemon is
registered to auto-start at logon AND whether it's currently running:

- `not-registered` — install didn't set up autostart, or user
  removed the task. Re-register via `cua-driver autostart enable`.
- `registered (not running)` — autostart task exists but no daemon
  process. Kick it with `cua-driver autostart kick`.
- `registered (running)` — happy path.

## Recording

Screen recording is **not yet supported on Windows** in
cua-driver-rs. The `recording start|stop|status` subcommands are
registered but return "Recording is currently macOS-only" on
Windows. Tracking: see the cua-driver-rs roadmap in the main repo.

For now, capture state via `screenshot` (per-window or full-desktop)
or `get_window_state` (returns a screenshot embedded alongside the
UIA tree).
