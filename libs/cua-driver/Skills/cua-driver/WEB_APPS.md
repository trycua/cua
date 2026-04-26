# Driving web-rendered apps

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
3. For typing into web inputs where `type_text` silently drops
   (input doesn't implement `AXSelectedText`), use `type_text_chars`
   — pure CGEvent keystrokes reach any focused keyboard receiver,
   including Unicode / emoji.
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
type_text_chars({pid, text: "https://cua.ai", delay_ms: 30})
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
Only *activating* a specific tab is visible.

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
  `type_text_chars URL` + `press_key return` on minimized Chrome —
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

| Browser | `execute javascript` supported | Setting needed | Programmatic path |
|---|---|---|---|
| Chrome | ✅ Full | ✅ Yes | Edit Preferences JSON (see below) |
| Brave | ✅ Full | ✅ Yes | Edit Preferences JSON (same key, different path) |
| Edge | ✅ Full | ✅ Yes | Edit Preferences JSON (same key, different path) |
| Safari | ✅ Full (`do JavaScript`) | ✅ Yes | UI automation only — `defaults write` broken |
| Arc | ⚠️ No return values | No toggle | No reliable path |
| Firefox | ❌ Not supported | N/A | N/A |

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
`AXSelectedText`), click the field first, then use
`type_text_chars({pid, text})` — pure CGEvent keystrokes delivered
to the pid, reaching any focused keyboard receiver.
