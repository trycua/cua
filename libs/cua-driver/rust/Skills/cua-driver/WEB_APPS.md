# Driving web-rendered apps

## Prefer the typed browser tools for Chromium page mutation

For Chrome, Edge, Brave, and Electron, use the first-class browser workflow
before legacy `page`, AX, or pixel mutation when a DevTools endpoint is
available:

1. Start a named driver session.
2. Select the exact native `(pid, window_id)` from `launch_app` or
   `list_windows`.
3. Bind with `get_browser_state({pid, window_id, session})`. Continue only when
   `binding_quality` is `exact`.
4. Snapshot with `get_browser_state({target_id, tab_id, session})`.
5. Use the returned current ref with `browser_click` or `browser_type`, or use
   `browser_navigate` for a URL.
6. Re-snapshot after navigation and whenever a call reports
   `browser_ref_stale`. End the session when done.

If binding reports `browser_requires_setup`, use the separately approved
`browser_prepare` tool with `allow_launch:true` and
`profile:{mode:"isolated_new"}`. It returns a new `prepared_pid`; select that
process's window and bind again. Do not pass remote-debugging flags through
`launch_app`: the driver rejects that route so an agent cannot expose a user's
normal profile accidentally. MCP hosts approve the destructive prepare call;
direct/raw clients first mint a single-use token with the interactive
`cua-driver browser-approve` command.

### Existing authenticated Chrome profiles (experimental macOS route)

Do not treat ordinary MCP approval as permission to attach to a person's live
profile. Mint an exact one-use artifact interactively before allowing any
existing-profile setup:

```bash
cua-driver browser-approve --strategy existing_profile \
  --pid <pid> --window-id <window_id> --session <session>
```

Pass the token to `browser_prepare` with
`strategy:{kind:"existing_profile"}` and the same pid, window, and session.
On macOS Chrome, if no endpoint exists, this approved operation opens a
temporary tab in that exact window, navigates to Chrome's internal
remote-debugging page, toggles the uniquely labelled per-instance checkbox,
proves the loopback listener belongs to the approved pid, and closes the tab.
Every visible effect, including temporary in-app address-field focus, is
returned in `side_effects`; a refusal after setup starts reports partial effects
in `detail.setup_side_effects`. Any missing or ambiguous control is refused.
The exact AX setup currently recognizes English Chrome labels and fails closed
on other locales. `get_browser_state` never performs this setup.
After `attached_existing_profile`, bind again with `get_browser_state`; every
older target, tab, and ref is stale. Rebind again after any successful bounded
reconnect. Never retry a consent dismissal, invent an approval token, click a
similar-looking dialog yourself, or pass the profile's path. End the named
session when finished to revoke the in-memory grant.

This is the page-aware Chromium rung. Navigation, default ref-bound text
insertion, and an explicit DOM click can run against an exactly bound page
without raising its native window. `browser_click` defaults to trusted
`Input.dispatchMouseEvent`: standalone Windows has passing background evidence,
while standalone macOS and Linux return `browser_input_trust_unavailable`
before dispatch because Chromium activates its window on that route. Embedded
Electron has a separately proven bounded route. Use
`input_route:"dom_event"` only when a synthetic DOM click is explicitly
acceptable. Target ids, tab ids, and refs are opaque session capabilities, not
raw CDP ids.

Refs traverse open shadow roots and same-process frames. Out-of-process frames
are included only when the runtime exposes an independently attached CDP
session; otherwise the frame is reported as a limitation. The legacy guidance
below remains relevant for Safari, Firefox, browser chrome, and surfaces where
exact browser binding is unavailable.

> **Most legacy guidance below is macOS-specific.** It covers AX-tree quirks, AppleScript
> JavaScript bridges, and Chromium / WebKit / Electron / Tauri patterns
> on macOS. For Windows web-app automation (Edge, Chrome with
> PostMessage / WebView2), see `WINDOWS.md` → "Web apps on Windows". For
> Linux: `LINUX.md` (the cross-platform `page` tool below uses AT-SPI
>
> - CDP the same way).
>
> The `page` tool itself is **cross-platform** — Windows + Linux back
> `get_text` / `query_dom` with UIA / AT-SPI respectively, and
> `execute_javascript` on those platforms uses CDP (the endpoint may be
> supplied by a test-owned fixture or a driver-owned browser prepared through
> `browser_prepare`; legacy callers can expose its port via
> `CUA_DRIVER_CDP_PORT`). macOS routing (this doc) remains
> Apple Events → CDP → AX-tree fallback.

Covers apps whose UI is rendered in a web runtime inside a native
macOS shell:

- **Chromium-family browsers** — Chrome, Edge, Brave, Arc, Vivaldi,
  Opera
- **WebKit** — Safari
- **Electron apps** — Slack, Discord, VS Code, Notion, Figma (desktop),
  and most "native" chat / productivity apps
- **Tauri apps** — use macOS's built-in WKWebView; native menu bar +
  web content, similar to Electron in driving patterns

These apps share two traits that drive the rest of this file:

1. Their AX tree is **sparse** until explicitly enabled, and even
   then can be incomplete.
2. Their web content is routed through a renderer with its own input
   filters — synthetic events need specific delivery paths to land.

## Sparse AX trees — populate on first snapshot

Chromium and Electron apps ship with their web accessibility tree
disabled by default. CuaDriver flips it on automatically the first
time you snapshot such an app — the first `get_window_state` call for
that pid takes up to ~500 ms while Chromium builds the tree,
subsequent calls are fast. Because `launch_app` runs hidden, the
Chromium activation nudges (`AXManualAccessibility`,
`AXEnhancedUserInterface`, `AXObserver` registration) all happen
during the `get_window_state` snapshot itself — no explicit activation
is needed to populate the tree.

If the first snapshot still looks sparse (just the window frame and
menubar), **retry once** — Chromium occasionally needs a second call
to finish populating.

If it stays sparse after a retry, the target's AX tree genuinely
doesn't expose the UI you want. Prefer these before reaching for
pixels:

1. Look for native entry points Chromium apps usually keep AX-visible:
   menu bar items (`AXMenuBarItem`) — expand them via the two-snapshot
   flow in SKILL.md's menu section, cmd-k style command palettes
   (often AX-exposed), toolbar buttons in the window chrome.
2. Use keyboard shortcuts delivered straight to the pid —
   `hotkey({pid, keys: ["cmd", "enter"]})`, `hotkey({pid, keys:
["cmd", "k"]})`, etc. Posted via `CGEvent.postToPid`, reaches the
   target regardless of AX state, no activation required.
3. For typing into web inputs, use `type_text` — it automatically
   falls back to CGEvent synthesis when the input doesn't implement
   `AXSelectedText`, reaching any focused keyboard receiver including
   Unicode / emoji.
4. If none of the above reaches the target, tell the user this
   interaction isn't reachable from the driver today and ask for
   guidance.

## Navigate to a URL

**Primary path — `launch_app` with `urls`:**

```
launch_app({bundle_id: "com.google.Chrome", urls: ["https://cua.ai"]})
```

Opens the URL in a new tab/window on the existing Chrome pid (or
starts Chrome if it isn't running). Fully backgrounded — the
driver's `FocusRestoreGuard` catches Chrome's internal
`NSApp.activate(ignoringOtherApps:)` during `application(_:open:)`
and clobbers the frontmost back to what it was before the call.
No omnibox dance, no focus-steal, no `⌘L` flash. This is the
default recommendation — use it even when Chrome is already
running.

Caveat: the new window is **hidden-launched** (the whole point of
cua-driver). If the user needs to see the page on screen, tell
them to Cmd-Tab / click the Dock icon; the driver never unhides.
AX reads + element-indexed actions against the hidden window
work normally, so for agents that just need to extract / click
things on the loaded page, no unhide is required.

**Last-resort path — omnibox via `⌘L`:** forbidden under the
no-foreground contract (see SKILL.md) because `⌘L` activates
Chrome even when delivered to a backgrounded pid. Keep this
documented only as historical context:

```
# DON'T DO THIS — ⌘L steals focus. Use launch_app above.
hotkey({pid, keys: ["cmd", "l"]})
type_text({pid, text: "https://cua.ai", delay_ms: 30})
get_window_state({pid, window_id})
click({pid, window_id, element_index: <suggestion>})
```

**Why not AX `set_value` + `press_key return` on the omnibox?**
Empirically, Chrome's omnibox commit logic requires a "user-typed"
signal that neither a raw AX value set nor `CGEvent.postToPid`
keystrokes reliably supply from a backgrounded pid. The URL
lands in the omnibox but Return fires as a no-op on the page
body instead of committing navigation. `launch_app({urls})` side-
steps this entirely by handing the URL to Chrome through the
canonical Apple Events / LaunchServices `open` path the app
itself honors.

Minor caveats for the rare case a `⌘L` flow is still needed
(last-resort only, with user buy-in on the focus flash):

- Don't drop `delay_ms` below ~25 for keystroked typing on
  Chromium — below that, autocomplete insertions interleave with
  your characters and you get garbage like `"exuample.comn"`
  instead of `"example.com"`.
- Chrome exposes omnibox suggestions as clickable AXMenuItems in
  a dropdown popup. Clicking the first match via AXPress is
  more reliable than pressing Return (which may not commit).

## Tabs vs windows — prefer windows for backgrounded drive

Browsers (Chrome, Dia, Arc, Brave, Edge, Safari) structure their
surface area as {windows → tabs → page content}. Picking the
right level for cua-driver is critical:

- **Tabs** share a window. Only the focused tab's `AXWebArea` is
  populated; switching tabs to drive a different one is visibly
  disruptive. `hotkey ⌘<N>` posts the real shortcut, the window
  re-renders, the user sees the flip. There is no AX path to
  read a background tab's DOM.

- **Windows** are independent AX trees. Each has its own `window_id`,
  its own `AXWebArea` with the page's content, and can be driven
  backgrounded via `get_window_state({pid, window_id})` + element-
  indexed clicks without activating or raising the window.
  `launch_app({bundle_id, urls: [url]})` opens each URL in a new
  window (tested against Chrome; other browsers vary).

**Rule of thumb:** if the user needs to drive content across URLs
in the background, open each URL in its own **window** via
`launch_app({urls: [...]})` and address them by `window_id`. Only
reach for tab shortcuts when the user explicitly asked for "do it
in a specific tab" (rare).

**Read-only tab enumeration is fine.** Walk the window's toolbar /
tab-strip in the AX tree for `AXTab` / `AXRadioButton` elements
and read their `AXTitle`s. You can discover which tabs exist and
what URLs/titles they carry without switching to any of them.
Only _activating_ a specific tab is visible.

## Keyboard commits on minimized windows

When the target window is **minimized** (genie'd into the Dock):

- **AX reads** (`get_window_state`), element-indexed AX **clicks**, and
  AX **value writes** (`set_value`) all still work — they land on
  the minimized AX tree and don't deminiaturize the window.
- **Keyboard commit events** — Return after typing into a text
  field, Space to toggle a checkbox, Tab to move focus — often
  **don't actually fire the element's handler**. The keystroke
  reaches the app via `SLEventPostToPid` but the app's renderer-side
  input focus isn't established on the intended field (setting
  `AXFocused=true` on a minimized window's descendants doesn't
  propagate to real keyboard focus). Symptom: macOS system-alert
  beep, or silent no-op. Example: `hotkey cmd+L` +
  `type_text URL` + `press_key return` on minimized Chrome —
  the URL lands in the omnibox AX value but Return doesn't commit
  the navigation.
- **Primary workaround — use `set_value` to commit directly**: For
  text fields, `set_value({pid, window_id, element_index, value})`
  sets the entire field value at once, bypassing keyboard commits.
  For a URL in Chrome: find the omnibox via `get_window_state`, then
  `set_value({pid, window_id, element_index: <omnibox>, value: "https://…"})`.
  The value is committed to the AX tree and rendered. Chrome
  auto-navigates when the omnibox value changes via AX on many
  versions.
- **Secondary workaround — find a clickable equivalent**: If
  `set_value` doesn't auto-commit, find a button and AX-click it
  instead. For a URL, click the "Go" button if exposed; for a form,
  click Submit; for a toggle, AX-click the checkbox. Clicks route
  through AXPress, which doesn't need renderer focus.
- **Last resort — tell the user the window needs to be un-minimized**:
  Only if neither `set_value` nor clickable equivalents work. Don't
  silently deminiaturize the window — layout-disrupting side-effect
  on many apps.

## Scroll the main page

```
snap = get_window_state({pid, window_id})
# Find the AXWebArea — typically one per tab.
scroll({pid, window_id, direction: "down", amount: 3, by: "page", element_index: <web_area>})
```

Under the hood: `scroll` synthesizes PageUp / PageDown / arrow-key
keystrokes and posts them via the same auth-signed `SLEventPostToPid`
path `press_key` uses. That's why it reaches Chromium even when the
window is backgrounded. Wheel events posted via the same per-pid
SkyLight path are silently dropped by Chromium's renderer (no
Scroll-specific auth subclass exists — probe tests confirmed this),
so the working primitive is keyboard.

Granularity: `by: "page"` → PageDown/PageUp (one viewport height
per unit). `by: "line"` → arrow keys (fine-grained; a few pixels
per unit in web views, one line in text views). Horizontal `page`
falls back to Left/Right arrows since there's no standard
horizontal-page shortcut.

`element_index` is focused (`AXFocused=true`) before the
keystrokes fire — useful for directing the scroll into a specific
element. Without it, keys land wherever the pid's current focus is.

## Jump to page bottom / top

```
press_key({pid, window_id, element_index: <web_area>, key: "end"})
# or "home" / "pagedown" / "pageup"
```

Targets the `AXWebArea` directly (not the omnibox). Routes keys
through SkyLight's `SLEventPostToPid` where available, falling back
to `CGEventPostToPid`. Works for most in-page shortcuts against a
backgrounded window.

## Click something inside a page

```
click({pid, window_id, element_index: <some_AXLink_or_AXButton>})
```

Standard element-indexed click. Chromium exposes `AXLink` /
`AXButton` / `AXTextField` / etc. under the `AXWebArea` — walk the
tree to find your target, snapshot, click.

For a **context menu** on a browser-chrome element (links, buttons,
toolbar items — anything that advertises `AXShowMenu`), use
`right_click({pid, window_id, element_index})`. Pure AX RPC,
identical to `click({pid, window_id, element_index, action: "show_menu"})`.

For a context menu on **web content itself** (right-clicking an
image, a selection, the page background), try `right_click({pid, x,
y})` — synthesizes a `rightMouseDown`/`rightMouseUp` via auth-signed
`SLEventPostToPid`. **Known limitation**: Chromium web content
coerces the event back to a left-click — this appears to affect
every non-HID-tap synthesis path. Prefer `element_index` whenever
the target is AX-addressable.

## Enable "Allow JavaScript from Apple Events" — browser support matrix

| Browser | `execute javascript` supported | Setting needed | Programmatic path                                |
| ------- | ------------------------------ | -------------- | ------------------------------------------------ |
| Chrome  | ✅ Full                        | ✅ Yes         | Edit Preferences JSON (see below)                |
| Brave   | ✅ Full                        | ✅ Yes         | Edit Preferences JSON (same key, different path) |
| Edge    | ✅ Full                        | ✅ Yes         | Edit Preferences JSON (same key, different path) |
| Safari  | ✅ Full (`do JavaScript`)      | ✅ Yes         | UI automation only — `defaults write` broken     |
| Arc     | ⚠️ No return values            | No toggle      | No reliable path                                 |
| Firefox | ❌ Not supported               | N/A            | N/A                                              |

### Chrome / Brave / Edge — Preferences JSON

Required for `osascript execute javascript` calls. All three are
Chromium-based and share the same preference key and mechanism.
Each browser stores preferences per-profile.

### Why menu clicks don't work

The menu item (`View → Developer → Allow JavaScript from Apple Events`)
is a security-sensitive toggle. Verified experimentally:

- `AXPress` — advertised actions are `[AXCancel, AXPick]`, not
  `AXPress`; Chrome's command dispatch silently discards it.
- `AXPick` on a leaf item — opens submenus correctly but does NOT
  commit a leaf toggle; the item is "selected" but not activated.
- System Events `click theItem` / `click at {x, y}` — returns the
  menu item reference (found it) but Chrome requires a genuine
  trusted user event to flip this flag; synthetic AppleEvent-routed
  clicks are rejected.
- `CGEvent.post(tap: .cghidEventTap)` while the menu is open —
  Chrome's event loop is occupied processing the menu; the event
  either races or Chrome treats it as untrusted for this toggle.

Additionally, when Chrome is **backgrounded**, the Developer submenu
items appear with `AXEnabled = false` (Chrome's `commandDispatch`
marks them DISABLED) — any action dispatched returns `.success` at
the AX layer but is silently discarded. This is the root cause of the
"ghost click" pattern: the driver reports ✅ but nothing changes.

### Correct path — write the Preferences JSON directly

Quit Chrome first, then write the flag, then relaunch:

```bash
# 1. Quit Chrome
osascript -e 'quit app "Google Chrome"'
sleep 1

# 2. Write the flag into the active profile's Preferences.
#    Chrome stores this in TWO places — both must be set.
python3 -c "
import json, os
prefs_path = os.path.expanduser(
    '~/Library/Application Support/Google/Chrome/Default/Preferences')
data = json.load(open(prefs_path))
data.setdefault('browser', {})['allow_javascript_apple_events'] = True
data.setdefault('account_values', {}).setdefault('browser', {})['allow_javascript_apple_events'] = True
json.dump(data, open(prefs_path, 'w'))
print('allow_javascript_apple_events enabled in Default profile')
"

# 3. Relaunch Chrome and wait for sync to stabilise.
#    Chrome sync fires ~1-2 s after launch and may briefly pull
#    an older value from the server before Chrome pushes our local
#    True back. Either test before sync fires (<1 s) or after it
#    settles (>4 s). Waiting exactly ~2 s lands in the race window
#    and is the most likely way to see a false negative.
open -a "Google Chrome"
sleep 5

# 4. Verify
osascript -e 'tell application "Google Chrome"
  tell active tab of front window
    execute javascript "1+1"
  end tell
end tell'
# → 2
```

**Which profile?** Chrome writes to whichever profile is active.
If you're unsure, write to all non-system profiles:

```bash
python3 -c "
import json, glob, os
for p in glob.glob(os.path.expanduser(
        '~/Library/Application Support/Google/Chrome/*/Preferences')):
    profile = p.split('/')[-2]
    if 'System' in profile or 'Guest' in profile:
        continue
    try:
        data = json.load(open(p))
        data.setdefault('browser', {})['allow_javascript_apple_events'] = True
        data.setdefault('account_values', {}).setdefault('browser', {})['allow_javascript_apple_events'] = True
        json.dump(data, open(p, 'w'))
        print(f'wrote to {profile}')
    except Exception as e:
        print(f'skipped {profile}: {e}')
"
```

Chrome overwrites its Preferences file on every clean exit, so the
write must happen while Chrome is **not running** — otherwise Chrome
will stomp the change when it quits.

**Sync note (Chrome only):** Chrome syncs `browser.allow_javascript_apple_events`
via Google account (confirmed in `chrome_syncable_prefs_database.cc`).
Writing both `browser` and `account_values.browser` to the local file
causes Chrome to push `true` to the sync server on next launch,
making the change durable. Brave and Edge use their own sync systems
and likely do NOT sync this Mac-only pref — treat as local-only for
those browsers.

### Brave

Same Chromium pref key, different profile directory:

```bash
osascript -e 'quit app "Brave Browser"' && sleep 1
python3 -c "
import json, glob, os
for p in glob.glob(os.path.expanduser(
        '~/Library/Application Support/BraveSoftware/Brave-Browser/*/Preferences')):
    profile = p.split('/')[-2]
    if 'System' in profile or 'Guest' in profile:
        continue
    try:
        data = json.load(open(p))
        data.setdefault('browser', {})['allow_javascript_apple_events'] = True
        json.dump(data, open(p, 'w'))
        print(f'wrote to {profile}')
    except Exception as e:
        print(f'skipped {profile}: {e}')
"
open -a "Brave Browser" && sleep 5
```

### Edge

Same Chromium pref key, different profile directory:

```bash
osascript -e 'quit app "Microsoft Edge"' && sleep 1
python3 -c "
import json, glob, os
for p in glob.glob(os.path.expanduser(
        '~/Library/Application Support/Microsoft Edge/*/Preferences')):
    profile = p.split('/')[-2]
    if 'System' in profile or 'Guest' in profile:
        continue
    try:
        data = json.load(open(p))
        data.setdefault('browser', {})['allow_javascript_apple_events'] = True
        json.dump(data, open(p, 'w'))
        print(f'wrote to {profile}')
    except Exception as e:
        print(f'skipped {profile}: {e}')
"
open -a "Microsoft Edge" && sleep 5
```

### Safari

Safari uses `do JavaScript "..." in document 1` (different AppleScript
verb from Chrome's `execute javascript`). The setting is under
`Develop → Allow JavaScript from Apple Events` (requires the Develop
menu to be enabled first via Settings → Advanced → "Show features for
web developers").

`defaults write -app Safari AllowJavaScriptFromAppleEvents 1` **does
not work** — Safari ignores the defaults key and routes this toggle
through macOS's security framework, which shows a password-confirmation
dialog. The only working programmatic path is UI automation:

```applescript
-- Requires Accessibility permission for the calling process.
-- Safari must already be running with the Develop menu visible.
tell application "Safari" to activate
delay 0.3
tell application "System Events"
  tell process "Safari"
    click menu item "Allow JavaScript from Apple Events" of menu 1 ¬
      of menu bar item "Develop" of menu bar 1
    delay 0.3
    -- Safari shows a confirmation dialog — click Allow
    click button "Allow" of window 1
  end tell
end tell
```

### Arc

Arc has an AppleScript dictionary and accepts `execute javascript`, but
**never returns a value** — the call always returns `missing value`.
There is no "Allow JavaScript from Apple Events" toggle. The only
workaround is to write results to the clipboard inside the JS and read
it back:

```applescript
tell application "Arc"
  tell active tab of front window
    execute javascript "navigator.clipboard.writeText(document.title)"
  end tell
end tell
delay 0.3
set theTitle to the clipboard
```

Arc's AppleScript JS support is confirmed broken for return values
(as of 2025). If you need JS results from Arc, use a WebExtension
with Native Messaging instead.

### Firefox

Firefox has no `execute javascript` capability. Bugzilla #287447
(filed 2004) tracks this and remains unresolved. Use WebDriver /
Playwright / a WebExtension with Native Messaging for Firefox.

## Typing into a web input

```
type_text({pid, window_id, element_index: <input_field>, text: "…"})
```

If it silently drops (some web inputs don't implement
`AXSelectedText`), `type_text` automatically falls back to CGEvent
synthesis — pure CGEvent keystrokes delivered to the pid, reaching
any focused keyboard receiver. You can also click the field first
to ensure focus before typing.

### `escalation.recommended == "page"` — stop typing, switch tools

On a rich-text contenteditable driven by React/Draft.js (X's tweet
composer is the canonical example), CGEvent keystroke synthesis is
fundamentally racy: a combined px-click-then-type call can drop or
reorder the first character, and `hotkey(["cmd","a"])` / `press_key
delete` frequently no-op against the editor's own selection model.
`type_text` knows this and says so — when its response includes
`escalation: {recommended: "page", reason: "...AX type_text on a
contenteditable is racy..."}`, that is your cue to **stop iterating
on the keyboard ladder immediately.** Do not retry with
`delivery_mode:"foreground"`, do not triple-click to select, do not
try `cmd+a`+`delete` first — every one of those is the wrong rung.
Go straight to the `page` tool instead:

```
page({pid, window_id, action: "execute_javascript", javascript: `
  (function() {
    var el = document.querySelector('[data-testid="tweetTextarea_0"]');
    el.focus();
    document.execCommand('selectAll', false, null);
    document.execCommand('delete', false, null);
    document.execCommand('insertText', false, ${JSON.stringify(text)});
    return el.innerText;
  })()
`})
```

`document.execCommand('insertText', ...)` dispatches the real
`beforeinput`/`input` events Draft.js/React listen for, so it lands
in the framework's own state — not just a raw DOM mutation the
framework will stomp on next render. Swap the selector for whatever
`[data-testid]`/`role="textbox"` the target editor uses; find it via
`page(query_dom)` first if unfamiliar with the site.

**Verification gotcha:** if the window is occluded or off-screen
when you re-snapshot, `get_window_state`'s screenshot can return a
stale cached frame while the DOM has actually already updated —
`list_windows` showing `is_on_screen: false` for the target is the
tell. Don't conclude the insert failed off a stale screenshot; verify
with a follow-up `page(execute_javascript)` reading `el.innerText`
directly, which reads the live DOM regardless of on-screen state.

**`execCommand` is not durable on editors that reconcile their own
state.** Confirmed on X's composer: `insertText` lands in the DOM
immediately (verified via `el.innerText` right after), but a later
background re-render of the surrounding page (e.g. the timeline
polling in new posts) can silently wipe it back to empty — because
it was one synthetic event, not something the editor's own input
pipeline ever actually observed as a keystroke. If you see the text
vanish on a follow-up read with no error anywhere, that's this,
not a transient bug — reach for `insert_text` or `type_keystrokes`
(below) instead of retrying `execCommand` in a loop.

### `insert_text` — a cheaper, more durable middle rung

**macOS-only for now.** Windows and Linux don't yet implement the CDP
`Input.insertText`/`Input.dispatchKeyEvent` calls this and
`type_keystrokes` need — both return a clean "not implemented" error
there, not a silent no-op. Tracked in
[trycua/cua#2084](https://github.com/trycua/cua/issues/2084). Every
other `page` action (`get_text`, `query_dom`, `click_element`,
`execute_javascript`) works cross-platform.

```
page({pid, window_id, action: "insert_text", text: "…"})
```

Inserts `text` at the focused element in a single CDP
`Input.insertText` call — no synthesized keydown/keyup sequence, so
it's one round-trip instead of three-per-character. It's still more
durable than `execCommand`, though, because Chrome's renderer treats
it like an IME composition commit rather than a JS-triggered DOM
mutation, and rich editors already have to handle real IME commits
correctly (CJK input, etc.) — so they're less likely to treat it as
untrusted and discard it. It does skip real key events, so an
editor's own keydown/keyup handlers (shortcut bindings, per-key
validation) never fire for it.

Try this before `type_keystrokes` when `execCommand` got discarded.
Escalate to `type_keystrokes` only if `insert_text` also gets wiped,
or the editor specifically needs to observe real keystrokes.

Same preconditions as `type_keystrokes` below (focus + live CDP
port on a non-default profile).

### `type_keystrokes` — the slowest, most durable rung

```
page({pid, window_id, action: "type_keystrokes", text: "…"})
```

Types `text` as a stream of real per-character keyboard events
(`Input.dispatchKeyEvent`: keyDown → char → keyUp per character) via
the Chrome DevTools Protocol, rather than one synthetic DOM write.
This is what makes it durable against React/Draft.js/Lexical/Slate
editors that reconcile their own internal model on every render —
they only trust input their own keydown/keypress/input pipeline
actually saw, and `execCommand`/`innerText =` never touches that
pipeline. Slower than `insert_text` (3 CDP round-trips per
character, with a small inter-character delay) — reach for it only
after `insert_text` didn't stick, or the editor needs real keydown
events.

Preconditions (shared with `insert_text`):

- The target element must already have DOM focus — click it first
  (`page(click_element)` or `execute_javascript` with `el.click()` /
  `el.focus()`).
- The browser needs a live CDP port. Two supported sources are:
  - **Driver-owned automation profile** — call the approved
    `browser_prepare` operation with `allow_launch:true` and an
    `isolated_new` or `isolated_named` profile. It launches a separate
    process and returns `prepared_pid`. It never copies or modifies the
    user's normal profile.
  - **The user's real, already-logged-in Chrome** — have them open
    `chrome://inspect/#remote-debugging` and check "Allow remote
    debugging". Confirmed this opens a CDP port on the _default_
    profile without relaunching anything, but it doesn't serve the
    classic `/json` HTTP discovery (confirmed `/json/version` returns 404) — `insert_text`/`type_keystrokes` fall back automatically to
    connecting straight to the browser-level `ws://host:port/devtools/browser`
    endpoint and `Target.attachToTarget{flatten:true}` in that case,
    but since that path can't be auto-discovered via `pid` + `lsof`
    (no working `/json` to validate against), pass the port explicitly
    via the legacy `cdp_port` argument (commonly 9222, but whatever the chrome://inspect
    page shows) — auto-discovery only works for the dedicated-profile
    route above.
    **This can't be made fully unattended, and the popup fires more
    often than you'd think.** Chrome shows a live "Allow remote
    debugging?" confirmation on every _new_ WebSocket connection to the
    browser endpoint — not once per Chrome process, and NOT something a
    saved preference can skip. Confirmed by testing live, twice, with
    different results depending on what actually opened a new socket:
    - Naively calling `insert_text`/`type_keystrokes` fresh each time
      (each one connecting from scratch) got prompted on **every single
      call, 3 for 3** — worse than "once per session."
    - `CdpSessionCache` (this platform's backend) fixes that: it caches
      one open connection per port and reuses it across calls,
      re-attaching to a different tab on the _same_ connection via
      `Target.getTargets`/`Target.attachToTarget` when
      `target_url_contains` points somewhere new — flattened-mode
      re-attach doesn't open a new socket, so it doesn't re-prompt.
      Confirmed live: after the first approval, two more calls (typing
      into the same tab) went through silently in well under a second.
    - The popup _does_ come back the moment the cache has to open a
      genuinely new connection — confirmed by closing the tab the
      cached session was attached to: the next call took ~2s (evict +
      reconnect) and needed a fresh click. Also expect this after a
      full Chrome restart, or a daemon restart (the cache is in-memory,
      not persisted).
      Net: one click to start, silent after that for as long as the
      browser and the daemon both stay up and you're re-attaching rather
      than reconnecting — but don't promise a caller "no more prompts,
      ever" for a long session that outlives either of those.
      If no CDP port is found (and none was given explicitly), both
      actions fail fast with an actionable error rather than silently
      no-op'ing.
- Multi-tab ambiguity: with more than one tab open, both actions act
  on whichever tab is found first — not necessarily the one you
  mean — unless you pass `target_url_contains` (a substring to match
  against the tab's URL, e.g. `"x.com/compose"`). Always pass this when
  the browser might have more than one tab open, which in practice is
  every real, non-disposable-profile session. Be as specific as the
  URL allows — `"x.com"` alone matched BOTH a compose tab and a
  notifications tab in testing, and `pick_target` just took whichever
  came first; `"x.com/compose"` was needed to disambiguate.

Prefer `execCommand` first for plain `<input>`/`<textarea>` fields
(cheaper, no CDP-port precondition); reach for `type_keystrokes`
specifically when `execCommand` landed but didn't stick, or when
you're driving a known rich-text composer up front.
