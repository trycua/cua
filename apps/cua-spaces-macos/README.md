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
| Onboarding | Paged setup with the stacked Cua mark and page dots; installs the bundled `cua` and sets up host access through the shared core |
| Devices | Settings → Devices: this Mac's enrollment (enrolled until, grace ends, needs enrollment), the account's devices with Rename and Revoke, Recent Access; the Enroll sheet (sign in again, or a one-time code approved from an enrolled device), the approval sheet for a device asking to join (a notification too; Approve asks for Touch ID or the login password), and a banner in the main window when this Mac needs enrolling |
| Menu bar | The Cua mark (template); status line, Open, New Space, Settings, Quit |
| Launch at login | Settings → General: `SMAppService.mainApp`, showing what macOS reports (on, off, waiting for approval with a button to Login Items, not found); the first run's Done page ticks it. A login launch opens without the main window, and the app's daemon (host Spaces, persistent agents, Cua Volume) starts with it |
| Notch panel | The "N Spaces" tab, Space tiles with live thumbnails, and the Teleport prompt; hover opens it after 300 ms, a dragged window or file drops on a tile. Window drags need Accessibility; the panel says so with an Open Settings button |

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
`CUA_SPACES_FIXTURES=1` runs on fixtures (no daemon, no Space, an in-memory login item whose status `CUA_SPACES_LOGIN_ITEM` sets: `enabled`, `notRegistered`, `requiresApproval`, `notFound`);
`CUA_SPACES_START_VIEW` opens `new-space`, `keyvault`, `approval`, `settings-devices`, `settings-about`, `device-approval` (with `CUA_SPACES_APPROVE_CODE`), `device-enroll`, `device-enroll-code`,
`onboarding`, `stream` (the Space detail at its Stream section), `window-pip` (the same, with the first window popped out), `teleport-picker` (the Space's teleport picker; set `CUA_TELEPORT_APP_ROOTS` to fixture apps for images) or a notch state (`notch-open`, which springs
open 1.5 s after launch, `notch-hover`, `notch-permission`,
`notch-drag-target`, `notch-drop`); `CUA_SPACES_SELECT`
selects a Space; `CUA_SPACES_CREATE=<image>[,<name>]` creates one on this
Mac through the real create path (end-to-end runs); `CUA_SPACES_NOTCH_HIGHLIGHT` (`tile`, `list`, `settings`,
`search` or `tab`, optionally `:pressed`) forces a notch control's hover or
pressed look; `CUA_SPACES_UPDATER=live` runs Sparkle in a fixtures run (the feed in
Info.plist; use a test bundle identifier and a local feed), `CUA_SPACES_UPDATE_CHECK=background`
checks at launch, and `CUA_SPACES_REFRESH=1` runs the after-update refresh in a fixtures
run (with a throwaway `HOME`). State lives under `$HOME/Library/Application Support/com.trycua.spaces.macos`.
`CuaSpacesMac --check-launch-only` loads the app's libraries, prints its version and exits
(`scripts/check-launch.sh`; release builds too).

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

## Which app is primary

Both ship from the same core, so neither owns behaviour. Recommendation:
make this SwiftUI app the default macOS distribution and keep the Tauri app
for Windows and Linux.

- The Mac surfaces that matter most are native here: the notch panel, the
  menu bar extra, sheets, Liquid Glass, Passwords-style lists and system
  controls, with no webview.
- The Keyvault identity is simpler: one signed native binary instead of a
  webview host plus sidecars.
- Smaller and faster: no bundled web runtime or wasm build for the UI.
- The cost is a second UI to keep up; the core and the parity flows keep that
  to rendering.

This app is the macOS release; the Tauri app ships for Windows and Linux.

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

```sh
scripts/build-release.sh --version 0.2.0 --out /tmp/spaces-release   # ad hoc signed, universal
scripts/package-dmg.sh --app "/tmp/spaces-release/Cua Spaces.app" --out /tmp/cua-spaces.dmg
scripts/verify-release.sh cua-spaces-0.2.0-darwin-universal.dmg      # a downloaded release
```

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
unpacked.

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

An ad hoc build has the release layout and flags but no team, so it is never
first party to the Keyvault and cannot be notarized.

## License

Source-available under FSL-1.1-MIT ([LICENSE](LICENSE)). Offering it as a hosted or managed service needs a commercial licence: see [COMMERCIAL.md](../../COMMERCIAL.md).
