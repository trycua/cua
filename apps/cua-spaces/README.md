# Cua Spaces

A Tauri 2 desktop app for your Spaces: a main window (sidebar of Spaces,
toolbar, Space detail with a live preview, windows, agents and file drop), a
menu bar item, and the notch panel as the quick switcher.

A **Space** is a sandbox that runs **cua-spacesd** (port 3211). Without any
Space the browser preview shows synthetic fixtures (and so do the tests);
nothing touches the network.

## Windows

| Surface | What it is |
| --- | --- |
| Main window (`main`) | Standard, opaque, decorated window. First run shows the welcome flow here; then the Spaces sidebar and detail. New Space (Cmd+N) is a sheet; Settings (Cmd+,) is a page. Closing it hides it; the Dock icon, the menu bar item and the notch reopen it. |
| Menu bar item | The Cua mark as a template image. Click: open the main window. Right click: status line, Open Cua Spaces, New Space, Settings, Quit. |
| Launch at login | Settings → General → Launch Cua Spaces at login (the first run's Done page ticks it). Linux: an XDG autostart entry; Windows: the current user's `Run` key; macOS: a LaunchAgent. A login launch passes `--autostart` and opens without the main window. |
| Notch panel (`portal`) | The only overlay: the Space tiles (click one to open its desktop) and the Teleport drop prompt. Its New, list and gear buttons open the main window. |
| Space viewer (`space-*`) | Ordinary resizable window. The remote display scales to fit; Actual Size (Cmd+0) shows it 1:1, Cmd+9 fits again. It never enters fullscreen on its own. |
| Teleport picker (`teleport-picker`) | Standard opaque window: pick an app window, review the manifest, confirm. |
| PiP (`pip-*`) | Small decorated window that floats above other windows. |

Nothing is translucent: no vibrancy, no glass theme.

`CUA_SPACES_START_VIEW=new-space` (or `settings`) opens the main window on that
view at launch in debug builds (release builds ignore the recording hooks).

## Architecture

```
  Cua Spaces app (Tauri)                    other clients
  ├─ UI: notch, switcher, viewers, PiP,     cua CLI, `cua daemon mcp`
  │  window drag, teleport consent ladder   (Claude Code, Codex, OpenClaw)
  └─ src-tauri: thin commands over                │
     cua-spaces (in-process)  ─────┐              │
     cua-daemon client ────────────┼──── `cua daemon` (UDS ~/.cua/cua.sock
                                   │      + loopback): media bridge, env
       shared registry ~/.cua/spaces.json          passthrough, MCP
                                   │
            direct:host:port | cloud:name | local:name
                                   ▼
                     cua-spacesd :3211 in every Space
                     gRPC + gRPC-Web, /media (wire v2), /mcp, /health
```

- **cua-spaces** (`libs/cua/crates/cua-spaces`) is the Spaces core: registry,
  create / delete in Cua Cloud or on this Mac, files, streams, presence, teleport,
  hotspot, agents. The app links it directly; `src-tauri` commands are
  one-liners over it. There is no cloud REST client, Lume CLI, rcdp client or
  WebSocket relay of the app's own any more.
- **`cua daemon`** hosts the same SDK runtime out of process. The app connects
  to a running daemon or starts one (`cua daemon start`), so the CLI and MCP
  clients share Spaces with the app. The daemon's **media bridge**
  (`OpenMediaBridge`) serves ticketed WebSockets for Spaces whose socket needs
  headers a webview cannot set (the Cua Cloud gateway).
- **MCP**: agents drive Spaces through `cua mcp` (`cua daemon mcp`).
  Settings → AI agents runs the SDK's agent onboarding (`cua-agent-setup`, same
  as `cua agents setup`): it installs the cua skills and registers `cua mcp` in
  each detected coding agent. Tools that present a Space
  on this Mac (pin PiP, open viewer, stream a window) call back into the app's
  loopback control server (`~/.cua/spaces-control.json`).

### Adding Spaces

**New Space** (Cmd+N, the sidebar's +, the notch's New, or the menu bar item)
is a step-by-step sheet:

1. **System**: Linux, Windows or macOS, the image (a dropdown of the published
   entries of `libs/images/sandbox-images.json`, the same list the docs show),
   and where it runs: This Mac. "Your cloud (AWS, Azure, GCP...)" shows as
   coming soon and cannot be picked (the core's `CLOUD_SPACES_OFFERED`; the
   cua SDK and CLI still create cloud sandboxes, and cloud Spaces that already
   exist still list). **Advanced** picks the kind (container or virtual
   machine, when the image comes in both) and the runtime, offering only what
   the image runs there: containers `auto`, `gvisor`, `runc`; VMs `auto` and
   the image's `qemu` or `lume`.
2. **Resources**: CPU cores and memory.
3. **Options**: name, and whether to open the desktop when it is ready.
4. **Summary**, then **Create Space**.

Every create is one `create_space {image, on, kind, runtime, ...}` call.
Spaces on this Mac run through
`cua-vmm`. The notch "+" drop creates in the default location. **Connect by
address** adds a machine that already runs cua-spacesd (`host:port` plus its
token). Delete deletes a created Space's sandbox (cloud or local); a Space
added by address is only removed from the list.

### First run and "This machine"

The first launch shows the welcome flow in the main window, one step per page
with page dots: Welcome, Command line (put the bundled `cua` on PATH), Sign in,
AI agents (cua skills and the cua MCP server), This machine, Done. Every write
asks first and shows its path.

"This machine" asks what the machine is for:

- **Access other machines**: nothing is installed. Cloud Spaces and the
  signed-in account's own machines (`space://relay/<id>`) appear automatically.
- **Set up this machine for unattended access**: `cua-host` (the library
  behind `cua host setup`) installs cua-spacesd as a service that joins the Cua
  relay (`https://relay.cua.ai`, or `CUA_RELAY_URL`). Direct `ip:port` is
  under **Advanced**. On macOS the Screen Recording and Accessibility panes are
  listed for the user to open; nothing is granted programmatically.

The sidebar always has a **This machine** entry with its sharing state and
**Stop sharing** / Resume / Remove.

### Installers

`tauri.conf.json` bundles a dmg (macOS), NSIS + MSI (Windows) and deb +
AppImage (Linux); `scripts/build-pkg.sh` builds a macOS .pkg for MDM (it also
links `/usr/local/bin/cua` to the bundled CLI). Releases
(`.github/workflows/cd-cua-spaces.yml`) ship this app for Linux and Windows
only; the macOS release is the SwiftUI app (`apps/cua-spaces-macos`), and
the release .pkg wraps that app.
`.github/workflows/ci-cua-spaces.yml` builds them unsigned on pull requests. Silent / MDM installs can preselect the first-run choice
(the user still confirms in the app):

| Installer | Flag |
| --- | --- |
| NSIS | `setup.exe /S /MODE=host` (writes `%USERPROFILE%\.cua\spaces-install-mode`) |
| MSI | `msiexec /i … /qn CUA_SPACES_MODE=host` (writes `%ProgramData%\Cua\spaces-install-mode`) |
| any | launch with `--mode host\|client`, or write `~/.cua/spaces-install-mode` (`/Library/Application Support/Cua/…` or `/etc/cua/…` machine-wide) |

### Streams

Viewers use media **wire v2** (`libs/cua/proto/MEDIA.md`): the app asks for a
media ticket (`open_space_stream`), the webview attaches to the ticket URL
(the ticket is scoped to one session and safe in a URL), the first packet is
a keyframe, video decodes with WebCodecs (H.264, BGRA/PNG fallback) and audio
(Opus) with `AudioDecoder` + an AudioWorklet. Full-Space viewers use the
spacesd **desktop** target, the only transport. A Space whose cua-spacesd
lacks `desktop_stream` shows an "update the image" message. The per-window client (`win-*` / `winone-*`) opens one
media session per remote window.

### Teleport

Dragging a local app window onto a tile (or the per-tile Sync) opens the
consent ladder: pick a window → see the manifest item by item (sensitive items
flagged) → confirm. Only the confirmed items are approved: the app mints a
cua-spaces `Approval` (which refuses unacknowledged sensitive items) and
`Space::teleport` exports on this Mac and imports into the Space through
`TeleportService`, SHA-256 verified. Files dropped on the Teleport zone land
in the Space's `~/Downloads`, digest-verified.

### Cua Cloud configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `CUA_FLEET_BASE_URL` | `https://run.cua.ai` | Cua Cloud API base URL |
| `CUA_TOKEN_URL` | `https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token` | OAuth token endpoint |
| `CUA_CLIENT_ID` / `CUA_CLIENT_SECRET` | none | client-credentials auth |
| `FLEETS_TOKEN` | none | static bearer token (takes precedence) |

The signed-in user (device grant, Settings → Sign in to Cua) takes precedence
over the environment. The default location is `default.on` in
`$CUA_HOME/config.toml` (Settings → Default location); `CUA_DEFAULT_ON`
overrides it and Settings shows that read-only. `./run-live.sh` loads `~/.env` and starts the daemon.

The Tauri command contract is listed in `TELEPORT.md`.

## Prerequisites

- Node 20+ and `pnpm` 10+
- Rust 1.97 (pinned by `rust-toolchain.toml`)
- For live Spaces: the `cua` CLI (`cargo build --manifest-path ../../libs/cua/Cargo.toml -p cua-cli --release`), Docker (+ gVisor `runsc` for local container Spaces) or Lume for macOS Spaces
- Tauri 2 platform prerequisites for your OS
  (macOS: Xcode Command Line Tools; see <https://tauri.app/start/prerequisites/>)

## Commands

All commands run from `apps/cua-spaces`.

| Task                        | Command                                               |
| --------------------------- | ----------------------------------------------------- |
| Install                     | `pnpm install`                                        |
| Native dev (Tauri + Vite)   | `pnpm tauri dev`                                      |
| Browser-only dev (no shell) | `pnpm dev` then open <http://localhost:1420>          |
| Frontend tests              | `pnpm test`                                           |
| Rust tests                  | `cargo test --manifest-path src-tauri/Cargo.toml`     |
| Frontend type-check + build | `pnpm build`                                          |
| Native build, no bundle     | `pnpm tauri build --no-bundle`                        |

In browser-only dev the native bridge is replaced by a local stand-in with
fixtures. <http://localhost:1420> shows the notch panel (add `?display=no-notch`
for the capsule fallback); `?surface=main` shows the main window and
`?surface=main&onboarding` its welcome flow.

## Layout

```
src-tauri/src/          Tauri commands over cua-spaces / cua-daemon; portal geometry,
                        viewer windows (space-*/pip-*/win-*/winone-*), window drag,
                        control server (PiP/viewer/window hooks for `cua daemon mcp`),
                        agent_setup (skills + `cua mcp` in coding agents), teleport policy
src/model/              Serializable types, fixtures, MRU, consent ladder (unit tested)
src/state/              Portal reducer; Spaces roster sync
src/native/             Typed bridges to the Rust commands, with browser/test fallbacks
src/components/         Ambient, Switcher, SpaceTile, NewSpaceView, TeleportSheet, …
src/viewer/             SpaceViewer (desktop stream) + WindowStream (wire v2)
scripts/golden/         macOS golden image (Lume) with cua-spacesd
```

### Window modes

The renderer asks Rust for a mode; Rust resizes the window and re-centres it
at the top of the **current** monitor (falling back to the primary), clamped so
the frame never leaves the display.

| Mode           | Notched (pt) | No-notch (pt) |
| -------------- | ------------ | ------------- |
| `ambient`      | 420 × 38     | 180 × 34      |
| `switcher`     | 760 × 320    | 760 × 300     |

### Notch detection (best-effort)

Tauri exposes no public safe-area API, so the shell uses a heuristic: notched
MacBook panels report a logical aspect ratio near 1.54:1 (e.g. 1512×982),
whereas non-notched Macs and external displays sit at 1.6:1 or 16:9. The result
is reported to the renderer as `displayStyleSource: "heuristic"`. Override it
with the `CUA_SPACES_DISPLAY=notched|no-notch` environment variable, the
development-only toggle in the switcher footer, or `⌘⇧D` in dev builds.

### Keyboard

| Key           | Action                                   |
| ------------- | ---------------------------------------- |
| Click ambient | Expand to the switcher                   |
| `←` `→`       | Move focus across Spaces and **+ New**   |
| `↩`           | Switch to the focused Space              |
| `⌘N` / `Ctrl+N` | Open New Space in the main window      |
| `esc`         | Collapse                                 |

Selecting a Space updates MRU order and the selected state, shows a brief
"Switching to …" confirmation, and collapses. The OS Space is **not** switched;
only the intent is recorded.

## Limitations

- **Fixtures in the browser preview.** Outside the shell (`pnpm dev`, tests)
  the notch and the main window (`?surface=main`) show fixtures; nothing touches
  the network.
- **Mission Control button is intent only.** Space selection focuses the
  sandbox's own window (fullscreen it to get a real OS Space); no private
  macOS Spaces or Mission Control APIs are used.
- **The notch panel uses Tauri's `macOSPrivateApi` flag** for its transparent
  webview background (only the notch shape is drawn). It is not an
  App-Store-safe API. Every other window is opaque.
- **macOS uses a status-level window.** The native shell raises the portal to
  AppKit's public status-window level so its frame can occupy the menu-bar/notch
  band. Other platforms use their ordinary always-on-top window behavior.
- **Notch detection is heuristic**, as described above.
- **Thumbnails are synthetic for fixtures.** Live tiles poll
  `space_screenshot` (spacesd `ComputerService.Screenshot`) at ~0.3 fps;
  local screen content is never captured.
- **A regular app.** The app has a Dock icon and a main window; the notch and
  the menu bar item are quick paths into it.
- Auto-suspend affects the displayed estimate only as an upper-bound caveat.

## License

Source-available under FSL-1.1-MIT ([LICENSE](LICENSE)). Offering it as a hosted or managed service needs a commercial licence: see [COMMERCIAL.md](../../COMMERCIAL.md).
