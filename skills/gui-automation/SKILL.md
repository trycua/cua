---
name: gui-automation
description: >-
  Use when you need to visually interact with a GUI: test buttons, fill forms,
  verify visual layouts, fuzz web pages, automate user flows, take screenshots,
  or perform end-to-end QA on any application. Works on cloud VMs, Docker
  containers, local machines, and sandboxes. Needs the `cua` CLI
  (curl -fsSL https://cua.ai/install.sh | sh).
---

# GUI Automation

CUA gives you **eyes and hands on a real computer**: see the screen, move the
mouse, click, type, drag, and manage windows, like a human at the keyboard.

Use this skill for **visual interaction** that can't be done via shell or API.

## Setup

```bash
cua --version          # check install; if missing: curl -fsSL https://cua.ai/install.sh | sh

# Connect to target (pick one)
cua do switch my-sandbox                    # any sandbox from `cua sb ls`
cua do switch url http://127.0.0.1:3211 --as dev   # a cua-spacesd by URL
cua do-host-consent && cua do switch host   # local machine (one-time consent)
```

> `ANTHROPIC_API_KEY` is optional. With it, `cua do snapshot` returns an
> AI-annotated screen with element coordinates. Without it, use `screenshot`
> and read the image yourself.

## Workflow

**Look → Act → Verify**: repeat until done, then share:

```bash
cua do screenshot          # look
cua do click 450 280       # act
cua do screenshot          # verify
cua trajectory view        # open the replay for the user
```

> Re-screenshot after every UI change: coordinates go stale when the screen changes.

## Scenarios

### Click a button

```bash
cua do screenshot
cua do click 450 280
cua do screenshot
```

### Fill a form

```bash
cua do screenshot
cua do click 400 200 && cua do type "Jane Doe"
cua do key tab            && cua do type "jane@example.com"
cua do key tab            && cua do type "SecureP@ss123"
cua do click 400 500
cua do screenshot
```

### File upload dialog

```bash
cua do click 350 400       # "Choose File"
cua do type "/home/user/report.pdf"
cua do key enter
cua do screenshot
```

### Zoom in for precision clicks (host or small targets)

When clicking small or dense UI elements, especially on the host machine,
zoom into the target window first. Coordinates become **window-relative** and
screenshots show only that window, giving you higher effective resolution.

```bash
cua do zoom "Google Chrome"   # crop to Chrome window; coords are now window-relative
cua do screenshot              # zoomed view, easier to locate small elements
cua do click 112 44            # precise click on a small tab or button
cua do screenshot              # verify
cua do unzoom                  # restore full-screen coords when done
cua do screenshot              # back to full desktop view
```

> Use `zoom` any time click accuracy is uncertain. `unzoom` before switching
> windows or when you need to see the full desktop again.

### Drag and drop

```bash
cua do window ls               # list open windows
cua do drag 150 300 650 400    # source → destination
cua do screenshot
```

### Fuzz a form

```bash
cua do screenshot
cua do click 400 200
cua do type "<script>alert(1)</script>"
cua do key tab && cua do type "'; DROP TABLE users; --"
cua do key tab && cua do type "AAAAAAAAAAAAAAAAAAAAAAA"
cua do click 400 500
cua do screenshot              # check for errors, crashes, unexpected behavior
```

## Trajectory

Every action is auto-recorded to `~/.cua/trajectories/{machine}/{session}/`.

```bash
cua trajectory view            # open the replay viewer (do this at the end)
cua trajectory ls              # list sessions
cua trajectory clean           # delete sessions
cua do --no-record click 100 200   # disable recording for a single action
```

Tell the user where the trajectory is (`cua trajectory ls` prints the path).

## Quick Reference

| Action              | Command                                      |
| ------------------- | -------------------------------------------- |
| Connect to target   | `cua do switch <sandbox>`                    |
| Screenshot          | `cua do screenshot`                          |
| AI-annotated screen | `cua do snapshot ["instructions"]`           |
| Click               | `cua do click <x> <y> [left\|right\|middle]` |
| Double-click        | `cua do dclick <x> <y>`                      |
| Type text           | `cua do type "text"`                         |
| Press key           | `cua do key <key>`                           |
| Hotkey              | `cua do hotkey <combo>` (e.g. `ctrl+c`)      |
| Scroll              | `cua do scroll <direction> [amount]`         |
| Drag                | `cua do drag <x1> <y1> <x2> <y2>`            |
| Move cursor         | `cua do move <x> <y>`                        |
| Shell command       | `cua do shell "command"`                     |
| Open URL/file       | `cua do open <url\|path>`                    |
| List windows        | `cua do window ls [app]`                     |
| Focus window        | `cua do window focus <id>`                   |
| Zoom to window      | `cua do zoom "App Name"`                     |
| Unzoom              | `cua do unzoom`                              |
| View trajectory     | `cua trajectory view`                        |

## Providers

| Target            | Example                                      |
| ----------------- | -------------------------------------------- |
| a sandbox         | `cua do switch my-sandbox` (see `cua sb ls`) |
| a spacesd URL | `cua do switch url http://host:3211 --as x`  |
| this machine      | `cua do switch host`                         |

Legacy `<provider> <name>` pairs (`cloud`, `docker`, `lume`, ...) still work.

See [references/command-reference.md](references/command-reference.md) for full argument syntax.
