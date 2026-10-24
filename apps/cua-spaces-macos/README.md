# Cua Spaces for macOS (SwiftUI)

A native macOS 26 app for Spaces and the Keyvault. It is a thin SwiftUI shell
over the same app core as the Tauri app (`apps/cua-spaces`): every product
decision lives in `libs/cua/crates/cua-spaces-app-core`, reached through the
cua SDK's Swift package (`libs/cua/swift`, module `CuaSDK`). The views only
render and do platform glue.

| Surface | What it is |
|---|---|
| Main window | Sidebar with Spaces (one section per location; a dragged window drops on a row) and the Keyvault (All, Waiting, Access, Recent, one row per site) with the global Keyvault switch; Space detail with the live desktop, per-window streams, picture in picture, agent runs and the Teleport drop well |
| New Space | Step-by-step sheet: System, Resources, Options, Summary; Connect by address |
| Keyvault | Sidebar and list: categories, a site's accounts with their unattended switches, approval sheets |
| Onboarding | The shared web onboarding (`/onboarding` in `apps/cua-spaces-web`), in the New UI window, whether or not the New UI setting is on; the app draws none of its own. The page asks and finishes it through `onboarding.get` and `onboarding.complete` (the state is `onboarding.json`); a Space it creates posts a "<Space> is ready" notification (`notifyWhenReady`) that opens the Space. With the New UI off, the window gives way to the native one when the first run ends |
| Devices | Settings → Devices: this Mac's enrollment (enrolled until, grace ends, needs enrollment), the account's devices with Rename and Revoke, Recent Access; the Enroll sheet (sign in again, or a one-time code approved from an enrolled device), the approval sheet for a device asking to join (a notification too; Approve asks for Touch ID or the login password), and a banner in the main window when this Mac needs enrolling |
| Menu bar | The Cua mark (template); status line, Open, New Space, Settings, Quit |
| Launch at login | Settings → General: `SMAppService.mainApp`, showing what macOS reports (on, off, waiting for approval with a button to Login Items, not found); the first run's Done page (the web onboarding's) ticks it. A login launch opens without the main window, and the app's daemon (host Spaces, persistent agents, Cua Volume) starts with it |
| Join with a code | `host.joinWithCode` runs the bundled `cua host setup --pair <code>` (3 minute limit); `cua://add/<CODE>` opens Machines at `?join=<CODE>` and the page asks before joining |
| Notch panel | The "N Spaces" tab, Space tiles with live thumbnails, and the Teleport prompt; hover opens it after 300 ms, a dragged window or file drops on a tile. Window drags need Accessibility; the panel says so with an Open Settings button |
| Teams billing | Settings shows a failed-payment notice for a Teams plan, and adding a team computer without a paid plan fails at once with an Upgrade button (owner) or "Ask the team owner" (member) |

Keyboard in the live desktop (main window and floating viewer): once you
click the stream it has the keyboard, and Command chords go to the Space
instead of the app's shortcuts (Super on a Linux guest, Command on macOS).
Press and release Control+Option, or click outside the stream, to give
Command shortcuts back to the Mac. Shortcuts macOS reserves (⌘Tab, ⌘Space,
⌘⇧3/4/5, Mission Control) stay on the Mac: capturing them would need an
event tap and the Accessibility permission. Policy: `KeyCapture` in
`libs/spaces-app-swift`.

## One core, two shells

```
apps/cua-spaces (Tauri)        apps/cua-spaces-macos (SwiftUI)
  React: render only             SwiftUI: render only
  src/model/*.ts over wasm       view models over UniFFI
          \                           /
      libs/cua/crates/cua-spaces-app-core
      (Space list, wizard, teleport, Keyvault, onboarding, notch)
```

Both apps ship from the same core, and the parity flows
(`cua-spaces-app-core/parity`) replay identically through Rust, the Tauri
shell, the webview and Swift. See `architecture.md` in the work notes, or the
core's crate docs.

## Build and run

```sh
# The cua SDK library (release) for the Swift package:
cd libs/cua && cargo build --release -p cua-sdk && node scripts/stage-uniffi-library.mjs --only=swift
cd ../../apps/cua-spaces-macos
swift build                      # the package
scripts/test.sh                  # unit, parity and snapshot tests (swift-testing)
scripts/build-app.sh release     # .build/app/Cua Spaces.app, ad hoc signed
```

`SNAPSHOT_RECORD=1 scripts/test.sh` re-records the snapshot references.
UI tests (XCUITest) need Xcode: `xcodegen generate` (project.yml), then
`xcodebuild test -scheme CuaSpacesMacUITests`.

Environment (debug builds only; release builds ignore every `CUA_SPACES_*` hook):
`CUA_SPACES_FIXTURES=1` runs on fixtures (no daemon, no Space, an in-memory login item whose status `CUA_SPACES_LOGIN_ITEM` sets: `enabled`, `notRegistered`, `requiresApproval`, `notFound`; a relay allowance `CUA_SPACES_RELAY_USAGE` sets: `reached`, `team`, `none`, default the free plan within it);
`CUA_SPACES_START_VIEW` opens `new-space`, `keyvault`, `approval`, `settings-devices`, `settings-about`, `device-approval` (with `CUA_SPACES_APPROVE_CODE`), `device-enroll`, `device-enroll-code`,
`onboarding`, `stream` (the Space detail at its Stream section), `window-pip` (the same, with the first window popped out), `teleport-picker` (the Space's teleport picker; set `CUA_TELEPORT_APP_ROOTS` to fixture apps for images), `run-timeline` (an agent run's conversation from the Space's Agents section; fixtures give `local:aurora` two runs) or a notch state (`notch-open`, which springs
open 1.5 s after launch, `notch-hover`, `notch-permission`,
`notch-drag-target`, `notch-drop`); `CUA_SPACES_SELECT`
selects a Space; `CUA_SPACES_CREATE=<image>[,<name>]` creates one on this
Mac through the real create path (end-to-end runs); `CUA_SPACES_NOTCH_HIGHLIGHT` (`tile`, `list`, `settings`,
`search` or `tab`, optionally `:pressed`) forces a notch control's hover or
pressed look; `CUA_SPACES_UPDATER=live` runs Sparkle in a fixtures run (the feed in
Info.plist; use a test bundle identifier and a local feed), `CUA_SPACES_UPDATE_CHECK=background`
checks at launch, and `CUA_SPACES_REFRESH=1` runs the after-update refresh in a fixtures
run (with a throwaway `HOME`). State lives under `$HOME/Library/Application Support/com.trycua.spaces.macos`.
`CUA_SPACES_FIXTURE_PEER_CUA=<version>` (fixtures) signs the fixture account in with one Linux machine, `ellieln`, that shares its desktop, provides a Space and runs that Cua (This Mac reports 0.6.0), and `CUA_SPACES_FIXTURE_UPDATE=<version>` has that Cua Spaces update downloaded; `CUA_SPACES_WEBUI_CAPTURE_SELECT=<machine id>` captures the Machines page with that machine chosen;
`CUA_SPACES_OFFSCREEN=1` runs with nothing on screen, for captures on a Mac in use (`OffscreenRun`): an accessory app (no Dock icon or menu bar), no menu bar item, notch or global shortcut, no activation, and transparent click-through windows that still paint, so `CUA_SPACES_WEBUI_CAPTURE` works (with fixtures and a throwaway `HOME`, a first run captures `/onboarding`).
`CuaSpacesMac --check-launch-only` loads the app's libraries, prints its version and exits
(`scripts/check-launch.sh`; release builds too).

## New UI

The New UI (the `web_ui` experiment) makes
`WebUIWindowController` (`Sources/CuaSpacesMacKit/WebHost`) the main window: a
`WKWebView` with the shared React UI (`apps/cua-spaces-web`) in a window
without a title bar. Turning it on puts that window where the native one was
(same frame) and closes the native one; turning it off does the reverse
(`MainWindowSwitch`). While it is on, everything that opens the main window
opens it instead, at the matching page (`MainUIRouter`): the Dock icon, the
menu bar item, the notch, File, New Space (⌘N), the View menu, Help, Getting
Started and the app's notifications; the native window, opened any other way
(the Window menu), gives way to it. The notch, the menu bar extra, Touch ID, the updater and video stay
native; a Space's desktop opens in its native window.

It is on by default on the Beta update channel (the setting Sparkle uses)
from the 0.8.1 betas, and on every channel from 0.8.1 (0.8.0 shipped without it)
(`experiments::new_ui_default`). Settings, Experiments, "Use classic UI"
(`experiment:classic_ui`) is the way back; it saves the user's choice
(`AppSettings.new_ui`), which later launches keep. Only a choice is saved:
an older build's saved `webUi: false` was its default, so it reads as no
choice, and a saved `true` is kept (`settings::load`). A beta's first launch
starts on the Beta channel (`settings::first_launch_channel`).

- Release: `scripts/build-app.sh` copies the web build (`apps/cua-spaces-web/dist`,
  or `CUA_WEBUI_DIST`) to `Contents/Resources/WebUI`, served at
  `cua-spaces://app/`. Without it the window shows how to add it.
- Debug: `CUA_WEBUI_DEV=1` loads the dev server at `http://localhost:5174`
  (release builds ignore it). `CUA_SPACES_START_VIEW=webui` opens the window at launch.
  `CUA_SPACES_WEBUI_CAPTURE=<dir>` (debug) saves each route, light and dark,
  from `WKWebView.takeSnapshot` with a `report.json` of the bridge mode per page, then
  quits ([docs/webui/](docs/webui/)). Window captures from outside the app miss
  the web content, which draws in another process.
- The page gets `--titlebar-left-inset` (about 78px; 0 in full screen),
  `--titlebar-height` and `window.cuaHost` at document start. The window shows
  after the first page loads, over the theme's background (the page can set
  it with `window.setBackgroundColor`).

The bridge is the `cua` script message handler (`WebUIBridge.swift`):
`window.webkit.messageHandlers.cua.postMessage({id, method, args})` resolves
with `{id, ok: true, result}` or `{id, ok: false, error: {code, message}}`
(`code`: `unimplemented`, `bad_args`, `not_found`, `cancelled`,
`native_only`, `forbidden`, `unsupported`, `failed`). `app.info` lists the
methods. Native events arrive as `cua:event` window events (`spaces.changed`,
`machines.changed`, `keyvault.changed`, `session.changed`, `settings.changed`,
`agents.changed`, `update.changed`, `spaces.createProgress`, `spaces.newRequested`,
`settings.openRequested`). Results are the
core's records as the SwiftUI views get them (camelCase fields; enums as the
case name, or `{type, ...}` with a payload). `keyvault.get` also carries the
broker's redacted `overview` (items without `blob`) for the web vault list.
`spaces.cancelCreate`, `host.status`, `keyvault.approve`, `keyvault.deny`,
`keyvault.revokeGrant` and the seven `agents.*` methods use the web bridge's
operation names and arguments,
the same as the Electron router's channels
(`apps/cua-spaces-web/src/bridge/coverage.ts` lists every operation per host).
The web side of this contract is `apps/cua-spaces-web/src/bridge/webkit-protocol.ts`
and `adapters/webkit.ts`. The `agents.*` methods answer from `PersistentModel`
(persistent agents, pause and resume), `AgentRunsModel` (a Space's runs), the
daemon's `agent_events` tool and the Settings "AI agents" rows; without the
daemon they fail with `unsupported`. `machines.list` carries `host` (the
`host.status` record) for the "This machine" panel, and `settings.get` carries
`updateChannel` (null with no updater), set with
`settings.choose {row: "update-channel"}`. The other pages (New Space's
address form and clouds, Teleport and Share, Cua Volume, Settings,
Notifications, usage events, a Space's detail and This machine) are routed
under the web bridge's operation names in `WebUIBridge+Pages.swift`, each to
the view model or service its native screen uses. New Space runs in the
page: `spaces.createOptions` is the env the native sheet opens with
(`AppModel.newSpaceEnv`) and `spaces.create` runs its create
(`AppModel.runCreate`). While the `web_ui` experiment is on, every New Space
item (⌘N, the menu bar, the toolbar, a machine's "New Space on…") opens the
New UI window's wizard (`spaces.newRequested`); the native sheet is the
fallback when it is off. The app menu's Settings command (⌘,) opens the New UI's
Settings (`settings.openRequested`) while its window is the key window (or, with no key
window, the frontmost one), even when its web view has no focus; with any other window
in front it opens the native Settings scene (`WebUIWindowController.openSettings`).
Keyvault unlocks use the native
prompt and the daemon's Touch ID; a passphrase never crosses the bridge.

## Keyvault identity

The broker checks the calling process's code signature. Production policy
(`cua-keyvault/src/caller.rs`, `TrustPolicy::production`): Apple-anchored,
team `YCK386LBJ7`, identifier in `CUA_IDENTIFIERS`, hardened runtime.

- `com.trycua.spaces.macos` is in `CUA_IDENTIFIERS`, under the same
  team and hardened-runtime requirement as the Tauri app. A shipping build
  is signed with the Cua Developer ID and the hardened runtime; an ad hoc
  build (`scripts/build-app.sh`) is never first party. Nothing else
  changes: the security model is the same (nothing selected by default in an
  approval, the broker mediates delivery, no secret values in the app, the
  daemon asks for Touch ID to widen access).
- Development builds use the debug-only test requirement. With a debug
  `cua daemon` and a debug SDK library in the bundle
  (`CUA_SDK_DYLIB=<target>/debug/libcua_sdk.dylib scripts/build-app.sh`):

  ```sh
  CUA_KEYVAULT_TEST_REQUIREMENT="$(scripts/dev-keyvault.sh "<path>/Cua Spaces.app")" \
    cua daemon start --foreground
  # the app, allowed to talk to that unsigned daemon
  CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON=1 "<path>/Cua Spaces.app/Contents/MacOS/CuaSpacesMac"
  ```

  Such a daemon never touches the login keychain, so the Keyvault page
  offers a passphrase (twice at setup) instead of Touch ID. Add a debug
  `cua` to the `dev-keyvault.sh` arguments to use `cua keyvault` as well.
  Release daemons and release libraries ignore both variables.

## Keychain

The saved sign-in (`cua auth login`, shared by the app, its `cua` daemon
and the `cua` CLI) is a generic password in the login keychain: service
`run.cua.ai`, account `cua-cli`, plus `cua-cli.<name>` for named secrets
such as the device key (`cua-auth`). Only Cua-signed builds use the
keychain; ad hoc and unsigned builds keep it in `~/.cua/credentials.json`.

**The launch never waits on it.** The window (or New UI's) opens first,
on stand-ins for the live services (`Models/AppStartup.swift`). Then, off
the main thread:

1. The bundled `cua auth keychain` reads every Cua item with keychain
   prompts turned off (`SecKeychainSetUserInteractionAllowed(false)`,
   process-wide, which is why it runs in a `cua` of its own). Readable,
   none, or a file store: go on.
2. Not readable without a prompt: the window says "Allow Keychain access".
   Nothing asks until the user clicks Allow access, which runs
   `cua auth keychain --prompt`. While macOS shows its prompt, the window
   says "Waiting for Keychain access…" and, after 25 s, offers Try again
   (which stops the open prompt and asks again) and Sign in again
   (`--forget`, after a confirmation: removes the saved sign-in without
   reading it, and any other Cua item this build cannot read). A launch at
   login shows the same state when the window is opened; it never prompts
   on its own.
   Whatever the prompt step ends with (allowed, closed, killed, failed), a
   fresh check without a prompt decides: still not readable goes back to
   "Allow Keychain access" with "macOS didn't give access. Click Allow
   access to ask again." Try again stops the open check (SIGTERM, then
   SIGKILL after 2 s) before it asks again. Once waiting 25 s, a prompt that
   is no longer on screen (no SecurityAgent window wider than 100 pt) is
   reported: "The macOS password prompt was closed or is hidden."
3. Then "Starting Cua…": `cua daemon start` (up to 30 s, plus a retry) and
   the SDK, which hand their services to the stand-ins. After 75 s it says
   what is stuck: "Cua is waiting for Keychain access" (back to Allow
   access) or "Cua's background service didn't start", whose Try again
   restarts the daemon (stop, then kill this bundle's daemon still starting
   or not answering) and starts again.

**No other read may prompt.** The app sets `CUA_KEYCHAIN_NONINTERACTIVE=1`
for itself and the daemon it starts (cua-auth turns keychain prompts off
around each of its keychain calls then): a read that would need a prompt
fails at once as "needs access", so the in-process SDK and the daemon start
signed out instead of hanging, and the window asks. Only the clicked
`cua auth keychain --prompt` runs without it. A `cua` run from a terminal
stays interactive; one without a terminal (an agent, a script) is
non-interactive unless `CUA_KEYCHAIN_NONINTERACTIVE=0`, and a call macOS does
not answer within `CUA_KEYCHAIN_TIMEOUT_SECS` (default 5) fails as "needs
access" too.

New UI shows the same states and words through the bridge (`startup.get`,
`startup.act`, `startup.changed`; `WebHost/WebUIBridge+Startup.swift` and
`apps/cua-spaces-web/src/bridge/ops/startup.ts`). Fixture runs show each
state with `CUA_SPACES_STARTUP=needs-keychain|needs-keychain-locked|waiting|denied|starting`.

**Why other Cua builds don't ask again.** The item uses the legacy
(file-based) keychain's access list, not the data protection keychain.
Each trusted application entry records that executable's designated
requirement, and a Developer ID build's is its identifier plus the team
(`identifier "com.trycua.spaces.macos" and anchor apple generic and ...
certificate leaf[subject.OU] = YCK386LBJ7`; `cua` is `com.trycua.cua`),
which every later dev, RC or release build meets. A Cua-signed writer also
gets the partition `teamid:YCK386LBJ7`, which every Cua build has. A writer
lists itself, the other executables of its bundle (`Contents/MacOS/*`) and,
for a `cua` outside a bundle, those of `/Applications/Cua Spaces.app`.
`scripts/build-release.sh` signs every executable with the one identity and
a fixed identifier, and fails when a designated requirement is a cdhash or
lacks the identifier and team.

The data protection keychain with a `keychain-access-groups` entitlement
(`YCK386LBJ7.com.trycua.cua`) was rejected: a Developer ID app needs a
provisioning profile for that entitlement, and the standalone `cua` CLI (a
bare executable, also used over SSH) can't carry one, so the CLI and the
app could no longer share the session.

**Moving from an ad hoc build's item.** An ad hoc build's entries are its
cdhash, and an item written before the Cua access list (no comment) trusts
only its creator. Items carry a comment: `cua-acl/v3` from a Cua-signed
writer, `cua-acl/v2` from an unsigned one. A Cua-signed process recreates
(deletes and writes again, with the Cua list) every item it has just read
that is not `v3`, and every item it writes. So the first Developer ID
build to read an older item asks once, from the visible "Allow Keychain
access" state, and no Cua build asks after that. If the item cannot be
deleted it is left as is; a failed rewrite falls back to the default list,
so the sign-in is never lost.

## Which app is primary

Both ship from the same core, so neither owns behaviour. Recommendation:
make this SwiftUI app the default macOS distribution and keep the Electron
app for Windows and Linux.

- The Mac surfaces that matter most are native here: the notch panel, the
  menu bar extra, sheets, Liquid Glass, Passwords-style lists and system
  controls, with no webview.
- The Keyvault identity is simpler: one signed native binary instead of a
  webview host plus sidecars.
- Smaller and faster: no bundled web runtime or wasm build for the UI.
- The cost is a second UI to keep up; the core and the parity flows keep that
  to rendering.

This app is the macOS release; the Electron app (`apps/cua-spaces-desktop`)
ships for Windows and Linux.

## Run a Local build next to the stable app

`scripts/install-local.sh` builds this checkout and installs it as **Cua Spaces
Local**, beside the released Cua Spaces, so you can use Spaces to build Spaces:
the released app on your production account, your build on staging, both
signed in and both hosting this Mac.

```sh
scripts/install-local.sh                 # debug build, then opens the app
scripts/install-local.sh --release       # release configuration
scripts/install-local.sh --no-launch     # install without opening it
scripts/install-local.sh --with-lume-local   # also install lume-local
scripts/install-local.sh --prefix DIR    # app in DIR/Applications, cua-local in DIR/.local/bin
```

`--prefix` is resolved first (`..` and symlinks included) and refused when the
app or `cua-local` would land on, or inside, anything the released Cua Spaces
owns: `/Applications`, `~/.cua`, `~/.local/bin/cua`, `~/.lume`, `~/Cua Volume`.

The Local build shares nothing with the released app. Every name comes from
`CuaChannel::Local` (`libs/cua/crates/cua-home/src/channel.rs`):

| | Cua Spaces | Cua Spaces Local |
|---|---|---|
| App | `/Applications/Cua Spaces.app` | `~/Applications/Cua Spaces Local.app` |
| Bundle id | `com.trycua.spaces.macos` | `com.trycua.spaces.local` |
| CLI | `~/.local/bin/cua` | `~/.local/bin/cua-local` |
| Home | `~/.cua` | `~/.cua-local` |
| Backend | production | staging (`CUA_ENV=prod` opts in) |
| URL scheme | `cua://` | `cua-local://` |
| Updates | Sparkle | none |

The script builds the library, the `cua` CLI and `cua-spacesd` from this
checkout (`--skip-rust-build` reuses the last build), then runs
`scripts/build-app.sh --channel local` with `CUA_SPACESD_BIN` set to that
`cua-spacesd`. Without it build-app.sh bundles the pinned release of the host
service, which does not know the Local ports; the script compares the bundled
daemon's machine code with the one it built and refuses the app if they differ.
It copies the app to a staging folder, signs it, checks the signature, the bundle id, the URL scheme
and that there is no update feed, and only then swaps it in. If the copy does
not verify it puts the previous Local app back. It refuses to replace anything
at the Local path that is not a Cua Spaces Local bundle, and it never writes to
`/Applications/Cua Spaces.app`, `~/.local/bin/cua`, `~/.cua` or `~/.lume`.
`cua-local` is a link to the `cua` inside the app; no shell profile is edited,
so put `~/.local/bin` on your PATH.

**Permissions survive a rebuild** when the app is signed with the stable local
certificate that Cua Driver's local install uses
(`libs/cua-driver/scripts/_local-signing.sh`; setup in
`libs/cua-driver/scripts/README.md`, "Stable macOS local signing"). Without it
the app is signed ad hoc and macOS asks for Screen Recording and Accessibility
again after every rebuild; the script says so, and clears the stale rows for the
Local bundle id only. `--require-stable-signing` stops instead of signing ad hoc.

Rebuilding is the same command. To remove it:

```sh
scripts/uninstall-local.sh               # asks first; --yes to skip, --with-lume-local to include lume-local
```

It unregisters the Local helper login item and removes the app, `cua-local`,
`~/.cua-local`, the Local app support folder and the Local Keychain items
(`ai.cua.session.local*`). It keeps `~/Cua Volume Local` and the permissions
macOS granted.

There is one Local slot per Mac: an Internal build from CI uses the same
identity, so it and a build from source replace each other.

`scripts/tests/install-local-test.sh` checks all of this against a fake `HOME`
and a fake build: a Stable install is byte-identical before and after install,
rebuild and uninstall, a wrong bundle id is refused, a failed copy is rolled
back. `CUA_TEST_SIGNING_KEYCHAIN=<keychain>` adds the certificate-signing checks.

## Release

`.github/workflows/cd-cua-spaces.yml` (tags `cua-spaces-v*`) runs
`scripts/build-release.sh`: both architectures of the app export and of the
`cua` CLI, joined with lipo, the Swift app for both architectures, the
tag's version in Info.plist, the repository's update feed (`--feed-url`),
and a Developer ID signature with the hardened runtime from the inside out
(Sparkle.framework in Sparkle's documented order: `Installer.xpc`,
`Downloader.xpc`, `Autoupdate`, `Updater.app`, the framework; then
`libcua_sdk.dylib`, then `Contents/MacOS/cua` as `com.trycua.cua` with
`Support/cua.entitlements`, then the app as `com.trycua.spaces.macos` with
`Support/CuaSpacesMac.entitlements`). It ends with `scripts/check-launch.sh`:
every library resolves inside the bundle and passes library validation, and
`CuaSpacesMac --check-launch-only` starts and exits. The workflow notarizes
and staples the app, packs it with `scripts/package-dmg.sh` into
`cua-spaces-<version>-darwin-universal.dmg`, signs, notarizes and staples
that, and builds the MDM .pkg from the same app. The app needs macOS 26.

The app ships the `cua` CLI, daemon and app core built from `libs/cua` at the
release tag, so a fix that lands only in `libs/cua` (for example the sign-in
refresh in #4633) reaches app users with the next Cua Spaces release. Cut one
when such a fix should go out; the `cua-spaces` Release Please component counts
commits under this directory.

```sh
scripts/build-release.sh --version 0.2.0 --out /tmp/spaces-release   # ad hoc signed, universal
scripts/package-dmg.sh --app "/tmp/spaces-release/Cua Spaces.app" --out /tmp/cua-spaces.dmg
scripts/verify-release.sh cua-spaces-0.2.0-darwin-universal.dmg      # a downloaded release
scripts/verify-release.sh --dir assets --version 0.8.0-beta.6 \
  --expect swift,electron,windows,appcast,feed                      # every asset of a release
```

CD (`cd-cua-spaces.yml`, job `verify-release`) runs `verify-release.sh` on the
signed and notarized build before anything is published.

The disk image window is the spaces.cua.ai hero with the blue Cua Spaces key
missing. The app, whose icon is that key, sits on the left. The Applications
alias, whose icon is the key's outline with an Applications glyph, sits where
the key was, so installing is dragging the key into its place.
`package-dmg.sh` lays the window out with a pinned
[dmgbuild](https://github.com/dmgbuild/dmgbuild) (`scripts/dmg-settings.py`)
and makes the alias with `scripts/make-alias.swift`. The art in
`Support/dmg` and the app icon `Support/AppIcon.icns` (the key with the Spaces
mark, the Cua mark in stacked squares, on a slate tile) are drawn from the site's keycap sprites by
`scripts/dmg-art/render.sh` (headless Chrome), which also writes the same icon
to `apps/cua-spaces/src-tauri/icons` for the other apps; rerun it only when
the art changes.

## Updates (Sparkle)

The app updates itself with [Sparkle](https://sparkle-project.org) 2
(SwiftPM, pinned in `Package.resolved`; Settings, About drives it through
the core's `about` module). Info.plist names the feed and the EdDSA public
key: `SUFeedURL` is
`https://github.com/trycua/cua/releases/download/cua-spaces-latest/cua-spaces-appcast.xml`,
the rolling `cua-spaces-latest` release that already carries the Spaces
updater feed. GitHub's repository-wide "latest" release belongs to whichever
component released last, so a `/releases/latest/download/` URL would not
always be the Spaces feed; a fixed tag always is. The app is outside the App
Sandbox, so it needs no `SUEnableInstallerLauncherService`.
`SUVerifyUpdateBeforeExtraction` checks the signature before anything is
unpacked (Sparkle does this whatever the user driver).

Sparkle shows no window of its own. `SparkleUpdater` runs an `SPUUpdater`
with a custom user driver (`UpdateUserDriver`, over the plain-value
`UpdateFlow` state machine in `UpdateDriver.swift`). It checks at launch and
periodically (`SUEnableAutomaticChecks` and `SUAutomaticallyUpdate` are on, so
updates check and download in the background out of the box, as on Windows
and Linux; an explicit off is kept). An
update found by a scheduled check downloads and extracts at once; Sparkle's
ready-to-install reply is then held, and nothing restarts the app until the
page calls `update.install` ("Restart to update"). With "check for and
download updates automatically" on (the default), Sparkle's silent path reaches the same state through
`updater(_:willInstallUpdateOnQuit:immediateInstallationBlock:)`. The page
reads the state with `update.get` (`{status, currentVersion, version,
percent, notesUrl, message}`; status is `idle`, `checking`, `available`,
`downloading`, `downloaded` or `error`) and refetches on the `update.changed`
event. Check Now (Settings, About) is quiet too: nothing found ends silently,
and only an error from a check the user started shows a small "Update Error!"
alert; background errors are logged. Usage telemetry records `found`,
`not_found`, `downloaded`, `installed` and `failed`.

Channels: an `X.Y.Z-suffix` release is a `beta` item, which only apps set to
"Update to: Beta" (`AppSettings.updateChannel`) are offered.

After an update, the first launch (the settings file's `lastSeenVersion`
changed) runs the bundled `cua agents update` (skill folders and MCP entries
cua installed; folders the user edited are skipped) and stops and starts the
running daemon when its executable is inside this bundle, with one notice
only if either failed.

### Release versions

Release Please drives the `cua-spaces` component from this directory: a
releasable conventional commit (`fix:`, `feat:`) that touches
`apps/cua-spaces-macos` opens the `cua-spaces` release PR, which bumps
`VERSION`, `CHANGELOG.md` and `CFBundleShortVersionString` in
`Support/Info.plist`, plus the Tauri app's versions in `apps/cua-spaces` (it
shares the version but its commits never open a release). Release Please
splits commits by package path and skips a path with no commits (even for a
targeted bump), so a change only under `libs/spaces-app-swift` does not open
one on its own: land it with the app change that uses it.

The same holds for the `cua` the app bundles (built from `libs/cua`): a
cua-sdk release is a chore commit under `libs/cua`, so it opens no app
release. `BUNDLED_CUA_VERSION` records the cua the app bundles, and the
`Release: Bundle the new cua in Cua Spaces` workflow opens a
`fix(spaces-macos)` PR that updates it whenever `libs/cua/VERSION` changes
on main. Merging that PR makes Release Please open the next `cua-spaces`
release, so the app never lags the machines running the new cua.

### Publishing the appcast (runbook)

1. The key: the `CUA_SPACES_SPARKLE_ED_PRIVATE_KEY` secret (base64 Ed25519
   seed, Sparkle's key format) on the repository that builds the release. Its
   public half is `SUPublicEDKey` in `Support/Info.plist`. Without the secret
   the release workflow makes no appcast and changes nothing else.
2. Each `cua-spaces-v*` release with a Developer ID build: the macOS job runs
   `scripts/make-appcast.sh` (Sparkle's `generate_appcast` signs the
   notarized DMG and merges its item into the published appcast;
   `scripts/verify-appcast.sh` checks it against `SUPublicEDKey`) and keeps it
   as the `sparkle-appcast` artifact. After the release is published, the last
   job uploads it to `cua-spaces-latest` as `cua-spaces-appcast.xml`. Release
   notes are the release's body, else the version's `CHANGELOG.md` section.
3. `cua-spaces-latest` must exist once (the Actions token cannot create a new
   tag): `git push origin <any commit>:refs/tags/cua-spaces-latest`.
4. Check it: `curl -L <SUFeedURL>` lists the new item; CI's "Sparkle appcast
   dry run" (`scripts/appcast-dry-run.sh`) proves the secret matches the
   public key on every PR that has it.
5. A bad release: delete its `<item>` from the appcast and re-upload (never
   reuse a bundle version; `CFBundleVersion` carries the run number).
6. Rotating the key: ship a release signed by the old key whose Info.plist
   has the new `SUPublicEDKey` first, then switch the secret.
7. The move to the Electron app (off): for one release,
   `make-appcast.sh --electron` makes the item the Electron app's disk image
   (same bundle id, team and key; the workflow does it when the
   `CUA_SPACES_SPARKLE_ELECTRON_VERSION` variable names that version). See
   `apps/cua-spaces-desktop/docs/sparkle-cutover.md`.

An ad hoc build has the release layout and flags but no team, so it is never
first party to the Keyvault and cannot be notarized.

## License

Source-available under FSL-1.1-MIT ([LICENSE](LICENSE)). Offering it as a hosted or managed service needs a commercial licence: see [COMMERCIAL.md](../../legal/COMMERCIAL.md).
