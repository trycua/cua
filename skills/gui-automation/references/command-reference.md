# CUA Command Reference

Complete argument syntax for all `cua do` and `cua trajectory` commands.

## Global Flags

| Flag          | Effect                                        |
| ------------- | --------------------------------------------- |
| `--no-record` | Disable trajectory recording for this command |

Usage: `cua do --no-record <action> [args]`

## Target Management

```
cua do switch <provider> [name]
```

Providers: `cloud`, `cloudv2`, `docker`, `lume`, `lumier`, `winsandbox`, `host`

- `name` is required for all providers except `host` and `winsandbox`
- Target persists in `~/.cua/do_target.json` — set once, then operate

```
cua do status
```

Show the current target and zoom state.

```
cua do ls [provider]
```

List VMs for a provider. If `provider` is omitted, uses the current target's provider.

## Host Consent

```
cua do-host-consent
```

One-time consent to grant AI control of the local machine. Creates
`~/.cua/host_consented`. Required before `cua do switch host`.

## Screenshot and Snapshot

```
cua do screenshot [--save PATH]
```

Take a screenshot. Returns the image path. If `--save` / `-s` is provided,
saves to that path instead of the default temp directory.

```
cua do snapshot ["extra instructions"]
```

Screenshot + AI-powered screen summary. Returns a JSON object with:

- `summary`: human-readable description of the screen
- `elements`: list of interactive elements with `name`, `type`, `x`, `y`

Requires `ANTHROPIC_API_KEY`. Optional instructions refine the AI's focus
(e.g., `cua do snapshot "focus on the form fields"`).

## Window Zoom

```
cua do zoom "Window Name"
```

Crop all subsequent screenshots to the named window. Coordinates become
window-relative. Useful for focusing on a single app.

```
cua do unzoom
```

Return to full-screen screenshots.

## Input Actions

### Click

```
cua do click <x> <y> [left|right|middle]
```

Click at image-space coordinates. Default button: `left`.

### Double-Click

```
cua do dclick <x> <y>
```

Double-click at image-space coordinates.

### Move Cursor

```
cua do move <x> <y>
```

Move the cursor to image-space coordinates without clicking.

### Type Text

```
cua do type "text"
```

Type a string of text. Supports any Unicode characters.

### Press Key

```
cua do key <key>
```

Press a single key. Common keys: `enter`, `escape`, `tab`, `space`, `backspace`,
`delete`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`,
`f1`–`f12`.

### Hotkey

```
cua do hotkey <combo>
```

Press a keyboard shortcut. Use `+` to combine modifiers.

Examples: `cmd+c`, `ctrl+shift+s`, `alt+f4`, `cmd+shift+p`

### Scroll

```
cua do scroll <direction> [amount]
```

Scroll in a direction. Default amount: `3`.

Directions: `up`, `down`, `left`, `right`

### Drag

```
cua do drag <x1> <y1> <x2> <y2>
```

Drag from `(x1, y1)` to `(x2, y2)` in image-space coordinates.

## Shell and Open

### Shell

```
cua do shell "command"
```

Run a shell command on the target machine. If no command is provided, opens an
interactive terminal session.

Optional flags:

- `--cols N` — terminal width (default: auto-detect)
- `--rows N` — terminal height (default: auto-detect)

### Open

```
cua do open <url|path>
```

Open a URL in the default browser or a file in its default application on the
target machine.

## Window Management

All window commands are under `cua do window`.

### List Windows

```
cua do window ls [app]
```

List all windows, optionally filtered by application name. Returns window IDs
needed for other window commands.

### Focus / Activate

```
cua do window focus <id>
cua do window activate <id>
```

Bring a window to the front.

### Unfocus

```
cua do window unfocus
```

Remove focus from the current window.

### Minimize / Maximize / Close

```
cua do window minimize <id>
cua do window maximize <id>
cua do window close <id>
```

### Resize

```
cua do window resize <id> <width> <height>
```

### Move

```
cua do window move <id> <x> <y>
```

### Info

```
cua do window info <id>
```

Get details (position, size, title) for a window.

## Trajectory Commands

All trajectory commands are under `cua trajectory` (alias: `cua traj`).

### List Sessions

```
cua trajectory ls [machine] [--json]
```

List all recorded sessions, optionally filtered by machine name.

### View (Local)

```
cua trajectory view [target] [--port PORT]
```

Zip the session and open it in the browser via `cua.ai/trajectory-viewer`.
Starts a local file server (default port: `8089`).

`target` can be a machine name, session timestamp, or directory path. Defaults
to the latest session.

### Share (Upload)

```
cua trajectory share [target] [--no-open] [--api-url URL]
```

Upload the trajectory and get a shareable HTTPS link. Opens the link in the
browser by default.

- `--no-open` — don't open browser after uploading
- `--api-url` — custom API URL (default: `CUA_API_URL` env or `https://cua.ai`)

### Export (HTML)

```
cua trajectory export [target] [--output PATH] [--quality N] [--no-open]
```

Generate a self-contained HTML report.

- `--output` / `-o` — output file path (default: `report.html` in session dir)
- `--quality` / `-q` — JPEG compression quality 1–100 (default: `75`)
- `--no-open` — don't open the report in browser

### Clean

```
cua trajectory clean [--older-than DAYS] [--machine NAME] [-y]
```

Delete old trajectory sessions.

- `--older-than DAYS` — only delete sessions older than N days
- `--machine NAME` — only delete sessions for a specific machine
- `-y` / `--yes` — skip confirmation prompt

### Stop

```
cua trajectory stop
```

Stop the local trajectory file server started by `cua trajectory view`.
