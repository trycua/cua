# cua-driver — Claude Code skill

A [Claude Code](https://code.claude.com) skill that teaches Claude to
drive native macOS apps via the
[`cua-driver`](https://github.com/trycua/cua/tree/main/libs/cua-driver)
CLI — snapshot an app's accessibility tree, click/type/scroll by
`element_index`, and verify via re-snapshot. Backgrounded-first: no
focus steal, no cursor warp, no Space follow.

## What the skill covers

- The snapshot-before-AND-after invariant that keeps the agent honest
  about whether an action actually landed.
- The backgrounded-click recipe (yabai focus-without-raise + stamped
  SLEventPostToPid) that lets synthetic clicks land on Chrome web
  content without raising the window or pulling the user across Spaces.
- Web-app quirks (`WEB_APPS.md`) — Chromium/WebKit/Electron/Tauri,
  including the minimized-Chrome keyboard-commit caveat and the
  `set_value` workaround.
- Trajectory recording (`RECORDING.md`) — optional per-session
  recording + replay for demos and regressions.
- Canvas/viewport apps (Blender, Unity, GHOST, Qt, wxWidgets) —
  HID-tap fallback when AX is empty.

See `SKILL.md` for the main body.

## Prerequisites

1. **macOS 14 or newer** — the driver depends on SkyLight private SPIs
   that were stabilized in Sonoma.
2. **`cua-driver` CLI + `CuaDriver.app`** — installable one-liner:
   ```bash
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
   ```
   Or from a clone of `trycua/cua`:
   ```bash
   cd libs/cua-driver
   scripts/install-local.sh   # builds + installs + symlinks for dev use
   ```
   The driver runs as an `.app` bundle because macOS TCC grants are
   tied to a stable bundle id (`com.trycua.driver`). The CLI symlink
   lets Claude invoke tools via plain shell.
3. **TCC grants on `CuaDriver.app`** — **Accessibility** and
   **Screen Recording** in System Settings → Privacy & Security.
   Verify with:
   ```bash
   cua-driver check_permissions
   ```
   Both fields must be `true`. If not, the app appears in the
   relevant panes of System Settings after first use; toggle it on
   there.

## Install

The skill is two drop-in directories.

**Personal scope** (all Claude Code sessions on your machine):

```bash
mkdir -p ~/.claude/skills
cp -R Skills/cua-driver ~/.claude/skills/
```

Or symlink if you want edits-in-place:

```bash
ln -s "$PWD/Skills/cua-driver" ~/.claude/skills/cua-driver
```

**Project scope** (committed alongside a specific repo):

```bash
mkdir -p .claude/skills
cp -R /path/to/cua/libs/cua-driver/Skills/cua-driver .claude/skills/
```

## Invoking the skill

Claude Code auto-invokes the skill when you ask for macOS GUI
automation — e.g. "open the Downloads folder in Finder", "click the
Save button in Numbers", "navigate to trycua.com in Chrome". You can
also invoke it explicitly:

```
/cua-driver
```

## Claude Code MCP compatibility mode

For normal skill-driven use, prefer the CLI or the standard MCP server. If you want Claude Code's vision/computer-use-style flow to ground on CuaDriver screenshots, register the compatibility server:

```bash
claude mcp add --transport stdio cua-computer-use -- cua-driver mcp --claude-code-computer-use-compat
```

This mode exposes the normal CuaDriver tools and changes only `screenshot`. The compatibility screenshot requires `pid` and `window_id`, captures that window only, and establishes a window-local pixel coordinate frame. It does not call Anthropic APIs or expose Anthropic's native computer-use API tool.

Use MCP for this Claude Code vision/computer-use-style path. CLI screenshots still work as CuaDriver calls, but they do not expose the `mcp__cua-computer-use__screenshot` tool name that Claude Code appears to use as the image-grounding cue.

## Files

- `SKILL.md` — the main skill body (~500 lines). Loaded on first
  invocation; stays in context for the session.
- `WEB_APPS.md` — browsers, Electron, Tauri (Chromium + WebKit). Loaded
  on demand when SKILL.md's pointer is followed.
- `RECORDING.md` — trajectory recording / replay. Loaded on demand.
- `TESTS.md` — manual test scripts for end-to-end skill verification.

## Troubleshooting

- `cua-driver: command not found` → re-run the installer or add
  `.build/CuaDriver.app/Contents/MacOS/` to `$PATH`.
- `No cached AX state for pid X window_id W` → element_index was
  reused across turns, or across different windows of the same app.
  Call `get_window_state({pid, window_id})` first in the same turn,
  with the same window_id you're about to act against.
- Empty `tree_markdown` → `capture_mode` is set to `vision`, which
  skips the AX walk by design. Flip back to the default `som`
  (`cua-driver config set capture_mode som`) to get the tree.
  Tiny screenshot → likely a stale window capture. See "Behavior
  matrix" in SKILL.md for the full mode table.
- System-alert beep when pressing Return on a minimized Chrome
  omnibox → the keyboard-commit-on-minimized limitation. Use
  `set_value` on the field instead, or AX-click a Go/Submit button.
  See `WEB_APPS.md`.

## Updates

The skill evolves alongside the driver. To update:

```bash
cd /path/to/cua && git pull
# if you copied: re-copy
cp -R libs/cua-driver/Skills/cua-driver ~/.claude/skills/
# if you symlinked: nothing needed
```

## License

MIT. Same license as the parent `trycua/cua` repo.
