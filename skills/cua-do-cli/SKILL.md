---
name: cua-do-cli
description: Automates interactions with remote VMs, local Docker containers, cloud sandboxes, and the host PC via cua do. Use when navigating GUIs, taking snapshots/screenshots, clicking, typing, scrolling, running shell commands, or managing windows on a target machine.
---

# VM Automation with cua do

One command = one action = one ✅/❌ line. Switch targets once, then operate.

## Start Here

1. Pick a flow:
   - API key available: switch → snapshot → interact → snapshot
   - No API key: switch → screenshot → read image → interact → screenshot
   - Host control: consent → switch host → operate
2. Run the canonical flow below.
3. Check the command skeleton only if blocked.

## Decision Map

- No target set: `switch` → then operate
- **ANTHROPIC_API_KEY set** (normal): `switch` → `snapshot` → `click/type/key` → repeat snapshot after each UI change
- **No API key**: `switch` → `screenshot` → read image to determine coords → `click/type/key` → repeat screenshot
- Zoom to one window: `zoom "App Name"` → operate → `unzoom`
- Run shell: `shell "command"`
- Control host PC: `do-host-consent` (once) → `switch host` → operate

## Canonical Flows

### 1) Normal (ANTHROPIC_API_KEY set)

```bash
cua do switch docker my-container
cua do snapshot                        # AI summary + interactive elements with coords
cua do click 450 300
cua do type "hello"
cua do key enter
cua do snapshot                        # re-snapshot after UI change
```

### 2) No API Key

```bash
cua do switch docker my-container
cua do screenshot                      # save path printed; read the image to find coords
cua do click 450 300
cua do type "hello"
cua do key enter
cua do screenshot                      # re-screenshot after UI change
```

### 3) Focused Window (Zoom)

```bash
cua do zoom "Chrome"
cua do snapshot                        # cropped to Chrome; coords are window-relative
cua do click 120 80
cua do unzoom
```

### 4) Host PC Control

```bash
cua do-host-consent                    # one-time consent, no prompt
cua do switch host
cua do snapshot
cua do click 100 200
```

## Command Skeleton

### Target

```bash
cua do switch <provider> [name]        # cloud, docker, lume, lumier, winsandbox, host
cua do status
cua do ls [provider]
```

### Snapshot / Screenshot

```bash
cua do snapshot ["extra instructions"] # screenshot + AI summary (needs ANTHROPIC_API_KEY)
cua do screenshot [--save path]
cua do zoom "Window Name"
cua do unzoom
```

### Input

```bash
cua do click <x> <y> [left|right|middle]
cua do dclick <x> <y>
cua do move <x> <y>
cua do type "text"
cua do key <key>                       # enter, escape, tab, space, f1–f12, …
cua do hotkey ctrl+c
cua do scroll <up|down|left|right> [n]
cua do drag <x1> <y1> <x2> <y2>
```

### Shell / Open

```bash
cua do shell "command"
cua do open <url|path>
```

### Window

```bash
cua do window ls [app]
cua do window focus <id>
cua do window unfocus
cua do window minimize/maximize/close <id>
cua do window resize <id> <w> <h>
cua do window move <id> <x> <y>
cua do window info <id>
```

## Guardrails

- Re-snapshot after navigation, modals, or list changes — coords go stale.
- `snapshot` needs `ANTHROPIC_API_KEY`; use `screenshot` otherwise.
- Coords are image-space: zoom + max-length scaling are applied automatically.
- `do-host-consent` is permanent until the consent file is deleted (`~/.cua/host_consented`).
- Set `PYTHONIOENCODING=utf-8` on Windows for correct emoji output.
