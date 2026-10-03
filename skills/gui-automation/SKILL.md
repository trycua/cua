---
name: gui-automation
description: Operate a GUI by sight with the `cua do` CLI: take screenshots, click, type, drag, manage windows on a sandbox, a spacesd URL or this machine. Use for visual testing, form filling and flows that shell or API cannot do.
---

# GUI automation (`cua do`)

Look, act, verify. Coordinates are screenshot pixels and go stale when the screen changes, so screenshot after every change.

## Connect

```bash
cua --version                                   # missing: curl -fsSL https://cua.ai/install.sh | sh
cua do switch my-sandbox                        # a sandbox from `cua sb ls`
cua do switch url http://127.0.0.1:3211 --as dev  # a cua-spacesd by URL
cua do-host-consent && cua do switch host       # this machine (asks the user once)
```

## Loop

```bash
cua do screenshot
cua do click 450 280
cua do type "Jane Doe" && cua do key tab
cua do screenshot
cua trajectory view   # at the end: replay for the user
```

Small targets: `cua do zoom "Google Chrome"` makes coordinates window-relative; `cua do unzoom` restores them.

`cua do snapshot ["what to find"]` returns an annotated screen with coordinates when `ANTHROPIC_API_KEY` is set.

## Commands

| Do | Command |
|---|---|
| Click, double-click | `click <x> <y> [left\|right\|middle]`, `dclick <x> <y>` |
| Type, key, hotkey | `type "text"`, `key <key>`, `hotkey ctrl+c` |
| Scroll, drag, move | `scroll <dir> [n]`, `drag <x1> <y1> <x2> <y2>`, `move <x> <y>` |
| Shell, open | `shell "cmd"`, `open <url\|path>` |
| Windows | `window ls [app]`, `window focus <id>` |
| Skip recording | `cua do --no-record <cmd>` |

Every action is recorded to `~/.cua/trajectories/`; `cua trajectory ls` prints the path. Full syntax: [references/command-reference.md](references/command-reference.md).
