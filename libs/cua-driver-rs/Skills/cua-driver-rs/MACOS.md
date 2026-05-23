# cua-driver-rs — macOS specifics

This file holds macOS-only material that used to live in `SKILL.md`.
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
  pulls focus. **For omnibox navigation specifically**, the correct
  path is `launch_app({bundle_id: "com.google.Chrome", urls:
  ["https://…"]})` — no omnibox dance, no `⌘L`, no focus-steal. Do
  NOT try `set_value` on the omnibox: Chrome's commit logic requires
  a "user-typed" signal that neither an AX value write nor
  `CGEvent.postToPid` keystrokes supply from a backgrounded pid —
  the URL lands in the field but Return fires as a no-op. See
  `WEB_APPS.md` → "Navigate to a URL" for the full pattern. The
  general principle: a shortcut that says "put my cursor inside this
  app" is a focus-steal; a shortcut that says "do this thing" (copy,
  save, quit) is fine.
- **Tab-switching shortcuts in browsers (`⌘1..⌘9`, `⌘]`, `⌘[`,
  `⌘⇧[`, `⌘⇧]`) are visibly disruptive even when delivered to a
  backgrounded pid.** The app's key handler processes the shortcut,
  the window re-renders the new tab's content, the user sees their
  tabs flipping. There is no AX-only workaround: page content (HTML,
  form state, `AXWebArea`) populates only for the focused tab;
  inspecting a background tab requires activating it, which is the
  visible flip. Observed with Dia; the same mechanic applies to every
  Chromium-family browser (Chrome, Arc, Brave, Edge).

  **Prefer the windows-over-tabs pattern**: for each URL you need to
  drive backgrounded, use `launch_app({bundle_id, urls: [url]})` —
  browsers open each URL in a new **window**. Each window has its own
  `window_id`, its own AX tree, and can be inspected / interacted with
  via `element_index` without activating or switching anything. Tabs
  are a UX grouping for humans; cua-driver workflows should default to
  windows. See `WEB_APPS.md` → "Tabs vs windows" for the full pattern.

  Tab-title enumeration (read-only) IS safe — walk a window's toolbar
  AX tree for `AXTab` / `AXRadioButton` children and read their
  `AXTitle`s. Tab switching (activating one) is not.

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

- **Tiny screenshot / empty `tree_markdown`?** Check
  `cua-driver get_config` → `capture_mode`. Default `"som"` returns
  both the AX tree and screenshot. `"vision"` omits the AX tree
  (PNG only), `"ax"` omits the PNG. If a snapshot lacks a tree,
  `capture_mode` is almost certainly `"vision"` — either reason
  purely from the PNG or flip to `"som"` / `"ax"` via `set_config`.
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

## Browser JS primitives — Apple Events backend

The `page` tool is **cross-platform** — Windows uses UIA + CDP,
Linux uses AT-SPI + CDP, and macOS (this section) uses the
Apple-Events / CDP / AX-tree routing described below.

When the AX tree doesn't expose the data you need (common in
Chromium/Electron — the tree is sparse for web content), use the
`page` tool or the `javascript` param on `get_window_state` to query
the DOM directly via Apple Events. Requires "Allow JavaScript from
Apple Events" to be enabled — see `WEB_APPS.md` for the setup path.

**Three actions on the `page` tool:**

- `page({pid, window_id, action: "get_text"})` — returns
  `document.body.innerText`. Fastest way to read page content, prices,
  article text, or any raw text the AX tree truncates or omits.

- `page({pid, window_id, action: "query_dom", css_selector: "a[href]",
  attributes: ["href"]})` — runs `querySelectorAll` and returns each
  match's tag, text, and requested attributes as a JSON array. Use for
  table rows, link hrefs, data attributes, structured page data.

- `page({pid, window_id, action: "execute_javascript", javascript:
  "..."})` — raw JS. Wrap in an IIFE with try-catch. Don't use this for
  elements already indexed by `get_window_state` — `click` and
  `set_value` are more reliable there.

**Co-located read — `get_window_state` with `javascript`:**

```bash
get_window_state({pid, window_id, javascript: "document.title"})
```

Runs the JS and appends the result as a `## JavaScript result` section
alongside the AX snapshot — one round-trip instead of two. Use this
when you need both the element tree (for subsequent clicks) and some
page data in the same turn.

**Decision rule — AX vs JS:**

| Need | Use |
|---|---|
| Click / type into an element | `get_window_state` → `click` / `set_value` (AX, works backgrounded) |
| Read text the AX tree drops | `page(get_text)` or `get_window_state(javascript=)` |
| Scrape structured data (tables, hrefs) | `page(query_dom)` |
| Trigger JS events / mutations | `page(execute_javascript)` |

Supported backends:

| App type | How | Context |
|---|---|---|
| Chrome / Brave / Edge | Apple Events `execute javascript` | Full DOM ✅ |
| Safari | Apple Events `do JavaScript` | Full DOM ✅ |
| Electron (VS Code, Cursor…) | SIGUSR1 → V8 inspector → CDP | Main process only: `process`, `Buffer` — no `document`, no `require` in sandboxed apps |
| Electron (with `--remote-debugging-port`) | CDP page target | Full DOM ✅ |

**Electron sandbox note:** SIGUSR1 connects to the Node.js *main* process.
Sandboxed Electron apps (VS Code, Cursor) strip `require` and Electron
APIs there. Useful for: `process.env`, `process.versions`, `process.cwd()`,
`process.pid`. For full DOM/renderer access, launch the app with
`--remote-debugging-port=9222` — cua-driver will detect and prefer the
page target automatically.

Arc returns no values; Firefox has no JS-via-AppleEvents support — see
`WEB_APPS.md` for the full matrix.

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
