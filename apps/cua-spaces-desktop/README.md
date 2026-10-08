# Cua Spaces desktop shell

The Electron shell for the Cua Spaces web UI on macOS, Windows and Linux. It
loads the UI from `apps/cua-spaces-web` over a private `cua-spaces://app/`
scheme and gives it a small bridge, `window.cuaDesktop`, into the main process.
The main process is a native host like the SwiftUI app: it loads the same Rust
library (`cua-spaces-ffi`: the cua SDK and the Spaces app core) and answers the
SwiftUI host's bridge methods with the same shapes (see "Native layer").

This is a standalone pnpm project with its own lockfile. It is not part of the
root workspace: its own `pnpm-workspace.yaml` makes this directory the
workspace root, so a plain `pnpm install` here uses that file's settings
(which builds may run, and short `node_modules/.pnpm` names). With
`--ignore-workspace` pnpm skips those settings, does not download Electron, and
exits 1 over the ignored `electron-winstaller` build script.

```sh
pnpm install
pnpm native         # the native layer into native/<platform>-<arch>/ (see below)
pnpm dev            # Electron against the web dev server on :5174
pnpm build          # typecheck, then bundle main + preload into dist-electron/
pnpm start          # run the built bundle against ../cua-spaces-web/dist
pnpm dist:mac:dir   # unsigned arm64 .app in dist/mac-arm64/
pnpm dist:mac       # dmg + zip, arm64 + universal (unsigned unless CSC_* is set)
pnpm dist:win       # NSIS installers, x64 + arm64 (cross-builds on macOS)
pnpm dist:linux     # AppImage + deb, x64 + arm64 (cross-builds on macOS)
pnpm smoke:linux    # deb + AppImage under Xvfb in Docker (after dist:linux)
pnpm smoke:win      # unpack and check the NSIS installers in Docker (after dist:win)
pnpm measure        # cold start of the packaged mac app (after dist:mac:dir)
pnpm video:bench    # the video bench, this app or the SwiftUI app on one Space (docs/video.md)
pnpm notch          # macOS: the notch helper into native/notch/ (before dist:mac)
pnpm test           # unit tests; the bridge on the real app core once `pnpm native` ran
```

Artifacts go to `dist/`, which is gitignored. Build `apps/cua-spaces-web`
first (`pnpm build` there) so the real UI is bundled; otherwise the package
carries only the placeholder page.

`pnpm dev` uses `http://localhost:5174`, or `CUA_SPACES_DEV_URL` if set. If
nothing is listening there and `apps/cua-spaces-web` exists, it starts that
app's Vite server. Otherwise it shows the bundled placeholder page in
`placeholder/`. The same placeholder is served when `../cua-spaces-web/dist`
has not been built.

If you launch Electron from another Electron app (an editor or an agent host),
unset `ELECTRON_RUN_AS_NODE` first, or Electron starts as plain Node.
`scripts/dev.mjs` does this for you.

`CUA_SPACES_CAPTURE_ROUTES` (below) captures every route, in light and dark,
with `capturePage`, so the native traffic lights are not in the images.

## Measurements

Measured on 2026-10-03 on an Apple silicon Mac (macOS 26.6) with Electron
44.5.1, an unsigned build, and the placeholder page.

| | |
|---|---|
| `Cua Spaces.app`, arm64 | 242 MB, of which Electron Framework is 238 MB and `app.asar` is 2.1 MB |
| `Cua Spaces.app`, universal | 461 MB |
| arm64 dmg / zip | 116 MB / 115 MB |
| universal dmg / zip | 215 MB / 215 MB |
| Cold start, first launch of a new build | about 2.1 to 2.7 s from spawn to `ready-to-show` (macOS scans the new binary first) |
| Cold start, later launches | median 336 ms from spawn to `ready-to-show` (about 295 ms inside the process) |

`pnpm measure` repeats the start-up runs with a fresh profile each time. These
numbers will grow once the real web UI replaces the placeholder.

## Layout

| File | What it does |
|---|---|
| `src/boot.ts` | Packaged entry. Turns on the V8 compile cache (`module.enableCompileCache`), then loads `main.cjs` |
| `src/main.ts` | Single-instance lock, start-up order (the native host, the bridge, the notch), theme updates |
| `src/native/` | The native layer: where it is (`location.ts`), loading it (`load.ts`, `node-runtime.ts`), and the generated bindings (`generated/`, `pnpm native -- --bindings`) |
| `src/bridge/` | The bridge host (the SwiftUI app's `WebUIBridge`): one file per area registering its methods, the table and envelope (`host.ts`), `BridgeValue` (`value.ts`), IPC (`ipc.ts`) |
| `src/model/` | The SwiftUI app's models, ported: `AppModel`, `SpacesBackend`, the launch (`startup.ts`), the daemon (`daemon.ts`), `AppEnvironment` (`environment.ts`), host, devices, clouds, the Keyvault (`keyvault.ts`), Teleport's sources, consent and caches (`teleport.ts`), a Space's stream rows and panels (`streams.ts`), thumbnails (`thumbnails.ts`) |
| `src/drop.ts` | The preload's part of the drop well: the last drop's paths go on `spaces.droppedFiles` (the page sees only names) |
| `src/model/agents.ts` | Agents: the persistent agents and a run's events over the daemon's tools, a Space's runs, the provider keys the daemon keeps in the OS credential vault (`agentKeys.*`), and the coding agents on this machine (`agents.setup`, `configure`, `setupDriver`) |
| `src/model/update-refresh.ts` | The first launch after an update: `cua agents update` with the bundled `cua`, and the notice when it fails |
| `src/prompts.ts`, `src/passphrase.ts` | The Keyvault's native prompts (unlock, delete) and its passphrase form: a window of this app that only its own preload can talk to, so a passphrase never crosses the web UI's bridge |
| `scripts/build-native.mjs` | `pnpm native`: builds the native layer for a Rust target, and the bindings |
| `src/protocol.ts` | `cua-spaces://app/`: privileged scheme, `protocol.handle`, CSP, static files or the dev-server proxy |
| `src/window.ts` | Main window: created hidden and shown on `ready-to-show`, title bar, `--titlebar-left-inset`, saved bounds |
| `src/theme.ts` | `nativeTheme`; background and overlay colours from the web UI's `--background` and `--foreground` (`#16181c` / `#e9ebef` dark, `#f7f8fa` / `#181b1f` light) |
| `src/preload.ts` | Sandboxed preload that exposes `window.cuaDesktop` |
| `src/channels.ts` | Re-exports the web bridge's Electron channels (see below) |
| `src/capture.ts` | `CUA_SPACES_CAPTURE_ROUTES`: route screenshots and checks |
| `src/media-bridge.ts` | `spaces.openStream`: media tickets from the cua daemon (see "Live video") |
| `src/gpu.ts` | Chromium's hardware video decode switches (VA-API on Linux) |
| `src/pip.ts`, `src/pip-layout.ts` | Picture in picture: a floating window per panel, where it opens and its shape |
| `src/viewer.ts`, `src/viewer-layout.ts` | "Open in window": a Space's viewer window (the web UI's `/viewer`: its desktop under the source picker, size and Pop out), one per Space, shaped to the stream, where the last one was left |
| `src/video-bench.ts`, `src/video-report.ts`, `scripts/video-bench.mjs` | The video bench, the same hooks as the SwiftUI app's ([docs/video.md](docs/video.md)) |
| `src/icon.ts` | The window icon on Linux |
| `src/menu.ts` | The application menu, as the SwiftUI app's: Settings… (⌘,), New Space (⌘N), full screen, Cua Spaces Help; on Windows and Linux the same items in File and Help |
| `src/tray.ts` | The menu bar item on macOS and the tray icon on Windows and Linux: the core's menu (on Windows and Linux with each Space) |
| `src/system.ts`, `src/login-item.ts` | The bridge's system services: links and settings panes, Show in Finder / Explorer, notifications, the owner check before approving a device, launch at login on each system |
| `src/migrate-swift.ts` | macOS: takes over the Swift app's settings once (files, defaults domain) |
| `src/notch.ts`, `src/notch/` | The macOS notch: the app core's notch model and the Swift helper that draws it (see "The macOS notch") |
| `src/updater.ts` | electron-updater behind Settings → About (automatic checks and installs, the channel, Check Now with its own windows), beta channel only until the Sparkle cutover, off by default |
| `scripts/sparkle/` | Sparkle's own update checks against an Electron build (`accepts.sh`, `selftest.sh`; [docs/sparkle-cutover.md](docs/sparkle-cutover.md)) |
| `scripts/feed-urls.mjs` | Makes a feed's installer URLs absolute (CI) |
| `scripts/smoke/` | Linux and Windows package checks in Docker |
| `electron-builder.config.cjs` | Packaging and signing hooks |

## Window and chrome

- On macOS the window uses `titleBarStyle: "hiddenInset"` with the traffic
  lights at x=16, centred on a 40 px top bar. The shell injects
  `--titlebar-left-inset: 78px` (0 in fullscreen) and `--titlebar-height: 40px`
  into the page. The web UI pads its top bar with them and marks it
  `-webkit-app-region: drag`.
- On Windows and Linux it uses `titleBarStyle: "hidden"` with a
  `titleBarOverlay` in the web UI's background and foreground colours.
  `--titlebar-left-inset` is 0 there. The web UI reads the overlay's width
  from the `titlebar-area-x` and `titlebar-area-width` CSS environment
  variables (`--titlebar-right-inset`), so the top bar's search field and
  theme toggle stay clear of the window controls.
- On Windows the app sets its AppUserModelID to `ai.cua.spaces.desktop`, the
  same id the NSIS shortcuts carry, so taskbar grouping and notifications
  belong to the app.
- The menu bar item (macOS) and the tray icon (Windows and Linux) carry the
  SwiftUI app's menu from the app core: how many Spaces (the Spaces the page
  lists, not the machines' own desktops), the Cua Volume's sync line, Open
  Cua Spaces, New Space…, Settings… and Quit; on Windows and Linux, in place
  of the notch, also each Space. On Windows, closing the window hides it to
  the tray; Quit in the tray menu or File > Exit stops the app. Linux keeps
  quit-on-close, because many desktops (GNOME without the AppIndicator
  extension) show no tray.
- On Linux the WM_CLASS is `cua-spaces` (`--class`) and the desktop name is
  `cua-spaces.desktop`, matching the installed desktop entry's file name and
  `StartupWMClass`, so docks show the Cua Spaces icon.
- Electron draws the overlay above the page, so a dialog's backdrop can't
  cover it. The web UI marks each backdrop `data-window-dim`; the preload
  watches for one and the main process repaints the overlay in the colour
  the backdrop gives the top bar (`src/overlay.ts`: black at 25% light, 40%
  dark), so the whole title bar dims with the window.
- `backgroundColor` follows `nativeTheme`, so there is no white flash. It
  updates live, along with the overlay colours, when the theme changes.
- Bounds and the maximized state are saved to `settings.json` in the user data
  directory. They are restored only if they still fit on a connected display.

## Security

- `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`.
- Electron fuses in every packaged build (`packaging/fuses.cjs`, flipped by
  electron-builder's `electronFuses` right before signing): `RunAsNode`,
  `EnableNodeOptionsEnvironmentVariable`, `EnableNodeCliInspectArguments`
  and `GrantFileProtocolExtraPrivileges` off; `EnableCookieEncryption`,
  `OnlyLoadAppFromAsar` and `EnableEmbeddedAsarIntegrityValidation` on. The
  asar check runs on macOS (Info.plist hash) and Windows (INTEGRITY
  resource); Linux has none. An unsigned macOS build is re-signed ad hoc
  after the flip. `pnpm check:fuses` reads them back from every build under
  `dist/` (or `node scripts/check-fuses.mjs <app>`; Node built-ins only, so
  it also runs next to an installed app). `pnpm dev` and `pnpm start` use
  the stock Electron and keep `--inspect`. `CUA_SPACES_NO_FUSES=1` at build
  time leaves the fuses off, for a local QA build that needs `--inspect`.
- The preload is a self-contained bundle that requires only `electron`.
- IPC handlers answer only frames whose URL is on `cua-spaces://app/`, and
  only for channels on the allow-list. Every result is wrapped in
  `{ ok, result | error }`.
- The window cannot navigate away from the app origin. External `http(s)`
  links open in the default browser.
- The CSP is set on every `cua-spaces://` response: `default-src 'self'`.
  Scripts may be `'self'`, inline (for the pre-mount theme script) or
  `'wasm-unsafe-eval'`. `connect-src` allows `'self'`, `https:`, `wss:` and
  loopback `127.0.0.1:*` for the cua daemon. Dev mode also allows the dev
  server's origin and its WebSocket. Frames and objects are blocked.

## Native layer

`pnpm native [-- --target <rust-triple>]` (`scripts/build-native.mjs`)
builds, for one Rust target (the host's by default), into
`native/<platform>-<arch>/` (gitignored):

| File | What it is |
|---|---|
| `libcua_spaces_ffi.dylib` / `.so` / `cua_spaces_ffi.dll` | `libs/cua/crates/cua-spaces-ffi`: the cua SDK (`cua_sdk`) and the Spaces app core (`cua_spaces_ffi`), the library the SwiftUI app links |
| `cua_node_runtime.node` | uniffi-bindgen-react-native's N-API runtime, rebuilt Electron-safe by `libs/cua/scripts/build-node-runtime.mjs` (copies instead of external ArrayBuffers) |
| `cua` / `cua.exe` | The Spaces build of the CLI (`cua-spaces-cli`): the app's daemon, the keychain check, and the CLI it puts on PATH |

| Target | Directory | Built on |
|---|---|---|
| `aarch64-apple-darwin`, `x86_64-apple-darwin` | `darwin-arm64`, `darwin-x64` | a Mac (both, for the universal app: electron-builder packs each arch and @electron/universal lipos them) |
| `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | `win32-x64`, `win32-arm64` | Windows (the C runtime linked statically) |
| `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | `linux-x64`, `linux-arm64` | Linux |

electron-builder ships the directory of the arch it packs as
`Resources/native` (`afterPack` refuses a package without it). The main
process loads it from there when packaged, from `native/<platform>-<arch>`
in development, or from `CUA_SPACES_NATIVE_DIR`. Without it a normal launch
says so and quits: there is no sample data.

The TypeScript bindings are generated from the built library into
`src/native/generated/` and committed (`pnpm native -- --bindings`;
`--check-bindings` fails on drift, as CI runs on the Mac build). Flat enums
carry the Swift case names as values (`AppSpaceOs.Macos = "macos"`), so a
view encodes as the SwiftUI host's `BridgeValue` does. The bindings load
lazily, after `load.ts` named the library and runtime, in a chunk of their
own. The uniffi tools are the ones `libs/cua/typescript` pins; the MIT SDK
there does not depend on any of this.

## Bridge

```ts
interface CuaDesktop {
  platform: "darwin" | "win32" | "linux";
  invoke(channel: "cua:bridge", request: { id; method; args? }): Promise<{ id, ok: true, result } | { id, ok: false, error: { code, message } }>;
  on(channel: "cua:event", listener: (event: { event; payload? }) => void): () => void; // returns unsubscribe
}
```

The page runs the webkit adapter over this transport
(`apps/cua-spaces-web/src/bridge/transport.ts`), so the Electron shell
answers the SwiftUI host's methods (`WebUIBridge.methods`, `src/bridge/methods.ts`)
in its envelope, error codes and `cua:event` pushes, plus one of its own:
`spaces.openStream`, the media ticket for the page's video. `src/bridge/`
has one file per area; each registers its methods in the table
(`index.ts`), and `test/not-yet.ts` lists the methods still answering
`unimplemented`. `pnpm build` ends with `scripts/check-channels.mjs`: the
preload allows the bridge channel only, and the main bundle routes every
method.

Behind the bridge, `src/model/` is the SwiftUI app's model layer over the
same core: the launch (the keychain check through the bundled `cua` on
macOS, `cua daemon start`, then `Cua.auto` on this app's daemon, with
stand-ins that wait for it), the daemon's supervision (started again when it
stops answering; it is this app's when its executable is in the native
directory, the core's rule, or another app's of the same or a newer cua
version, which it uses rather than replaces:
`cua_daemon::identity::verdict`), the list poll with its reconnect, creates,
power and deletes through the core's state machines, the device sign-in, and
the first run's CLI install.

The released SwiftUI app 0.7.2 predates that rule and still replaces this
app's daemon, even a newer one, when both run.

The app core's settings and first-run state are `app-settings.json` and
`onboarding.json` in userData (on macOS taken over from the SwiftUI app,
`migrate-swift.ts`).

`CUA_SPACES_E2E_DEMO=1` is the parity flows' test switch: no native library,
no daemon, and the page plays the browser demo host (`?bridge=demo`).

### Tests

`pnpm test` runs vitest on `test/`: the method table against the Swift list
and the web contract, the envelope, `BridgeValue`, the loader, the daemon and
the launch. With `pnpm native` built, the bridge also runs on the real app
core over fixture Spaces, host and devices (`test/bridge-native.test.ts`):
every ported method in the shapes of `bridge-shapes.json` (the document the
Swift tests check), the create, cancel, power and delete paths, sign-in, the
events, and the page's adapter reading every answer; and
`test/native-smoke.test.ts` loads the library in Node with a throwaway HOME.
`ELECTRON_RUN_AS_NODE=1 <electron> node_modules/vitest/vitest.mjs run` runs
the same under Electron's Node.

## Teleport and the Keyvault

`teleport.*` and `keyvault.*` answer the SwiftUI host's methods in its shapes
(`src/bridge/teleport.ts`, `src/bridge/keyvault.ts`, over `src/model/teleport.ts`
and `src/model/keyvault.ts`, the ports of `TeleportModel` and `KeyvaultModel`).
The page runs the picker and the review on the app core; the host answers
what only it can: the SDK's catalog, windows, icons, previews, plan and run,
and the Keyvault broker.

- **What the host keeps between steps** is bounded as the Swift bridge's is:
  the latest catalog (plus the apps picked from a window since, at most 512),
  and each Space's latest plan, which a finished, failed or cancelled run
  spends. A run needs the plan the page names by its `json`; a stale one is
  `not_found` ("Plan the teleport again").
- **Consent** reaches the SDK through one record, every field of it: Save to
  Keyvault (the SDK defaults it off), the sites, the exclusions, the items
  sent from the Keyvault, the saved passwords. `teleport.progress` carries
  each step with the run's `runId`; the notch shows the transfer while it runs.
- **Prompts** are this app's own, native and never the page's: the unlock
  prompt (Allow, Deny, Never ask again) and the delete confirmation are
  message boxes over the window that asked, in the core's words. A passphrase
  vault asks in a small window of its own (`src/passphrase.ts`: a page built
  in the app, no network, its own preload, the core's strength check). The
  passphrase goes from there to the main process to the broker over the
  verified Keyvault socket; it never crosses the page's bridge, and the page
  sees only `{recoveryKey}`.
- **The notch** shows the live-access line from the Keyvault's sign-ins, less
  the copies dismissed (`src/notch-feed.ts`); Dismiss hides and revokes or
  wipes nothing. The app reads the broker every 10 s while it runs, so access
  is never silent. An app dropped on a tile opens that Space's Teleport review
  at the app (the page's `?dropped=` names; the host holds the paths). A window
  dragged onto the notch is not wired yet (no window-drag monitor in this
  host).
- **Presence prompts** are the system's, shown by the daemon (Touch ID's
  sheet, Windows Hello, the desktop's polkit dialog). On Windows and Linux the
  host brings the app forward first, so the dialog opens over it.

On a Mac the picker's Open windows tab, window previews and the window drag
onto the notch are the SDK's (`listWindows`, `captureWindowThumbnail`,
`startWindowDrag`). The SDK has none of them on Windows or Linux, and neither
has the Swift app: the tab is empty there and the apps come from the catalog
(Start Menu shortcuts on Windows, `.desktop` entries on Linux), with their
icons where the SDK reads them (Linux; Windows has none yet).

### Who is trusted, where the vault key lives, what asks for you

The vault lives in the cua daemon (`cua-keyvault`), not in this app: the
Electron app, like the SwiftUI app, is a client of the broker's socket, and
the broker decides who may ask and when the user must confirm. This table is
what the SwiftUI app has and what each OS gets here.

| | SwiftUI app (macOS) | Electron, macOS | Electron, Windows | Electron, Linux |
|---|---|---|---|---|
| Vault key | 256-bit key in a login-keychain item (service `Cua Keyvault`), its access list trusting the creating binary's code requirement; wraps the vault master key. Not the Secure Enclave, no biometric access flag on the item | The same item, the same daemon identity (`com.trycua.cua`), so a vault the Swift app made opens without a keychain prompt | Credential Manager entry (DPAPI, this account): any program of the account can read it, there is no per-program access list | Secret Service entry (the login keyring): any program of the user on the session bus can read it |
| Confirmation before access widens | `LAContext` `deviceOwnerAuthentication`: Touch ID, Apple Watch or the login password, asked by the daemon | The same | Windows Hello (`UserConsentVerifier`): face, fingerprint or PIN, asked by the daemon over the foreground window | polkit `ai.cua.spaces.keyvault` (`auth_self`): the user's password or fingerprint through the session's agent, every time; installed by the `.deb` only |
| Who the broker lets in | Audit token of the socket peer, `SecCode` validity, requirement "Apple-anchored, team YCK386LBJ7, identifier in the Cua list", hardened runtime | The same: the app is `com.trycua.spaces.macos`, its daemon `com.trycua.cua` (`packaging/sign-mac.cjs`) | The pipe's client process id, its image file's Authenticode signature (chain to a trusted root, nothing fetched) and its publisher equal to this process's. No hardened-runtime flag exists; the running image is not re-checked | `SO_PEERCRED` and `/proc/<pid>/exe` inside `/opt/Cua Spaces` (or another Cua install root) in a tree only root can change. Not OS-verified: same-user processes can ptrace each other |
| Transport | Unix socket `$CUA_HOME/keyvault.sock`, 0600 in a 0700 directory | The same | Named pipe `\\.\pipe\cua-keyvault-<hash of that path>`: this account only, no remote clients, first instance unique, identification-level impersonation | The Unix socket, as macOS |
| Anti-rollback counter | Login-keychain item outside the vault folder | The same | A file beside the vault folder (the weaker, portable floor) | The same file |
| Third parties | Ask, wait on the Keyvault page, and are approved with confirmation | The same | The same | The same |

Not matched, and why: Windows and Linux have no per-program access list on
the vault key and no code-signature check of the running image (macOS's
`SecCode` and hardened runtime); Windows Hello and polkit confirm the user but
do not bind the key to the TPM or the device (the Secure Enclave is unused on
the Mac too). A Linux AppImage runs from a user-owned mount, so it is never a
Cua install root: the Keyvault is off there, and the page says the broker is
not Cua. The page's own words still say "Touch ID" and "keychain" on Windows
and Linux: they come from the shared app core, which also runs as wasm.

### This machine, Settings and the system

`src/bridge/host-setup.ts`, `devices.ts`, `settings.ts`, `about.ts`,
`storage.ts`, `volume.ts`, `notifications.ts` and `onboarding.ts` answer
This machine, Devices, Settings (General, Runtimes, Experiments, About,
launch at login, Storage), Cua Volume, Notifications and the first run, over
the SwiftUI app's models ported to `src/model/` (`host.ts` and
`host-failure.ts`, `devices.ts`, `login-item.ts`, `updates.ts`,
`storage.ts`, `notifications.ts`, `onboarding.ts`, `local-network.ts`,
`security-agent.ts`). `host-parts.ts` makes them once
per app model; `host-start.ts` runs what goes on while the app runs (a
device asking to join is a notification, the account's devices every
minute, the notifications feed, launch at login for an install that never
chose, whether the keychain prompt is still on screen). Per system:

- **Host setup** is the SDK's on each system: launchd on macOS, a scheduled
  task on Windows, systemd (or a plain process) on Linux. Only macOS has
  privacy panes to grant (`host.openSettings` opens only those) and Local
  Network access to ask for (a datagram to the LAN and vmnet when the first
  run ends and when this Mac provides Spaces).
- **Approving a device** asks the person here first, with the system's own
  prompt through the app core (`appConfirmPresence`, `model/presence.ts`):
  Touch ID, an Apple Watch or the login password on macOS (as the Swift app
  asks), Windows Hello on Windows, polkit's `ai.cua.spaces.devices`
  (`packaging/ai.cua.spaces.policy`, installed by the `.deb`) on Linux.
  polkit shows each action's own message: it takes details only from root,
  so `pkcheck` gets none. Its exit status is read as pkcheck(1) gives it (1
  not authorized, 2 no agent to ask, 3 dismissed); an error (127) is shown as
  an error, never as the user's refusal.
  Where Windows Hello is not set up or polkit cannot ask, a dialog of the
  app's own says so and asks instead.
- **Launch at login**: macOS's SMAppService (Electron's `mainAppService`,
  which can wait for approval in System Settings), Windows' startup apps
  (turned off in Task Manager reads as waiting for approval), an XDG
  autostart entry on Linux. A login launch (`--hidden` on Windows and Linux)
  starts without the window once the first run is done. On Windows the Run
  entry is `ai.cua.spaces.desktop` (the AppUserModelID, which Electron reads
  it under) and the app gives Electron its path quoted: Electron finds the
  entry by parsing that path as a command line, and an unquoted
  `...\Cua Spaces.exe` matches nothing, so a working entry read as waiting
  for approval.
- **Updates**: electron-updater, opt-in and beta only (`src/updater.ts`);
  About says so, and Check Now on Stable says Stable updates come later.
  Sparkle's relaunch prompt for a copy replaced on disk has no Electron
  counterpart (electron-updater installs on quit or relaunch).
- **The first run** is the page's (`routes/onboarding`, the app core's
  flow); `onboarding.json` is the Swift app's file, and Settings' Welcome
  "Show again" makes it due again.

## Live video

On every platform, macOS included, the page draws each Space's live desktop
itself, with WebCodecs. No host draws native video over the page here, as
the SwiftUI app does (`apps/cua-spaces-macos/docs/native-video.md`); Chromium
decodes on the GPU instead (VideoToolbox on macOS, Media Foundation on
Windows, VA-API on Linux). The tiles on the Spaces grid, the viewer on a
Space's page and the picture-in-picture panels use the same tiers, phases,
input and fallbacks as the Mac's native slots. [docs/video.md](docs/video.md)
has the design, how it compares with the SwiftUI app, and the bench that
measures both on one Space.

1. **Ticket.** The page asks for `spaces.openStream {spaceId, tier, windowId?, epoch?, activate?}`
   (`apps/cua-spaces-web/src/bridge/ops/stream.ts`). The bridge answers it
   in `src/media-bridge.ts` with the daemon's `DaemonService.OpenMediaBridge`:
   - one gRPC-Web call over HTTP/1.1 on the daemon's loopback listener,
     with the bearer token from `$CUA_HOME/daemon.json`;
   - the protobuf is hand-encoded, so there is no gRPC dependency;
   - the Space id goes in as the sandbox name, as the Tauri app sends it;
     the daemon resolves local, cloud and relay Spaces;
   - it starts the daemon (`cua daemon start`) when there is no
     `daemon.json`, or when the recorded daemon doesn't answer.

   The answer is `{wsUrl, expiresAt}`: a loopback WebSocket that speaks rcdp
   wire v2. Without cua it is `unsupported`, and the page keeps the drawn
   thumbnail and "Open window", as in a browser.
2. **Tiers.** These match the macOS app's `VideoTier`:
   - tiles ask for 10 fps, a 960 px long edge and view only;
   - the viewer takes the Space's defaults (30 fps, native size) with input
     (`SESSION_POLICY_ALLOW_ACTIVATION`).
3. **Slots.** `components/video/webcodecs-slots.ts` in the web app loads
   only in this shell. It gives each slot a `<canvas>` and the production
   `MediaSession` (`libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession.ts`):
   - the canvas shows from the first decoded frame (`live`);
   - `reopen` mints a fresh ticket when the old one expires or the socket
     closes with 4401;
   - a session that ends, or a ticket that doesn't come, is `failed`.
   The socket goes from the renderer straight to the daemon, so video never
   passes through IPC. `connect-src` already allows `ws://127.0.0.1:*`.
4. **Input.** In the viewer, a click gives the Space the keyboard. While it
   has it, every key goes to the Space, Command chords too (the SwiftUI
   app's `KeyCapture`): the window's menu shortcuts stand aside
   (`src/keyboard.ts`, `setIgnoreMenuShortcuts`) and the page's shortcuts
   don't see them. On a Mac, Control+Option pressed and released alone gives
   the keyboard back (⌘Esc goes to the Space); elsewhere Ctrl+Shift+F12 does.
   Ctrl+Esc is not used: it opens the Start menu on Windows, and RDP clients
   and Linux desktops claim Ctrl+Alt combinations. Shortcuts the OS reserves
   (⌘Tab, ⌘Space, ⌘⇧3/4/5, Mission Control) stay on the host. Tiles are view
   only. Scrolls go to the page until the viewer has the keyboard. On a Mac,
   ⌘ goes to a Linux or Windows Space as Control (⌘C arrives as Ctrl+C), and
   a ⌘ chord goes whole (press and release: Chromium drops its key-up).
5. **Pausing.** Every session closes while the page is hidden (the window is
   minimized or on another desktop). They reopen with fresh tickets when it
   shows again.
6. **Picture in picture.** A Stream section row's PiP button (`stream.pip`)
   opens a floating window (`src/pip.ts`): on top of every window and
   desktop, no frame, the stream's shape, 480 px wide where the last one was
   left. It shows the web UI's `/pip` view, a session of its own on the
   desktop or one window (in the background, as the SwiftUI app streams a
   window), with Open Space and close on a bar that shows under the pointer.
   Closing it is popping it in.

### Measured on Windows and Linux

Two Azure D8s_v3 VMs (8 vCPU Xeon, no GPU, so decode and compositing run in
software), 5 October 2026:

- Windows 11, in an SSH session with no interactive desktop;
- Ubuntu 24.04 XFCE on Xvfb.

The packaged app ran over CDP against a stream feeder
(`CUA_SPACES_TEST_STREAM_URL`), with 3 Spaces running, tiles on `tile10` and
the viewer on `full60`. CPU is summed over the app's process tree, in % of
one core.

| | Windows | Linux |
|---|---|---|
| App: 3 tiles | 10.3 fps each, 30% CPU, 672 MB | 10.0 fps each, 29% CPU, about 1 GB RSS |
| App: viewer, 1080p60 source | 21 fps, 72% CPU | 33 fps, 100% CPU |
| App: minimized | 0 frames sent; back within 2 s of restoring | the same |

Tiles hold their 10 fps with nothing dropped. Without a GPU, the viewer
falls well short of 60 fps while using less than two cores, so presenting
frames is the limit, not decoding. Numbers on real hardware with a GPU are
still to come; `pnpm video:bench` ([docs/video.md](docs/video.md)) measures
them.

## The macOS notch

On macOS 26 and later the app shows the same notch as the SwiftUI app: the
"N Spaces" tab beside the camera housing, the hover cue and dwell, the Space
tiles (OS logo, where each Space runs, status, thumbnail), the search and the
Spaces and Settings buttons, the activity ring, live Keyvault access, the
window-drag "Teleport to Cua" box and the permission line, and the
"Spaces tab in the notch" setting. On a Mac without a notch it is the
SwiftUI app's capsule at the top of the main display.

It is drawn by "Cua Spaces Notch.app", built from `libs/spaces-notch-swift`
with the SwiftUI app's own notch views (`CuaSpacesNotchUI`). `pnpm notch`
builds it universal (release, arm64 + x86_64) into `native/notch/`, ad hoc
signed so an unsigned build runs, and runs its `--selftest` (no window).
`electron-builder.config.cjs` copies it to `Contents/Helpers/`, where a
signed build signs it with the app's identity and hardened runtime, so it
notarizes with the app; it is already universal, so the universal merge keeps
it as is.

The app core's notch model runs in main (`src/notch/model.ts`: the reducer,
its dwell and close timers, the layout for the helper's screen, window drags
and the drag permission, the tiles' thumbnails). The helper only draws what
it is sent and sends input and clicks back, one JSON object per line on its
stdin and stdout (`src/notch/protocol.ts`; the Swift side is
`NotchProtocol.swift`, and both sides' tests read the same fixtures). It exits
when its stdin closes, so it never outlives the app; `src/notch/process.ts`
restarts it with a growing delay (0.5 s doubling to 30 s, giving up after ten
short runs in a row). `connectNotch(host)` in `src/notch.ts` is the one call
that starts it, with the cua-spaces-ffi bindings and the model's handlers;
the model then feeds it the Spaces, the Keyvault sign-ins, the activity and
the setting (`src/notch-feed.ts`). A tile shows the Space's thumbnail from
the app's store (`SpaceThumbnails`, kept a minute or two old for running
Spaces while the app is in use); its click selects the Space and shows it;
files dropped on it are sent into the Space with the drop well's transfer,
the activity indicator on meanwhile, and a notification says what landed.

## Packaging and signing

`electron-builder.config.cjs` builds without signing unless the signing
environment is present:

- **macOS: Developer ID and notarization.** Set `CSC_LINK` and
  `CSC_KEY_PASSWORD`, or `CSC_NAME`, to sign with the hardened runtime and
  `packaging/entitlements.mac.plist`. Add `APPLE_API_KEY`, `APPLE_API_KEY_ID`
  and `APPLE_API_ISSUER`, or `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and
  `APPLE_TEAM_ID`, to notarize as well. Without them, `identity` is `null`
  and the app keeps Electron's ad-hoc signature, which is enough to run
  locally on Apple silicon.
- **Windows: Azure Trusted Signing.** Set `AZURE_TENANT_ID`,
  `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`,
  `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` and
  `AZURE_SIGNING_PUBLISHER` to fill in `win.azureSignOptions`. Without all
  seven, nothing is signed. Azure signing runs through `signtool` on a
  Windows host, so a signed Windows build has to come from Windows CI, not
  from a macOS cross-build.
- **Linux.** AppImage and deb, no signing. The AppImage uses electron-builder's
  static runtime (`toolsets.appimage: "1.0.3"`), so it needs neither FUSE 2
  nor `libz.so` on the host. The default runtime's arm64 build links the
  unversioned `libz.so`, which stock distros only ship with `zlib1g-dev`, and
  did not start in the smoke test. The compile cache is skipped under
  AppImage, because it mounts at a new path on every launch. The deb's
  `/opt/Cua Spaces` is root's and writable by root alone, whatever the build
  machine's umask: `afterPack` gives folders 755 and files 755 or 644
  (`packaging/linux-permissions.cjs`), as the Keyvault's trust check needs of
  every folder above the bundled `cua`. The desktop entry's Comment and the
  package description are `linux.description`.

Both signing paths were checked only as far as the config: with dummy values
set, the config turns on the hardened runtime, notarization and
`azureSignOptions`; without them, every artifact in `dist/` is unsigned (the
Windows check reads the PE security directory).

The icons are the existing Cua Spaces icons in `apps/cua-spaces/src-tauri/icons/`,
used in place and not modified. `packaging/linux-icons/` holds symlinks to
them named by size, so the deb installs 32 to 512 px into `hicolor`. The tray
uses `icon.ico` on Windows and `32x32.png` on Linux, copied to
`resources/tray/`. The built web UI, without source maps, is copied next to
the asar as `resources/web/` when `../cua-spaces-web/dist/index.html` exists.

### Windows and Linux builds

All of these cross-build on an Apple silicon Mac with no Docker or Wine:
electron-builder brings its own NSIS, 7-Zip, mksquashfs and fpm.

electron-builder's macOS `makensis` (x86_64, under Rosetta) aborts with
SIGABRT, and `dist:win` fails with `ERR_ELECTRON_BUILDER_CANNOT_EXECUTE`, when
an `!include` path in its NSIS templates is about 280 characters long. With
pnpm's default `.pnpm/` directory names that happens in a checkout a little
deeper than `~/repo/<worktree>/`. `virtualStoreDirMaxLength: 40` in
`pnpm-workspace.yaml` keeps those paths short.

```sh
cd ../cua-spaces-web && pnpm install --ignore-workspace && pnpm build && cd -
pnpm install       # here, without --ignore-workspace (see the top of this file)
pnpm dist:win      # dist/Cua-Spaces-Setup-<v>-x64.exe, -arm64.exe, and both in one
pnpm dist:linux    # dist/Cua-Spaces-<v>-x86_64.AppImage, -arm64.AppImage, cua-spaces_<v>_{amd64,arm64}.deb
```

A release packs both arches and stops at one without its native layer.
A local build that has only some `native/<platform>-<arch>` folders sets
`CUA_SPACES_ARCHS=native` to pack just those arches (a mac universal build
needs both darwin folders), or a list such as `CUA_SPACES_ARCHS=x64`
(`packaging/archs.cjs`). On Windows: `set CUA_SPACES_ARCHS=native` in cmd,
`$env:CUA_SPACES_ARCHS="native"` in PowerShell, then `pnpm dist:win`.

Building on Windows itself (`pnpm native`, then `pnpm dist:win`) needs, besides
Node and pnpm: Rust with the MSVC toolchain, the Visual Studio C++ build
tools, and CMake on PATH (some of the native layer's C dependencies build with
it). The scripts run under cmd.exe and PowerShell: `pnpm build` removes
`dist-electron` with Node, and `pnpm native` runs `npm.cmd` through the shell.

Sizes for 0.6.0 with the real web UI, built on 2026-10-03 (MB = 10^6 bytes):

| Artifact | Size |
|---|---|
| `Cua-Spaces-Setup-0.6.0-x64.exe` | 103.4 MB |
| `Cua-Spaces-Setup-0.6.0-arm64.exe` | 97.3 MB |
| `Cua-Spaces-Setup-0.6.0.exe` (x64 and arm64; picks at install, and is the one `latest.yml` points to) | 200.3 MB |
| `Cua-Spaces-0.6.0-x86_64.AppImage` | 114.7 MB |
| `Cua-Spaces-0.6.0-arm64.AppImage` | 114.9 MB |
| `cua-spaces_0.6.0_amd64.deb` | 98.9 MB |
| `cua-spaces_0.6.0_arm64.deb` | 94.1 MB |
| Installed app, Windows x64 / arm64 | 331 / 333 MiB |
| Installed app, Linux x64 / arm64 | 280 / 299 MiB |
| `app.asar` (shell, preload, electron-updater) | 1.3 MB |
| `resources/web` (web UI and core wasm) | 5.2 MiB |

Nearly all of it is Electron itself.

### Upgrade and uninstall

The cua daemon runs from the app's `resources/native` and outlives the app
(it keeps the Spaces running). What each package does about it, and what it
leaves when removed:

- **Windows (NSIS, `packaging/installer.nsh`).** The installer and the
  uninstaller check for the running app before touching a file; that check
  is followed here by `cua daemon stop` with the installed `cua`, then by
  stopping anything still running from the install folder. In the installer
  that comes before the old version's uninstaller and before the new files
  are written, so an upgrade replaces `cua.exe` (a running executable cannot
  be overwritten) and an uninstall removes the folder. Uninstalling, not
  upgrading, also removes the launch-at-login Run entry, and the `cua` the
  app copied to `%LOCALAPPDATA%\Programs\cua\bin` with its user PATH entry,
  only as far as the app recorded doing so (`HKCU\Software\ai.cua.spaces.desktop`,
  written when it installs the CLI): a `cua` the CLI's own installer put
  there is left alone.
- **Linux deb.** Removing the package removes `/opt/Cua Spaces`; its scripts
  leave users' home folders alone. The CLI the app puts on PATH is a link
  to the package's `cua` (`~/.local/bin/cua`, as the Swift app links into its
  bundle), so it goes with the package and upgrades with it. The autostart
  entry carries `TryExec`, so desktops skip it once the app is gone.
- **AppImage.** An AppImage stays mounted while any process started from it
  runs. The daemon is started from a copy of the bundled `cua` in
  `~/.local/share/cua-spaces/daemon/<build>/` (`src/model/appimage-cua.ts`),
  so quitting the app unmounts it while the daemon keeps running; the copy of
  an older build is removed when a newer one starts.
- **Everywhere**, `~/.cua` (Spaces, the Keyvault, settings the CLI shares)
  and the app's data folder are the user's and stay, as with the Swift app.

### What is tested

`pnpm smoke:linux` runs each arch in an Ubuntu 24.04 container
(`scripts/smoke/linux.Dockerfile`), arm64 natively and x64 under emulation.
Results go to `dist/smoke/linux-<arch>/`. Both arches pass:

- the deb installs to `/opt/Cua Spaces/`, links `/usr/bin/cua-spaces`, and its
  desktop entry passes `desktop-file-validate`, with
  `StartupWMClass=cua-spaces` and icons at 32, 64, 128, 256 and 512 px;
- the AppImage starts under Xvfb and openbox, maps a window titled
  "Cua Spaces" with `WM_CLASS` `cua-spaces`, and renders the Spaces page
  (`screen.png`);
- with `CUA_SPACES_CAPTURE_ROUTES`, all six routes in light and dark report
  `data-bridge="electron"`, which is the bridge's `mode: electron`
  (`routes.json`, 12 captures).

Under emulation, binfmt_misc does not recognise an ELF with the AppImage
magic at offset 8, so the x64 run zeroes those three bytes in a copy first.
The arm64 run uses the file unchanged.

`pnpm smoke:win` unpacks each NSIS installer with 7-Zip in a container and
checks, per arch: NSIS 3 Unicode, the right `app-64.7z` / `app-arm64.7z`
payloads, the uninstaller, the PE machine type (x64 or ARM64) and GUI
subsystem, version resources (product Cua Spaces, file version, company),
the icon resource, `resources/app.asar` with the shell bundles and
electron-updater, `resources/web/`, the tray icon, `app-update.yml`, no
source maps, locales trimmed to English, and that nothing is signed. The
report is in `dist/smoke/windows/report.txt`. All checks pass.

### What is not tested

- **Windows at runtime, in part.** On a Windows 11 VM (5 October 2026),
  over SSH with no interactive desktop, these were checked: silent install,
  upgrade and uninstall, the shortcuts, launch, every page on demo data,
  the overlay's titlebar area, and the WebCodecs path (above). Still
  untested on Windows: the window chrome and overlay colours as drawn, the
  tray icon and close-to-tray, the AppUserModelID in the taskbar,
  SmartScreen for the unsigned installer, and the arm64 build on real ARM64
  hardware.
- **Linux desktops.** Xvfb with openbox in Docker, and XFCE (xfwm4, its
  panel and tray) on Xvfb on an Ubuntu 24.04 VM. On the VM the deb and the
  AppImage (through FUSE) both start, with no white flash at launch in light
  or dark, the Cua Spaces icon in the task list and the tray, and the
  WebCodecs path (above). Not tried: GNOME or KDE on
  Wayland (the app id), a real tray (Xvfb has none, so the tray menu is
  unverified), AppImage through FUSE rather than extract-and-run, and the
  Electron sandbox (the container runs as root, so it uses `--no-sandbox`;
  the deb's postinst sets up the AppArmor profile and `chrome-sandbox`).
- **x64 Linux on real hardware.** Only under emulation on an arm64 host.
- **Signing and notarization** with real credentials, and **updates** end to
  end: only CI holds the credentials, and only a prerelease publishes a feed.

## Updates

electron-updater reads a generic feed on the rolling `cua-spaces-latest`
release of the repository the build came from (`GITHUB_REPOSITORY` at build
time, default `trycua/cua`, baked into `app-update.yml`). GitHub's own
provider does not fit this monorepo: it needs semver tags and reads the
repository-wide latest release.

Electron builds ship as Cua Spaces prereleases only (`X.Y.Z-suffix`, built
and published by `cd-cua-spaces.yml`, see "Releases" below), so the only
feed is the beta channel (`beta.yml`, `beta-mac.yml`, `beta-linux*.yml`).
The updater is **off by default**: set `CUA_SPACES_AUTO_UPDATE=1` or put
`"autoUpdate": true` in `settings.json` to opt in. A build follows the
channel it was cut for; `"updateChannel": "beta"` or `"stable"` in
`settings.json` overrides it. The stable channel is served only after the
Sparkle cutover (`STABLE_FEED` in `src/updater.ts`,
[docs/sparkle-cutover.md](docs/sparkle-cutover.md)). Nothing downgrades.

## Releases

The app has the cua-spaces version (Release Please bumps `package.json` with
the macOS app). A prerelease tag `cua-spaces-vX.Y.Z-suffix` runs
`cd-cua-spaces.yml`, which builds and signs this app for macOS (arm64 and
universal dmg and zip, Developer ID, notarized), Windows (one NSIS
installer for x64 and arm64, Azure Artifact Signing) and Linux (AppImage and
deb, x64 and arm64), uploads them to that GitHub prerelease, and the
`beta*.yml` feed files (with absolute installer URLs, `scripts/feed-urls.mjs`)
to `cua-spaces-latest`. Stable releases ship only the Swift app. A manual run
with `electron_dry_run` builds and signs the same artifacts and keeps them as
workflow artifacts without publishing anything.

On macOS the bundle id is `com.trycua.spaces.macos`, the Swift app's, with
its usage descriptions, Apple Events entitlement and Sparkle public key, so
a single Sparkle update can replace the Swift app in place and keep its
permissions. Installing a beta on a Mac with the Swift app replaces it
(same name and id). On its first launch the app copies the Swift app's
settings and onboarding files (`~/Library/Application Support/com.trycua.spaces.macos/`)
to `app-settings.json` and `onboarding.json` in its user data folder and
takes its window frame and update channel (`src/migrate-swift.ts`); the
Swift app's files stay as they are.

## Environment variables

| Variable | Effect |
|---|---|
| `CUA_SPACES_DEV_URL` | Proxy this dev server instead of serving files. Ignored in packaged builds |
| `CUA_SPACES_USER_DATA` | Use a different user data directory (for tests and measurements) |
| `CUA_SPACES_LOG_STARTUP` | Log the web root and the time to `ready-to-show` |
| `CUA_SPACES_CAPTURE=<png>` | Save a `capturePage` image after load, then quit |
| `CUA_SPACES_CAPTURE_ROUTES=<dir>` | Load every route in light and dark at 1440×900, check that the page is on the Electron bridge, save `capturePage` PNGs to `<dir>`, print a JSON report, then quit |
| `CUA_SPACES_QUIT_AFTER_LOAD` | Quit after the first load (used by `pnpm measure`) |
| `CUA_SPACES_AUTO_UPDATE=1` | Turn on the updater |
| `CUA_SPACES_VERSION` | Build time: the version (default `package.json`); `X.Y.Z-suffix` is a beta (`beta*.yml`) |
| `CUA_SPACES_BUILD_NUMBER` | Build time: the last part of CFBundleVersion and the Windows file version, `X.Y.Z.N` (default 0) |
| `GITHUB_REPOSITORY` | Build time: the repository whose `cua-spaces-latest` release is the update feed (default `trycua/cua`) |
| `CUA_SPACES_NO_FUSES=1` | Build time: leave the Electron fuses as Electron ships them (QA builds only) |
| `CUA_SPACES_ARCHS` | Build time: `native` packs only the arches with a native folder, or a list (`x64`, `arm64,x64`); unset packs all (a release) |
| `CUA_SPACES_TEST_STREAM_URL` | Answer every `spaces.openStream` with this URL (`{id}`, `{tier}` filled in), for the WebCodecs harness |
| `CUA_BIN`, `CUA_HOME` | Where to find `cua` and `~/.cua`. A `CUA_BIN` that doesn't exist means no cua (demo data) |
