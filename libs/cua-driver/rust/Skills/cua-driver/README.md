# cua-driver-rs — Claude Code skill

A [Claude Code](https://code.claude.com) skill that teaches Claude to
drive native GUI apps on **macOS, Windows, and Linux** via the
[`cua-driver`](https://github.com/trycua/cua/tree/main/libs/cua-driver/rust)
CLI — snapshot an app's accessibility tree (AX on macOS, UIA on
Windows, AT-SPI on Linux), click/type/scroll by `element_index` or
pixel coords, and verify via re-snapshot. Backgrounded-first on every
platform: no focus steal, no cursor warp.

## Platform reading order

- `SKILL.md` — shared core + macOS-specific patterns (read this
  first on macOS).
- `WINDOWS.md` — Windows-specific carve-out (UIA tree, UWP /
  ApplicationFrameHost, layered UIA+PostMessage click chain,
  Session 0 isolation, Windows web-apps section). Read this when
  driving on Windows.
- `LINUX.md` — Linux carve-out (X11 background input via AT-SPI +
  XSendEvent, recording, Wayland opt-in/preview). Read this when
  driving on Linux.
- `EMBEDDING.md` — embedding cua-driver inside another macOS app
  (agent harness) so the driver inherits the host's Accessibility +
  Screen Recording grants with zero extra prompts. Read this when
  integrating the driver into your own app rather than running it
  standalone.

## What the skill covers

- The snapshot-before-AND-after invariant that keeps the agent honest
  about whether an action actually landed.
- The backgrounded-click recipe that lets synthetic clicks land on
  web content without raising the window or pulling the user across
  virtual desktops. Per-platform mechanisms:
  - **macOS**: yabai focus-without-raise + stamped `SLEventPostToPid`.
  - **Windows**: layered UIA `Invoke` + `PostMessage` fallback, both
    z-order-independent and focus-steal-free.
  - **Linux**: AT-SPI `do_action` for element clicks +
    `XSendEvent(ButtonPress)` to the window for pixel clicks — both
    background, no raise, no real-pointer move.
- Web-app quirks — macOS specifics in `WEB_APPS.md` (Chromium / WebKit
  / Electron / Tauri, minimized-Chrome keyboard-commit caveat,
  `set_value` workaround). Windows web-apps coverage lives in
  `WINDOWS.md`'s "Web apps on Windows" section.
- Trajectory recording (`RECORDING.md`) — optional per-session
  recording + replay for demos and regressions. macOS and Linux
  (X11) today; Wayland video capture not yet supported.
- Canvas/viewport apps (Blender, Unity, GHOST, Qt, wxWidgets) —
  fallback paths when the AX/UIA/AT-SPI tree is empty.

See `SKILL.md` for the main body and platform-specific carve-outs for
forbidden-list / launch / click details.

## Prerequisites

### macOS

1. **macOS 14 or newer** — the driver depends on SkyLight private SPIs
   that were stabilized in Sonoma.
2. **`cua-driver` CLI + `CuaDriver.app`** — installable one-liner:
   ```bash
   /bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
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

   There are exactly two supported macOS identity models:
   - **Standalone:** launch the installed app daemon with
     `open -n -g -a CuaDriver --args serve`; grants belong to
     `CuaDriver.app` (`com.trycua.driver`). The installed CLI may
     auto-launch and proxy through this daemon.
   - **Embedded:** the app that owns the grants must spawn
     `cua-driver mcp` directly with `CUA_DRIVER_EMBEDDED=1` (or
     `--embedded`), so the child remains in that app's TCC
     responsibility chain. See `EMBEDDING.md`.

   A raw binary launched outside `CuaDriver.app` without embedded mode
   is unsupported because it has no stable bundle identity for TCC
   attribution. Do not grant permissions to arbitrary binary paths or
   use that configuration in production.

### Windows

1. **Windows 10/11** (any edition with PowerShell 5.1+).
2. **`cua-driver` CLI (Rust port `cua-driver-rs`)** — one-liner:
   ```powershell
   irm https://cua.ai/driver/install.ps1 | iex
   ```
   No TCC equivalent; no UAC elevation required for the default
   per-user install. See `WINDOWS.md` for Session 0 vs Session 1+
   caveats — the daemon MUST run in an interactive session, not via
   SSH-into-Windows.
3. **Verify** with `cua-driver doctor`.

### Linux

The full tool surface is supported on X11 (background input via AT-SPI
+ XSendEvent, screenshots, recording); native Wayland input/capture is
opt-in and still preview. See `LINUX.md`. Install:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
```

(On Linux the canonical installer auto-detects the non-macOS host and
installs the Rust port — no flag needed.)

## Install

The skill is a drop-in directory. Same shape on every platform.

**Personal scope** (all Claude Code sessions on your machine):

```bash
mkdir -p ~/.claude/skills
cp -R libs/cua-driver/rust/Skills/cua-driver ~/.claude/skills/
```

Or symlink if you want edits-in-place:

```bash
ln -s "$PWD/libs/cua-driver/rust/Skills/cua-driver" ~/.claude/skills/cua-driver
```

**Project scope** (committed alongside a specific repo):

```bash
mkdir -p .claude/skills
cp -R /path/to/cua/libs/cua-driver/rust/Skills/cua-driver .claude/skills/
```

Or run the verb that does this automatically + fetches the matching
release version from GitHub:

```bash
cua-driver skills install
```

See `cua-driver skills --help` for the full subcommand list
(`install` / `update` / `uninstall` / `status` / `path`).

## Invoking the skill

Claude Code auto-invokes the skill when you ask for GUI automation —
e.g. "open the Downloads folder in Finder", "click the Save button in
Numbers", "open Calculator and compute 17×23", "navigate to
trycua.com in Edge". You can also invoke it explicitly:

```
/cua-driver-rs
```

## Claude Code MCP compatibility mode

For normal skill-driven use, prefer the CLI or the standard MCP server. If you want Claude Code's vision/computer-use-style flow to ground on CuaDriver screenshots, register the compatibility server. The cleanest path is to let `cua-driver` print the right command for your shell:

```bash
cua-driver mcp-config --client claude
# then paste + run the printed `claude mcp add-json …` line
```

Under the hood this registers a `cua-computer-use` stdio server that runs `cua-driver mcp --claude-code-computer-use-compat`. The `add-json` form sidesteps a PowerShell arg-parsing quirk that mangles long flags following `--`.

This mode exposes the normal CuaDriver tools and changes only `screenshot`. The compatibility screenshot requires `pid` and `window_id`, captures that window only, and establishes a window-local pixel coordinate frame. It does not call Anthropic APIs or expose Anthropic's native computer-use API tool.

Use MCP for this Claude Code vision/computer-use-style path. CLI screenshots still work as CuaDriver calls, but they do not expose the `mcp__cua-computer-use__screenshot` tool name that Claude Code appears to use as the image-grounding cue.

## Files

- `SKILL.md` — main skill body, shared core + macOS-specific (~900
  lines). Loaded on first invocation; stays in context.
- `WINDOWS.md` — Windows-specific carve-out (UIA, UWP, layered
  click chain, Session 0, Windows web-apps). Loaded when driving
  on Windows.
- `LINUX.md` — Linux-specific carve-out (X11 background input,
  recording, Wayland opt-in/preview). Loaded when driving on Linux.
- `WEB_APPS.md` — browser patterns on macOS (Chromium + WebKit,
  Electron, Tauri, minimized-Chrome keyboard-commit caveat).
  Loaded on demand from `SKILL.md`. **Note**: Windows web-apps
  coverage lives in `WINDOWS.md`'s "Web apps on Windows" section.
- `RECORDING.md` — trajectory recording / replay (macOS and Linux
  X11 today; Wayland video capture not yet supported).
- `TESTS.md` — manual test scripts for end-to-end skill verification.

## Troubleshooting

- `cua-driver: command not found` → re-run the installer or add
  `.build/CuaDriver.app/Contents/MacOS/` to `$PATH`.
- `No cached AX state for pid X window_id W` → element_index was
  reused across turns, or across different windows of the same app.
  Call `get_window_state({pid, window_id})` first in the same turn,
  with the same window_id you're about to act against.
- Empty `tree_markdown` (with `degraded:true`) → this is a non-AX
  surface (canvas / WebGL / Electron web content), not a mode problem
  — `get_window_state` always walks the tree now. Act by pixel off the
  screenshot already in the same response (an element px action).
  Tiny screenshot → likely a stale window capture. See the perception
  + element ax/px action notes in SKILL.md.
- System-alert beep when pressing Return on a minimized Chrome
  omnibox → the keyboard-commit-on-minimized limitation. Use
  `set_value` on the field instead, or AX-click a Go/Submit button.
  See `WEB_APPS.md`.

## Updates

The skill evolves alongside the driver. To update:

```bash
# Easiest — fetch from the matching release tag on GitHub:
cua-driver skills update

# Or, from a clone of trycua/cua:
cd /path/to/cua && git pull
# if you copied: re-copy
cp -R libs/cua-driver/rust/Skills/cua-driver ~/.claude/skills/
# if you symlinked: nothing needed
```

## License

MIT. Same license as the parent `trycua/cua` repo.
