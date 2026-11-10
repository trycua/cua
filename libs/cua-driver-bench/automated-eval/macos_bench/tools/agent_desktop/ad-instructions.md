---
name: agent-desktop
tags: computer-use, desktop-automation, accessibility, ai-agent, gui-automation, cli
requirements:
  - agent-desktop
description: >
  Operate desktop applications on macOS, Windows and Linux through native accessibility trees,
  using the agent-desktop CLI. Use when an AI agent must see or act on a desktop GUI:
  click buttons, fill forms, read UI state, navigate menus, toggle checkboxes, scroll,
  drag, type text, press shortcuts, take screenshots, manage windows, use the clipboard,
  launch or close apps, read notifications.
  Triggers on: "click button", "fill form", "open app", "read UI", "computer use",
  "operate desktop", "accessibility tree", "snapshot app", "type into field", "navigate menu",
  "toggle checkbox", "take screenshot", "desktop automation", "agent-desktop",
  or any desktop GUI task on macOS, Windows or Linux.
---

# agent-desktop

agent-desktop is a CLI that lets an AI agent observe and operate desktop apps through the OS accessibility tree. It is a tool, not an agent. Every command prints one JSON envelope (`ok`, `data` or `error`). The observe, act and verify loop runs in you.

## Shell syntax

Examples use a POSIX shell. In PowerShell, quote every ref (`'@s8f3k2p9:e1'`), because a bare `@token` is the splatting operator. Use `$env:NAME = 'value'` instead of `export`, and `| ConvertFrom-Json` instead of `$(...)` and `jq`. The Windows skill has the PowerShell forms.

## First run

1. Run `agent-desktop status`. It reports the platform, permissions and `supported_surfaces`.
2. Run `agent-desktop skills get platform`. It loads the skill for the OS the binary runs on (macOS, Windows or Linux).
3. For flags and syntax, run `agent-desktop <command> --help`. This skill does not repeat flag lists.

## The loop

Use `find` when you know the role or exact name. Otherwise map the app first and drill into one region.

```
agent-desktop snapshot --skeleton --app "Notes" -i --compact   # shallow map, children_count on cut branches
agent-desktop snapshot --root @s8f3k2p9:e3 -i --compact        # drill into one region
agent-desktop click @s8f3k2p9:e12                              # act on a qualified ref
agent-desktop snapshot --root @s8f3k2p9:e3 -i --compact        # verify: re-read the same region
```

- A small app needs no skeleton. Take a full `snapshot -i`.
- Never skeleton a surface (`--surface menu`, `sheet`, `alert`). A surface is already focused.
- An action that opens a menu, sheet or alert lists it in `data.surfaces`. Target that overlay next.
- After an async step, use `wait` (see `commands-system.md`). A fixed sleep is a race.

## Refs

- Output refs are qualified: `@<snapshot_id>:e<N>`, for example `@s8f3k2p9:e1`. Pass them to later commands as they are.
- Refs are valid for one snapshot. After any UI change, re-drill the affected region or take a new snapshot.
- Only addressable elements get refs. Static text and inert containers do not.
- `STALE_REF`: the ref no longer matches the live UI. Take a fresh snapshot and use the new ref.
- `AMBIGUOUS_TARGET`: more than one live element matches. Narrow the target with `find --role --name --exact` or `--root`, then use the new ref.
- `SNAPSHOT_NOT_FOUND`: the snapshot id expired. Snapshot again.

## Delivery and retry

Action results carry `data.disposition` and errors carry `error.disposition`, each with `delivery` and `retry`. Observation commands such as `snapshot` have none on success; their errors, and every `INVALID_ARGS` refusal, carry `not_delivered` and `retry: safe`.

- Retry or substitute an action only when `disposition.retry` is `safe`. Only `not_delivered` is safe.
- Any other delivery value means the action may have landed. A repeat can click, type or submit twice, and `toggle` can undo itself.
- On a result that is not safe, read the live state (`get`, `is`, or a fresh snapshot) and decide from that. `details.post_state` helps when present.
- Branch on `error.code` and `error.disposition`. Message and suggestion text can change.
- `details.retryable: false` means waiting cannot change the outcome. Try a different approach.

## Headless and headed

Ref actions are headless by default. They use semantic accessibility calls and do not take focus, move the cursor, synthesize keys or use the clipboard. If no semantic path exists, the action fails closed. Use the global `--headed` flag only when the user explicitly permits physical input, such as double-click, hover, drag, or focus. Never switch to headed mode to recover from a headless failure. Raw coordinates (`--xy`) never imply focus. Which actions need `--headed` differs by OS: read the platform skill.

## References

Load one only when you need it: `agent-desktop skills get desktop <reference>`.

| Reference | Open it when |
|-----------|--------------|
| `commands-observation` | You need `snapshot`, `find`, `get`, `is`, `screenshot`, `list-surfaces` semantics, partial trees, or the `--root` rules |
| `commands-interaction` | You choose between click, type, set-value, select, toggle, scroll, press and mouse commands, or you debug an action error |
| `commands-system` | You launch or close apps, manage windows, use the clipboard, `wait`, `batch`, sessions, multi-agent cursors, tracing, or load skills |
| `workflows` | You want worked patterns (forms, menus, dialogs, waiting for a new window, scroll-find, drag) and the anti-patterns list |


---
---
name: agent-desktop-macos
tags: macos-automation, accessibility, ax, tcc, ai-agent, gui-automation, cli
requirements:
  - agent-desktop
description: >
  macOS platform guide for the agent-desktop CLI. Use for desktop automation
  on macOS: granting Accessibility and Screen Recording permission (TCC),
  what headed mode does on macOS (physical-first click, type and scroll),
  surfaces (menu, menubar, sheet, popover, alert), Notification Center,
  Chromium and Electron apps including launch --cdp, and fixing PERM_DENIED,
  empty accessibility trees, ACTION_FAILED and APP_UNRESPONSIVE.
---

# agent-desktop on macOS

Read the core skill first (`agent-desktop skills get desktop`) for the observe, act, verify loop. This skill covers only what is different on macOS. Symptoms and fixes are in [references/troubleshooting.md](references/troubleshooting.md).

## Requirements

- macOS 13 or later, Intel or Apple Silicon.
- Accessibility permission for the app that launches agent-desktop (Terminal, iTerm2, Warp, VS Code, Codex). macOS 27 and later name this list Device Control and Data Access. If System Settings lists the `agent-desktop` binary separately, grant it too.
- Screen Recording permission, only for `screenshot` and for `session start --screenshots` artifacts.
- Automation permission for System Events, only where a command reports it missing.

## First contact

1. Run `agent-desktop permissions`.
2. If a permission is denied, run `agent-desktop permissions --guide`. It opens the correct System Settings page with a panel that shows the user which app to drag into the list. The user confirms with Touch ID or a password, and the panel moves to the next missing permission by itself. The command returns at once with `"guide": "started"`, `"already_running"` or `"not_needed"`. `agent-desktop permissions --request` shows the system prompts instead.
3. If the guide cannot run, open System Settings > Privacy & Security > Accessibility (Device Control and Data Access on macOS 27 and later). Add the launching app and turn it on.
4. Run `agent-desktop permissions` to confirm `granted`. The next agent-desktop call sees a new grant, so the launching app does not need a restart.
5. Run `agent-desktop snapshot -i` to confirm. It reads the focused app. `snapshot --app Finder -i` returns `WINDOW_NOT_FOUND` when Finder has no open window.

`permissions` does not prompt. `permissions --request` can prompt, including for Automation. `permissions --guide` does not prompt and covers Accessibility and Screen Recording only. Automation reports `unknown` when macOS would need to prompt.

## What differs on macOS

**Headed is physical first** for `click`, `right-click`, `type`, `clear` and `scroll`. Core focuses the exact window of the ref. The adapter then sends a real click, key or wheel event to the verified point. `expand`, `collapse`, `set-value`, `select`, `toggle`, `check`, `uncheck`, `focus` and `scroll-to` stay semantic under `--headed`. Double-click, triple-click, `hover` and `drag` need `--headed`. Raw `--xy` input never changes focus.

**Judge delivery by observation.** AX return codes do not prove an effect. Finder returns success from an `AXConfirm` that does nothing. Snapshot again or use `get` and `is` to check the result.

**Surfaces.** Use `--surface menubar` for File, Edit and View menus and for menu bar apps (status items). While a menu, sheet, alert or popover is open, refs from the window tree behind it are the wrong target.

**Menus.** `right-click` headless uses `AXShowMenu`. If the menu opened but the app returned `APP_UNRESPONSIVE` with `retry: unsafe`, do not retry. Inspect the menu with `--surface menu`. For combo boxes and menu buttons, use `select @ref "Option"`. `select` needs a closed-to-open menu transition, so dismiss any open menu first.

**Notification Center.** The notification commands drive Notification Center through accessibility, with the rules in the core `commands-system` reference. When agent-desktop opens the center, it closes it afterwards and restores the previous app focus. Pass both `--expected-app` and `--expected-title` on a single dismiss or action, because the stack shifts. Use `wait --notification --app "App"` for banners that vanish before you list them.

**Chromium and Electron.** agent-desktop switches on the web accessibility of a Chromium renderer automatically when it needs it. For a dense running app, use `snapshot --skeleton -i --compact`, then drill with `--root @ref`.

**Identity.** `launch` accepts a display name (`"System Settings"`) or a bundle ID (`com.apple.systempreferences`). Other commands take the display name through `--app`. Window IDs such as `w-4521` stay valid for the life of the window. Find them with `list-windows`.

**Keys.** The primary modifier is `cmd`: `press cmd+c`, `press cmd+s`. Quit shortcuts such as `cmd+q`, `cmd+shift+q`, `cmd+alt+esc`, `ctrl+cmd+q` and `cmd+shift+delete` return `POLICY_DENIED` unless you pass `--force`. To quit an app, use `close-app`. Punctuation keys work in combos, as the symbol (`,` `.` `/` `;` `'` `[` `]` `\` `-` `=` `` ` ``) or by name (`comma`, `period`, `slash`, `semicolon`, `quote`, `leftbracket`, `rightbracket`, `backslash`, `minus`, `equal`, `grave`). Each one is sent with the key that types it in the active keyboard layout, so `press cmd+,` opens Settings on ANSI and JIS keyboards alike.

**Cursor overlay.** `cursor-overlay enable` draws the agent cursor in a click-through window. It never moves the OS pointer or activates an app. The macOS motion and image limits are in the core `commands-system` reference.

## Next step

When a command fails, find the symptom in [references/troubleshooting.md](references/troubleshooting.md).

