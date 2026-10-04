# Cua Spaces: teleport

How to run the app against real Spaces, build the macOS golden, and where the
pieces live. Architecture overview: `README.md`.

## 1. Pieces

| Piece | Where | Role |
| --- | --- | --- |
| App (Tauri) | `apps/cua-spaces` | UI; `src-tauri` commands are thin calls into cua-spaces / cua-daemon |
| cua-spaces | `libs/cua/crates/cua-spaces` | Spaces core, linked in-process by the app; also hosted by the daemon |
| `cua daemon` | `libs/cua/crates/cua-daemon`, CLI in `cua-cli` | shared runtime: registry, media bridge, `cua daemon mcp` |
| cua-spacesd | `libs/cua-spacesd` | the only daemon inside a Space (:3211 gRPC/gRPC-Web, `/media`, `/mcp`, `/health`; QUIC media on 3212) |
| Golden (macOS) | `scripts/golden/` | Lume image with `Cua Spacesd.app` + `CuaDriverLocal.app` |

Removed: `mcp/spaces_mcp.py` and `agent_harness.py` (now `cua daemon mcp` and
`cua_spaces::agents`), the app's own Fleet REST client, Lume CLI driver,
loopback WebSocket relay, rcdp CLI teleport, and the in-guest rcdpd /
rcdp-handoff / computer-server daemons.

## 2. Run the app against live Spaces

```bash
cd apps/cua-spaces
pnpm install
./run-live.sh            # loads ~/.env (Fleet creds), starts `cua daemon`, prints the launch command
./run-live.sh --launch   # same, then `pnpm tauri dev` (opens the app's windows)
```

Fleet credentials (`~/.env`, never committed): `CUA_CLIENT_ID`,
`CUA_CLIENT_SECRET`, `CUA_TOKEN_URL`, `CUA_FLEET_BASE_URL`, or `FLEETS_TOKEN`.
Without them Cloud is unavailable; **Add by address** and **Local** still work.

Quick direct Space for manual testing (Docker, gVisor, 4 GiB cap):

```bash
TOKEN=$(openssl rand -hex 16)
docker run -d --name cua-e2e-space --runtime=runsc --memory=4g --memory-swap=4g \
  -e CUA_ENV_TOKEN=$TOKEN -p 127.0.0.1:3211:3211 \
  cua-e2e-local/linux:docker-local-arm64
echo "Add by address: 127.0.0.1:3211  token: $TOKEN"
# ...
docker rm -f cua-e2e-space
```

Agent CLIs: Settings → AI agents installs the cua skills and registers
`cua mcp` in each detected coding agent (same engine as `cua agents setup`), or
`claude mcp add cua -- cua mcp`. OpenClaw:
`scripts/openclaw/install-openclaw-mcp.sh`.

## 3. Tauri command contract

Space ids are `space://direct/<host:port>`, `space://fleet/<ns>/<claim>`,
`space://local/<name>`.

| Area | Commands |
| --- | --- |
| Roster | `list_spaces`, `space_info(spaceId)`, `add_space(url, token, name)`, `create_space({image, on, kind, runtime, name, …})`, `delete_space`, `remove_space`, `get_default_location`, `set_default_location(on)`, `keep_alive_space`; event `spaces:changed` |
| Status | `fleet_status`, `begin_sign_in`, `sign_out`, `daemon_status`, `ensure_daemon`, `local_status` |
| This machine | `host_status`, `host_setup`, `host_stop_sharing`, `host_start_sharing`, `host_remove`, `onboarding_state`, `complete_onboarding`, `installer_*` |
| Space ops | `space_screenshot`, `send_files_to_space`, `list_remote_windows`, `remote_window_thumbnail`, `space_app_icon`, `list_space_agents` |
| Streams | `open_space_stream(space, {kind: display|window}, opts)` → ticketed `wsUrl` (direct, or daemon media bridge), `close_space_stream` |
| Teleport an app | `teleport_catalog(spaceId?)`, `teleport_app_icon(path, size)`, `teleport_entry_for_path(path)`, `teleport_parse_drop(items)`, `teleport_plan(spaceId, entry, options)`, `teleport_run(spaceId, plan, consent, onEvent)`, `teleport_choose_files` → the SDK core (`cua_teleport::ux`, `cua_spaces::teleport_app`) |
| Session teleport | `teleport_manifest(app, scope)`, `teleport_push(app, scope, space, include, acknowledgeSensitive)` → cua-spaces `Approval` + `Space::teleport` (the transfer overlay's Retry) |
| Hotspot | `start_hotspot`, `stop_hotspot`, `hotspot_status`; event `hotspot:changed` |
| App UI | viewer/PiP/window-stream windows, teleport picker, spaces list, transfer overlay, window drag, `agent_setup_*`, teleport policy |

The loopback control server (`~/.cua/spaces-control.json`) serves
`/pip/pin`, `/pip/unpin`, `/viewer/open` and `/window/stream` (a ticketed
`media_url`) for `cua daemon mcp`.

## 4. Build the macOS golden (Lume)

Needs a real Apple-Silicon Mac (TCC seeding only works in a SIP-disabled
Apple Virtualization VM), `lume serve` on :7777, a base VM `macos-tahoe`
(SIP off, autologin, ssh, password `lume`), `sshpass`, and a stable
certificate-backed signing identity (ad-hoc signatures do not hold the seeded
TCC grants).

1. Build and sign the bundles:
   - `Cua Spacesd.app` (bundle id `com.trycua.cua-env-driver`):
     `CUA_ENV_CODESIGN_IDENTITY="<identity>" libs/cua-spacesd/scripts/build-macos-app.sh`
   - `CuaDriverLocal.app` (`com.trycua.driver.local`) from `libs/cua-driver`.
2. Build the image, hand-run or CI wrapper:
   ```bash
   BUNDLE_DIR=/path/with/both/apps scripts/golden/build-golden.sh cua-golden
   # or
   DRIVER_APP=... SPACESD_APP=... GOLDEN=cua-golden scripts/build-macos-golden.sh
   ```
   `scripts/golden/check-golden-coverage.sh` is the fast static gate;
   `verify-golden.sh` runs last in the guest and requires the spacesd to
   answer on `:3211/health`.

In the golden the spacesd runs from its bundle as a LaunchAgent
(`com.trycua.spacesd`). No token ships: with `~/.cua/spacesd/token`
present it uses that, otherwise it starts in bootstrap mode and the first
client installs a fresh token with `SystemService.Init`. Agents inside the
Space use cua-driver's own stdio MCP (`cua-driver-local mcp`).

## 5. Tests

```bash
pnpm test                                             # vitest (jsdom), no network
pnpm build                                            # typecheck + bundle
cargo test --manifest-path src-tauri/Cargo.toml       # command layer against fakes
bash scripts/golden/check-golden-coverage.sh          # golden gates, VM-free
```

Live tests run only against Docker/Lume sandboxes, never the host's apps.

## 6. Teleport an app

The picker window (label `teleport-picker`, a normal decorated window) opens
on "Teleport an app…": every app on this Mac, classified by the cua SDK.

| Level | Meaning | Moves |
| --- | --- | --- |
| Full | a provider imports its signed-in state | app, app with files (when it opens files), app with state |
| Install only | a pinned, checksum-verified install from the manifest; opens empty or with files | app, app with files |
| Not available | shown disabled with the reason | nothing |

The flow is the SDK's headless picker (`@trycua/cua/teleport`, aliased to
its source in `vite.config.ts`); `AppTeleportPicker.tsx` is its view. The
consent screen lists every install, path and secret before anything runs;
secrets need an explicit acknowledgement. Progress goes to the picker and to
the Space window's transfer overlay. Recents land in
`~/.cua/teleport-recents.json`.

Entry points:

- the notch "Teleport to Cua" prompt and the Space tiles, the viewer's
  "Teleport an app…" button and the drop zone's "Select an app…";
- dropping a `.app` (Finder, Dock) on a Space tile, the drop zone or a Space
  window opens the picker on that app, with any files dropped alongside it.
  Files and folders alone keep the file transfer
  (`src/native/appDrop.ts`, `src/viewer/useAppDrop.ts`);
- dragging a real app window (Accessibility permission): the SDK's window-drag
  monitor reports the window and its app; for a Full or Install-only app the
  notch shows "Teleport to Cua" with the window's preview (that one window,
  captured in memory), and dropping it on a Space opens the picker on the app.

Tests use fixture app bundles and fixture window lists only
(`src-tauri/tests/teleport_app.rs`, `window_drag.rs` unit tests, vitest).

## 7. Teleport providers

Provider status (Firefox logs in cross-platform; Chrome moves tabs and
bookmarks, cookie login needs the macOS Safe Storage key; Electron apps,
WhatsApp and Steam move data but may re-authenticate) is tracked with the
providers in `libs/cua/crates/cua-teleport` (export) and
`libs/cua-spacesd/crates/cua-spacesd-teleport` (import). Two hard
limits: Chromium/Electron cookies are encrypted with a per-OS Keychain key
(macOS→macOS only), and many apps bind sessions to a device.

## 8. Troubleshooting

- **Golden boot "Failed to lock auxiliary storage"**: don't mix `lume run`
  with the daemon API; reset with
  `launchctl kickstart -k gui/$UID/com.trycua.lume_daemon`.
- **SSH "Too many authentication failures"**:
  `ssh -o PubkeyAuthentication=no -o PreferredAuthentications=password lume@<ip>`.
- **`lume stop` loses writes**: it is a hard power-off; `sync` in the guest first.
- **Spacesd not answering in a clone**: `tail ~/.cua-server/com.trycua.spacesd.err.log`
  in the guest; `launchctl kickstart -k gui/$(id -u)/com.trycua.spacesd`.
- **Chrome clone crashes / "Restore pages?"**: the golden's Chrome wrapper must
  be `codesign --force --deep` signed and carry `--hide-crash-restore-bubble`.
