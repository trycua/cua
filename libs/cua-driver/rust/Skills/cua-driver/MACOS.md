# cua-driver — macOS specifics

This file is the macOS-specific extension to `SKILL.md`.
The cross-platform core (snapshot invariant, CLI/MCP defaults,
behavior matrix, canonical loop, pixel-click contract, common error
patterns) is in `SKILL.md`. Read this in addition to `SKILL.md` when
you're driving an app on macOS.

## The no-foreground contract

**The user's frontmost app MUST NOT change.** This is the whole
reason cua-driver exists. Users pay for the right to keep typing in
their editor while an agent drives another app in the background.
Violate this rule and every other nice property the driver gives
you (no cursor warp, no Space switch, no window raise) stops
mattering — you just shipped the Accessibility Inspector with extra
steps.

Before running any shell command, ask: **"does this raise,
activate, foreground, or make-key any app?"** If yes, don't run it.
Every one of the commands below activates the target on macOS and
is therefore forbidden unless the user **explicitly** asked for
frontmost state:

- **Every form of the `open` CLI — `open -a <App>`, `open -b
  <bundle-id>`, `open <file>`, `open <path-to-App.app>`, `open
  <url>` — always activates.** macOS routes all forms through
  LaunchServices, which unhides and foregrounds the target
  regardless of whether you passed an app name, a bundle id, a
  document, a URL, or the bundle path itself. The activation
  happens even when the only intent was "start the process."
  **Never use `open` for any app launch.** This includes launching
  a just-built .app from a local build dir (e.g. `open
  build/Build/Products/Debug/MyApp.app`) — resolve the
  `CFBundleIdentifier` from `Info.plist` and use `launch_app`
  with that id. See "The narrow carve-out" below for why
  `launch_app` is safe even when the app internally calls
  `NSApp.activate`.
- `osascript -e 'tell application "X" to activate'` —
  activates by design. Same for `... to open <file>`,
  `... to launch`, and anything with `activate` in the tell block.
- `osascript -e 'tell application "System Events" to ... frontmost'`
  in a mutating form (setting `frontmost` rather than reading it).
- AppleScript files that invoke `activate`, `launch`, or `open`
  against the target app.
- `cliclick` (moves the user's real cursor to the target coords
  before clicking — a focus-steal-equivalent even if the app's
  window state is unchanged).
- `CGEventPost` with `cghidEventTap` targeting a coordinate over
  a different app's window (warps the cursor, possibly activates
  on hit).
- `AppleScriptTask`, `NSAppleScript`, `Process` wrapping `osascript`
  that contains any of the above.
- `NSRunningApplication.activate(options:)` called from your own
  helper binary — same class.
- Dock clicks and any `open` invocation (see the first bullet —
  every form of `open` goes through LaunchServices which
  activates, full stop).
- **Keyboard shortcuts that semantically mean "focus here" —
  most notably Chrome / Safari / Arc's `⌘L` (focus omnibox) and
  Finder's `⌘⇧G` (Go to Folder).** These aren't pure key events —
  the receiving app interprets "user wants to type here" as
  activation intent and raises its window to be key. Even when
  delivered to a backgrounded pid via `hotkey`, the downstream app
  pulls focus. For an exactly bound Chromium page, use
  `browser_navigate`; otherwise use `launch_app({bundle_id, urls})`
  to create a separately addressable window. Do not emulate navigation
  by writing the omnibox and pressing Return in a background window.
- **Tab-switching shortcuts in browsers (`⌘1..⌘9`, `⌘]`, `⌘[`,
  `⌘⇧[`, `⌘⇧]`) are visibly disruptive even when delivered to a
  backgrounded pid.** The app's key handler processes the shortcut,
  the window re-renders the new tab's content, and the user sees their
  tabs flipping. The typed browser route can inspect and mutate a returned
  `tab_id` without driving this native shortcut. For unsupported browsers,
  prefer separately addressable windows over visible tab switching.

Reading frontmost state is fine (`osascript -e 'tell application
"System Events" to get name of first application process whose
frontmost is true'`). Mutating it is not.

**Corollary — the AXMenuBar rule.** `AXMenuBarItem` + AXPick
dispatches at the AX layer regardless of which app is frontmost,
but macOS's on-screen menu bar always belongs to the frontmost
app. If you drive a *backgrounded* app's menu bar, the AX call
succeeds but the viewer sees the dispatch rendered over the
*frontmost* app's menu bar — confusing in any observed session and
routinely a silent no-op too, because action menu items go
`DISABLED` when their owning app isn't the key window. **So: only
use menu-bar navigation when the target is already frontmost.** For
backgrounded targets, read state via in-window AX (window title,
toolbar `AXStaticText`) and dispatch via in-window `element_index`
or pixel clicks — both paths are frontmost-insensitive. Full
rationale in "Navigating native menu bars" below.

**"Open \<app\>" in user speech means launch, not activate.**
`cua-driver launch_app` is the one correct path for process
startup — it's idempotent (no-op on a running app), returns the
pid, and has an internal `FocusRestoreGuard` that catches
`NSApp.activate(ignoringOtherApps:)` calls the target makes during
`application(_:open:)` and clobbers the frontmost back to what it
was before the launch. That guard is why `launch_app` with `urls`
(e.g. `{"bundle_id": "com.colliderli.iina", "urls": ["~/video.mp4"]}`)
is safe even for apps that normally foreground on media-load
(Chrome, Electron, media players).

## Intent → tool mapping (macOS-specific)

| Intent | Use | Don't use |
|---|---|---|
| Open / launch an app | `launch_app({bundle_id})` or `launch_app({bundle_id, urls:[...]})` | `open -a`, `osascript 'tell app … to launch/activate/open'` |
| Find a pid | `list_apps` or `launch_app`'s return | `pgrep`, `ps`, `osascript frontmost` |
| Enumerate an app's windows | `list_windows({pid})` — or read the `windows` array `launch_app` already returns | `osascript 'every window of app …'` |
| Click / type / scroll / keys | `click`, `type_text`, `scroll`, `press_key`, `hotkey` | `osascript`, `cliclick`, raw `CGEvent`, `open <url>` |
| Drag / drag-and-drop / marquee select | `drag({pid, from_x, from_y, to_x, to_y})` (pixel-only — macOS AX has no semantic drag) | `cliclick dd:`, `osascript drag` |
| Screenshot | `screenshot` or the PNG in `get_window_state` | `screencapture` |
| Quit an app | ask the user first, then `hotkey({pid, keys:["cmd","q"]})` | `kill`, `killall`, `pkill` |
| Hand a file/URL to an app | `launch_app({bundle_id, urls:[<path>]})` | `open -a <App> <path>`, `open <url>` |

### The narrow carve-out

The **only** legitimate use of `osascript -e 'tell app X to
activate'` is when the user **explicitly** asked for frontmost
state ("bring Chrome to the front", "make it frontmost", "I want
to see X"). Reaching for it because a tool call returned something
confusing is wrong — that's the skill's classic foot-in-the-door
failure mode and it steals focus every time.

When a cua-driver call surprises you, diagnose cua-driver first:

- **Empty `tree_markdown`?** `get_window_state` returns **both** the
  AX tree and a screenshot by default — there's nothing to configure and
  no capture mode to pick. An empty tree means the surface isn't AX (a
  non-AX surface: Electron/Chromium/canvas), and the response carries
  `degraded: true` — so act by **`px`** off the screenshot that's
  already in the same response. `capture_mode` is **deprecated and
  ignored** (still accepted so old callers don't error, but it has no
  effect — tree + screenshot come back regardless); don't reach for
  `get_config` to "switch modes," there is no mode to switch.
- **`has_screenshot: false`?** The window capture failed (transient
  race against a close, or the window has no backing store yet).
  Re-snapshot; if persistent, pick a different `window_id` via
  `list_windows`.
- **`Invalid element_index` / `No cached AX state`?** You either
  skipped `get_window_state` this turn or passed a different
  `window_id` than the one the snapshot cached against. The cache
  is keyed on `(pid, window_id)` — indices don't carry across
  windows of the same app. Re-snapshot with the same window_id
  you're about to click in.
- **Sparse Chromium AX tree?** Retry `get_window_state` once — the
  tree populates on second call.

Only after those are ruled out, and only if the user's action
genuinely needs frontmost state, fall through to the activate
fallback. Always name the focus steal in your response ("I'll
briefly bring Chrome to the front because …").

### Verifying actions: cross-check the tree against the pixels you already have

There is no `ax`/`vision` capture toggle. **Every `get_window_state`
returns both the AX tree and a screenshot** (default), so verifying that
an action **landed** never means "go grab a screenshot" — it means
cross-check the tree diff against the pixels you already have in the same
response, and only switch *dispatch rung* on a real signal:

1. **Re-snapshot and read the tree diff** — a changed `AXValue`, a new
   element, a collapsed menu, a disabled button. If the tree shows the
   change, you're done. When you only need the tree diff and don't need
   fresh pixels, pass `include_screenshot:false` to skip the grab — a
   **perf** knob, not a mode flip.
2. **Trust the screenshot and do an element px action** when the tree
   **lies** — the action response carried `effect:"suspected_noop"`, the
   re-snapshot came back `degraded` (empty tree), or the tree looks
   unchanged/unreadable / disagrees with the pixels on a surface where
   it's known to lie:
   - **Canvas-backed editors** — Monaco (VSCode, Cursor), xterm, Figma,
     WebGL. The AX tree shows the chrome but nothing for the canvas
     content; a snapshot's tree can look unchanged after a successful
     edit while the pixels show it landed.
   - **Catalyst / iOS-on-Mac text views** — see "Known text-input
     limits" above. `AXValue` can lag the rendered pixels or report the
     placeholder while the field is actually populated.

On these surfaces you read the result off the screenshot already in the
response, then address the target by `x,y` — an **element px action**.
`px` is your **conscious switch to the pixel addressing path**, not a
different capture: the screenshot was always there, you just change *how
you address* the target. The point is to catch the "type → AX-check
succeeds → believe the lie → find out three calls later" trap on exactly
the surfaces that warrant it.

Rule of thumb:
- **element ax action** (default) — the element lookup before a click
  AND the first verify after it; you address by `[N]` `element_index`
  and read the tree diff.
- **element px action** — when the tree is unreadable / `suspected_noop`
  / `degraded` / disagrees with the pixels, or for pure visual
  inspection (reading a chart). You address by `x,y` off the screenshot
  that's already in the snapshot response.

### Self-check pattern

Before every `Bash` call whose command line touches any macOS app
(launching, opening, clicking, typing, scripting, screenshotting),
run the self-check:

1. **Does this command foreground the target?** If yes — stop and
   translate to the cua-driver equivalent from the mapping table.
2. **Does this command move the user's real cursor?** (`cliclick`,
   any `CGEventPost` at `cghidEventTap` over another app's window).
   If yes — stop; use `click({pid, x, y})` which routes per-pid
   via SkyLight and never warps the cursor.
3. **Does this command bypass cua-driver entirely?** (`osascript`
   mutating GUI state, AppleScript files, external helpers.) If
   yes — stop; find the cua-driver tool that does the intent.

If all three are "no," the command is safe. If you can't answer,
default to stop and ask rather than proceed. A single `open -a`
run by accident kills the demo, the trust, and the user's in-flight
editor state.

## Prerequisites — macOS

1. `cua-driver` is on `$PATH` (`which cua-driver`). If not, point the
   user at `scripts/install-local.sh` and stop.
2. Run `cua-driver check_permissions` (with the daemon up — see step 3).
   The default behavior also raises the system permission dialogs for
   any missing grants, so the user can grant on the spot. If either
   grant still reads `false` after that (user dismissed the dialog),
   tell them to open System Settings → Privacy & Security and grant
   Accessibility and Screen Recording to `CuaDriver.app`, then stop.
   Pass `'{"prompt":false}'` for a purely read-only status check that
   won't steal focus.
3. Start the daemon with `open -n -g -a CuaDriver --args serve` (the
   recommended form — goes through LaunchServices so TCC attributes
   the process to CuaDriver.app). `cua-driver serve &` also works;
   the CLI auto-relaunches through `open -n -g -a CuaDriver` when it
   detects a wrong-TCC context (any IDE-spawned shell: Claude Code,
   Cursor, VS Code, Conductor). Verify with `cua-driver status`.

## Resolve target pid — always via `launch_app`

**Always start with `launch_app`**, whether or not the target is already
running. It's idempotent (relaunching returns the existing pid with no
side effects) and gives you the pid in one call — no `list_apps` hop.

- `launch_app({bundle_id: "com.apple.finder"})` — preferred, unambiguous.
- `launch_app({name: "Calculator"})` — when bundle_id isn't known.

`launch_app` is a **hidden-launch primitive by design** — that's the
entire point of cua-driver: agents drive apps in the background while
the user keeps typing in their real foreground app. The target's
window is initialized (AX tree fully populated, clickable via
`element_index`, the pid appears in `list_apps`) but not drawn on
screen. The driver never activates or unhides apps on its own; that
would violate the no-foreground contract the whole driver exists to
protect.

If the user explicitly wants the window visible (usually for a demo
or recording), they unhide it themselves — Dock click, Cmd-Tab, or
Spotlight. Do not reach for `open` / `osascript activate` as a
shortcut to make the window visible; those paths break the backgrounded
invariant on every call, not just the call that "needed" the
foreground. Say out loud what the user needs to do ("click the
Todo app in your Dock to bring it forward") and let them do it.

Never shell out to **any** form of `open` (including `open
<path-to-App.app>` for a just-built binary — resolve the bundle id
from `Info.plist` and use `launch_app` with that), `osascript 'tell
app … to launch/open'`, or similar. Those paths activate the target,
bypass the driver's focus-restore guard, and require a Bash
permission prompt the agent loop shouldn't be burning on app launch.

## Pixel-click dispatch (macOS)

The pixel click is routed through SkyLight's per-pid event path
(`SLEventPostToPid`), not the system HID stream. The dispatch recipe
is the backgrounded "noraise" sequence: yabai's focus-without-raise
SLPS event records followed by an off-screen user-activation primer
and the real click. The target app becomes AppKit-active for event
routing but its window does **not** rise to the front of the
z-stack, and macOS's "switch to Space with windows for app" follow
is suppressed. Full mechanics in
`Sources/CuaDriverCore/Input/MouseInput.swift` (`clickViaAuthSignedPost`)
and the companion `FocusWithoutRaise.swift`.

### `delivery_mode` on the pointer family (macOS)

`click`, `double_click`, `right_click`, `drag`, and `scroll` accept
`delivery_mode` (`"background"` default / `"foreground"`) — matching the
breadth Windows and Linux already exposed (`type_text` / `press_key` /
`hotkey` carry it too). `"background"` is the SkyLight per-pid path above:
no raise, no focus steal. `"foreground"` briefly fronts the owning app,
acts, then restores the prior frontmost — the explicit last resort for a
surface that only accepts events while frontmost (the canvas/viewport/game
case below). Element-indexed (AX) actions are inherently background and
hold the no-foreground contract without the flag.

macOS-specific residuals worth knowing (the rest of the capture/dispatch/
addressing params are a shared cross-platform contract — see `SKILL.md` →
*Cross-platform parameter contract*):

- **`check_permissions.prompt` is macOS-only.** It raises the TCC
  Accessibility / Screen-Recording dialogs; pass `{"prompt":false}` for a
  read-only status check. There is no Windows/Linux equivalent — TCC is a
  macOS construct, so the param is intentionally absent from the shared
  contract.
- **`session` always worked on macOS;** the cross-platform change is that
  Windows/Linux stopped *rejecting* it. No macOS-side change to how you
  pass it.
- **`scope`** (`window` default / `desktop`) is a **macOS-specific per-call
  param** on the pointer tools — pass `scope:"desktop"` with `x,y` and no
  pid/window_id for a screen-absolute click. Windows and Linux support the
  same windowless capability but gate it on the persisted `capture_scope`
  config instead (`set_config capture_scope=desktop`), so they have no
  per-call `scope` param. Unifying the knob is a tracked follow-up.

### Canvases, viewports, games (Blender, Unity, GHOST, Qt, wxWidgets)

Apps whose main surface is an OpenGL / Metal / Qt / wxWidgets
viewport expose **no useful AX tree** — the whole surface is one
opaque `AXGroup` or `AXWindow` from AX's perspective. Per-pid event
paths (`SLEventPostToPid`, `CGEvent.postToPid`) are filtered by the
viewport's own event-source check and silently dropped — the event
loop wants "real HID origin".

The working pattern:

1. Bring the target frontmost (a brief `osascript activate` is
   acceptable here — this is the carve-out the skill's osascript
   gate allows).
2. `CGEvent.post(tap: .cghidEventTap)` with a leading `mouseMoved`
   event (~30 ms before the click). `cua-driver click` when the
   target is frontmost automatically takes this path.
3. Accept that the real cursor visibly moves — `cghidEventTap` is
   the system HID stream, the cursor warps to the click point.

There is no backgrounded path that reaches these apps today.

### Known pixel-click limits

- **Chromium `<video>` play/pause**: pixel click is often rejected
  by HTML5's click-to-play handler on some builds. Use keyboard
  instead: `press_key({pid, key: "k"})` (YouTube) or
  `press_key({pid, key: "space"})` (generic). Keyboard events
  travel through a different auth envelope.
- **Pixel right-click on Chromium web content** coerces to a
  left-click — a known Chromium renderer-IPC limitation that affects
  every non-HID-tap synthesis path. For context menus on
  AX-addressable elements (links, buttons, toolbar items), use
  `right_click({pid, element_index})` instead.

### Known text-input limits (Catalyst + Electron)

On **Catalyst** apps (WhatsApp, Reminders, Notes-via-Catalyst,
anything in `/Applications` that's actually `iOSAppOnMac.app`) and
**Electron** apps (VS Code's Monaco editor, Slack composer, Discord,
Linear), an AX `type_text` can't reach the rendered text view: the
`AXSetAttribute(kAXSelectedText)` write succeeds on the AX shim, but
the UIKit/Chromium view that owns the input never observes it — and on
Electron the shim *echoes the value straight back through `AXValue`*,
so a naive read-back "confirms" a value that isn't really there.

The driver **detects Electron and refuses to trust that echo**: an
AX-path `type_text` on an Electron app returns `effect:"unverifiable"`
+ `escalation:{recommended:"px"}`, **never** a false `verified:true`.
(On Catalyst the AX value reads back unreadable, so it reports
unverified too.) Bottom line: on these surfaces **do not trust the AX
confirm — the screenshot in the same response is the only truth.**

Fix — **one call**: `type_text({pid, window_id, x, y, text})`. Passing
`x,y` (no `element_index`) is the **element px action** form of
`type_text` — the tool pixel-clicks at `(x,y)` to give the Chromium /
UIKit renderer the real keyboard focus the AX layer can't, then types
into the now-focused field. Read `x,y` straight off the screenshot in
the `get_window_state` response (same convention as `click`). This is
the one-call replacement for the old two-step "pixel-click then
`type_text`" — and you do **not** reach for a clipboard + `Cmd+V`
dance.

0. **If the control is CLOSED, open it first.** A px focus-click won't
   reliably *open and focus* a closed control (a search button, a
   collapsed field) — it lands on whatever is already focused (e.g.
   the message composer), so your text leaks there. **AX-press to
   open/activate the control first** (AX actions work in the
   background), then px-type into the now-open field.
1. **`type_text({pid, window_id, x, y, text})`** — focus + type in a
   single call. Re-snapshot and read the text off the screenshot to
   confirm; the AX value can still lag on Catalyst/Electron.
2. Only if the keystrokes *still* drop (a focus-polling app), escalate
   that one `type_text` with `delivery_mode:"foreground"`.

The `x,y` (px) form is **mutually exclusive** with `element_index`
(ax) — pass one or the other, not both. Why not `Cmd+V` / `hotkey`: a
keyboard combo does **not** focus a text field, and `hotkey` /
`press_key` no longer raise the window on their own (raising is gated
on `delivery_mode:"foreground"`, like every other tool). The reliable
move is the px form of `type_text` — focus and type in one call.

## Navigating native menu bars (AXMenuBar)

**Only drive the menu bar when the target app is frontmost.** This
is the single most-misused cua-driver capability. If the target is
backgrounded, don't reach for `AXMenuBarItem` + AXPick — use
in-window `element_index` or pixel clicks instead. Two reasons, one
functional and one perceptual:

- **Functional:** menu items that touch document/playback/editor
  state go `DISABLED` when their owning app isn't the key window
  (Preview rotate, IINA speed change, most editor commands). AXPick
  + AXPress will dispatch successfully from the driver's side but
  no-op at the target — you get a silent false-pass.
- **Perceptual (matters for demos, screen recordings, and anything
  the user watches live):** macOS's screen-rendered menu bar
  always belongs to the *frontmost* app. AXPick on a backgrounded
  app's `AXMenuBarItem` dispatches to that app's per-process menu at
  the AX layer, but any visible menu render happens over the
  frontmost app's menu bar — the viewer sees an IINA submenu
  flashing on top of Chrome's menus, which reads as "the agent
  clicked the wrong app." The AX call was correct; the frame the
  user sees is not. For recorded or observed sessions, this is an
  integrity bug even though it's not a correctness bug.

**Good decision rule:** if the target is not already frontmost, do
not use `AXMenuBarItem` at all. For *reading* in-window state,
snapshot the window AX tree — most apps expose the same state via
an in-window `AXStaticText`, title bar, or toolbar. For *dispatching*
actions, use in-window `element_index` (buttons, toolbar items) or
pixel clicks on in-window controls — both dispatch via AppKit's
window-under-pointer hit-test and are **not** frontmost-gated.

When the target IS frontmost, the menu-bar flow below is fine and
the canonical path for menus.

### The two-snapshot pattern (target frontmost only)

Menu contents are a two-snapshot flow. Closed AXMenu subtrees are
deliberately skipped during snapshot — otherwise every app's File /
Edit / View hierarchy plus every Recent Items macOS has ever seen
would inflate the tree 10-100x. But once a menu is *open*, its
AXMenuItem children do receive `element_index` values so you can
click them normally.

1. Find the `[N] AXMenuBarItem "<Menu Name>"` in the tree.
2. `click({pid, element_index: N, action: "pick"})` — menu bar items
   implement `AXPick` ("open my submenu"), not `AXPress`. Using the
   default action on an AXMenuBarItem is a no-op.
3. Re-snapshot. The expanded menu's items now appear under the bar
   item as `[M] AXMenuItem "<Item Name>"`.
4. Click the target item — most items respond to `AXPress` (default
   action). Submenus nest under the item and are walked the same way.
5. Re-snapshot and verify.

If you ever need to back out without selecting, `press_key({pid, key:
"escape"})` closes the open menu. Leaving a menu expanded between
turns poisons subsequent snapshots for that pid.

### Commands gated on the target being frontmost

Some menu items and global shortcuts (Preview's Tools → Rotate
Right, ⌘R; anything in the View menu that manipulates the current
document; most editor commands) are **disabled unless the target
app is the key / frontmost window**. You'll see it in the AX tree
as `DISABLED` on the menu item even though the user's intent is
obviously valid.

Before activating, confirm you're in this narrow case — the menu
item still reads `DISABLED` after a fresh snapshot AND the action
the user requested genuinely requires frontmost (Preview rotate,
View menu document manipulation, editor commands). If either
check fails, don't activate.

When both checks pass, the driver has no `activate` tool
(deliberately — the whole point is backgroundable control), so
this is the one legitimate `osascript` fallback:

```bash
osascript -e 'tell application "<App Name>" to activate'
```

Then re-snapshot — the menu item loses its `DISABLED` tag — and
`click({action: "pick"})` the item. Alternatively, a `hotkey`
call delivered to the now-frontmost app works for the shortcut
form (`⌘R`, `⌘+`, etc.).

**Always name the focus steal in your response** so the user isn't
surprised — "Briefly activating Preview to enable Tools → Rotate
Right" or similar. Don't silently steal focus. You don't need to
restore the previous frontmost afterwards unless the user asks —
they can cmd-tab back.

## Browsers on macOS

Use `BROWSER.md` for the typed browser capability workflow. Chrome and Edge
support exact native-window binding, page refs, navigation, typing, and an
explicit synthetic DOM click. Existing-profile preparation is separately
approved and may automate the exact product-specific remote-debugging control;
it does not depend on System Events or direct profile-file edits.

Standalone Chromium activates its window when CDP's trusted pointer route is
used on macOS. The driver therefore returns
`browser_input_trust_unavailable` before dispatch rather than falsely claiming
background delivery. Use `input_route:"dom_event"` only when synthetic click
semantics are acceptable. Embedded Electron has a separately bounded route;
do not infer that route for arbitrary WKWebView or Tauri hosts.

Browser chrome, permission prompts, downloads, file pickers, Safari, Firefox,
and unbound embedded webviews remain native surfaces. Inspect them with
`get_window_state` and use the AX/PX ladder in this file. The legacy `page`
tool and Apple Events JavaScript bridge remain compatibility surfaces, not the
starting point for new browser workflows.

## macOS common error patterns

| Error text | Meaning | Fix |
|---|---|---|
| macOS system-alert beep on `press_key` with no visible change | Target window is minimized; Return / Space / Tab commits don't establish real renderer focus on minimized windows | AX-click a clickable equivalent (Go button, Submit button, checkbox) instead of pressing the key; see "Keyboard commits on minimized windows" under the Browser section |
| `Accessibility permission not granted` | TCC not granted | Stop; tell user to grant in System Settings |
| `Screen Recording permission not granted` | TCC not granted for capture | Affects `screenshot` and `get_window_state` (which always captures). Grant in System Settings — the driver can't operate without it |

## Example end-to-end task (macOS)

**User:** "Open the Downloads folder in Finder."

1. `launch_app({bundle_id: "com.apple.finder", urls: ["~/Downloads"]})`
   → `{pid: 844, windows: [{window_id: 6123, title: "Downloads", ...}]}`.
   Idempotent launch; plus Finder opens a hidden window rooted at
   `~/Downloads` via `application(_:open:)` — zero activation, no
   focus steal. The `windows` array lets you skip a `list_windows` hop.
2. `get_window_state({pid: 844, window_id: 6123})` → verify an
   `AXWindow` whose title contains "Downloads" is present with a
   populated AX subtree (sidebar, list view, files).
3. Done.

If the user instead asks to navigate *within* an already-open Finder
window, use the menu-bar flow from "Navigating native menu bars"
above (click Go → pick a menu item → re-snapshot → click it).
