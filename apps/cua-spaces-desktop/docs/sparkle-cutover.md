# Sparkle cutover: from the Swift app to the Electron app on macOS

Installed Swift apps (apps/cua-spaces-macos) update with Sparkle. The
cutover is one stable release whose Sparkle appcast item is the Electron
app instead of the Swift app. Sparkle installs it in place of the Swift
app. From then on the Electron app updates itself with electron-updater.
Nothing below is switched on today.

## Why Sparkle accepts the Electron app

Checked against Sparkle 2.10.0, the version Package.swift pins
(`SUUpdateValidator.m`, `SUCodeSigningVerifier.m`, `SUInstaller.m`,
`SUPlainInstaller.m`). `scripts/sparkle/accepts.sh` compiles those files
and runs the same checks on a real app and disk image.

| Sparkle check | What the Electron build does |
|---|---|
| The archive's EdDSA signature must verify with the **old** app's `SUPublicEDKey`. The Swift app sets `SUVerifyUpdateBeforeExtraction`, so this runs before extraction. A Developer ID team match is the fallback only if EdDSA fails. | `make-appcast.sh --electron` signs the Electron DMG with the existing Sparkle key. |
| A new bundle without an EdDSA key is refused ("Sparkle only supports rotation, not removal"). | `SUPublicEDKey` is copied from the Swift Info.plist (`mac.extendInfo`). Electron does not use it. |
| The new bundle must be code signed if the old one is, and its signature must be valid. Sparkle runs `SecStaticCodeCheckValidity` on all architectures, without the nested-code check. | electron-builder signs deep with Developer ID and the hardened runtime, then notarizes and staples. |
| Without pre-extraction verification, EdDSA (old key) **or** the old app's designated requirement must pass. | Both pass: same key, and same identifier plus same team. |
| The installer finds the new app by the old app's file name, else by bundle id. | `Cua Spaces.app`, `com.trycua.spaces.macos`. |
| No downgrade: the new `CFBundleVersion` must be ≥ the old one (`SUStandardVersionComparator`). | `X.Y.Z.<run number>`, the Swift app's scheme, from the same workflow. |
| The new app does not need to contain Sparkle. The old app's Autoupdate swaps the bundle and relaunches it by path. | Nothing needed. |
| On macOS 13 and later, an atomic swap needs Autoupdate's team to equal the new bundle's team. | Both are the same Developer ID team. |
| DMG or zip archives. | DMG, like every Swift item. |
| `sparkle:minimumSystemVersion` comes from the new app's `LSMinimumSystemVersion`. | Electron's minimum is lower than the Swift app's (macOS 26), so every Swift user qualifies. |

**Verdict: Sparkle accepts the Electron app** as the update when all of
these hold:

- Same bundle id and file name.
- Same Developer ID team, notarized.
- `SUPublicEDKey` present.
- The DMG is EdDSA-signed with the existing key.
- `CFBundleVersion` is higher than the installed one.

`verify-appcast.sh --electron` checks the bundle id, the `X.Y.Z.N` form,
the key and the signature.

Some things carry over because the bundle id and team are the same:

- TCC grants are keyed by bundle id and designated requirement:
  Accessibility, Screen Recording, Apple Events (both apps hold
  `automation.apple-events` and the usage string), and Local Network (same
  usage string).
- The Keyvault trusts team plus signing identifier, and
  `com.trycua.spaces.macos` is on its first-party list.

Other state needs no work or is left as it is:

- Electron's own state lives under `~/Library/Application Support/Cua Spaces`,
  which is unaffected.
- The defaults domain `com.trycua.spaces.macos` becomes shared. Electron
  keeps its state in files, and nothing it writes collides with the Swift
  keys.
- `src/migrate-swift.ts` copies the Swift app's settings on first launch.

### What was proven locally, and what needs CI signing

`scripts/sparkle/selftest.sh "dist/mac-arm64/Cua Spaces.app"` builds a
stand-in old app with the Swift Info.plist and a throwaway key, ad hoc
signed. It then shows that Sparkle's own code accepts the real Electron
build in both validation paths, and finds and installs it without a
downgrade. It also shows that Sparkle refuses an Electron build without
`SUPublicEDKey`, a wrong signature, and a downgrade.

Only a Developer ID build can show three more things:

- the team match;
- Gatekeeper on the swapped app;
- that TCC grants survive.

Those are the rehearsal below.

## Preconditions (code, in the release commit)

1. The Electron app is at parity on macOS and has shipped as betas. The
   beta feed has worked end to end: a beta updated to the next beta.
2. `STABLE_FEED = true` in `src/updater.ts`. The workflow refuses the
   cutover otherwise. `test/release.test.ts` ("serves only the beta channel
   until the cutover") asserts the old value; it changes in the same commit,
   or electron-mac's tests fail before the build.
3. Migrated users update on their own. The updater counts as enabled when
   `swiftMigration.found` is true and `sparkleAutomaticChecks !== false`
   (Sparkle's default was on). Everyone else stays opt-in.
4. electron-builder sets `generateUpdatesFilesForAllChannels: true` from the
   cutover on. Stable releases then also write `beta*.yml`, so betas move
   on to the stable release.
5. The install scripts (`scripts/install`) and spaces.cua.ai point at the
   Electron DMG, as a separate change.
6. The release's `cua` replaces a daemon whose executable is gone or was
   replaced (`cua-daemon` `identity::verdict`). The daemon the Swift app
   started keeps running across Sparkle's swap; its file
   (`Contents/MacOS/cua`) is gone afterwards, and while the Electron app
   used it the Keyvault refused ("the peer process no longer matches its
   connect-time audit token"). Fixed in #4942 (cua-spaces 0.8.0-beta.3).

## Rehearsal (a staging repository)

Run it first on a copy of the repository (`<owner>/<repo>`), whose
releases feed only test Macs.

1. Install the Swift app from a staging release on a test Mac.
   Grant Accessibility, Screen Recording and Apple Events, change a few
   settings, and finish onboarding.
2. On the staging repository, set the repository variable
   `CUA_SPACES_SPARKLE_ELECTRON_VERSION` to the next version. Push its tag.
3. Check the run:
   - electron-mac's "Sparkle appcast (cutover…)" step passes
     `verify-appcast.sh --electron`.
   - build-macos skipped its appcast step.
   - `latest*.yml` is on `cua-spaces-latest`.
4. On the test Mac, choose Check for Updates in the Swift app. Then confirm:
   - The Electron app replaces it in `/Applications` and relaunches.
   - The settings, onboarding and window frame came across.
   - The permissions still work without new prompts.
   - The Keyvault opens.
5. On a second Mac, before updating, run
   `scripts/sparkle/accepts.sh "/Applications/Cua Spaces.app" <dmg> <edSignature from the appcast>`.
6. Release the next version without the variable. The Electron app updates
   itself from `latest-mac.yml`.

## Rehearsal without a staging repository

The staging repository has no signing secrets, so the same check can run
from trycua/cua without publishing anything:

1. On a throwaway branch with `STABLE_FEED = true` (and the test above),
   run "CD: Cua Spaces" by hand with `electron_dry_run` and
   `sparkle_electron` and a version whose `X.Y.Z.<run number>` is above the
   installed Swift app's. electron-mac signs, notarizes and staples the app
   and keeps the cutover appcast (`sparkle-appcast`, signed with the release
   key and checked by `verify-appcast.sh --electron`) and the DMG
   (`electron-darwin`) as workflow artifacts. Nothing is published: a dry
   run never uploads, and only a tag publishes.
2. On a test Mac running a signed Swift build on the Beta channel (or a
   stable version, for a stable item), point the item's enclosure URL at a
   server on the loopback interface (the EdDSA signature covers the file,
   not its URL), serve the appcast and the DMG there, and set the feed for
   this Mac only:
   `defaults write com.trycua.spaces.macos SUFeedURL http://127.0.0.1:<port>/cua-spaces-appcast.xml`
   (Sparkle reads the defaults before Info.plist).
3. Choose Check Now in the Swift app and install. Check what step 4 above
   lists. A semver below the published betas also shows the takeover: the
   Electron app's Check Now then updates from the real `beta*.yml`.
4. `defaults delete com.trycua.spaces.macos SUFeedURL` and stop the server.

## The one release (trycua/cua)

1. Before merging the Release Please PR for version X.Y.Z, set the
   repository variable `CUA_SPACES_SPARKLE_ELECTRON_VERSION=X.Y.Z`.
2. Merge. The tag push builds both apps. The Sparkle item is
   `Cua-Spaces-X.Y.Z-universal.dmg`, and the feed gets `latest*.yml`.
   - Alternative: run the workflow by hand from the tag with
     `version=X.Y.Z` and `sparkle_electron=true`. Only do this if the tag
     push did not publish the Swift item.
3. Delete the variable right after. Later releases put the Swift app back
   in the appcast unless the Swift app is retired.

## Verification

- The appcast's X.Y.Z item names the Electron DMG. Its `sparkle:version`
  is `X.Y.Z.<run>`. `sparkle:edSignature` passes `verify-appcast.sh --electron`.
- `accepts.sh` against an installed Swift app passes.
- One real update on a test Mac (rehearsal step 4), then watch the crash
  and telemetry reports for `spaces_app` launches from the new version.

## Rollback

- Before Swift apps have updated: put back the previous appcast. Upload the
  last `cua-spaces-appcast.xml` without the X.Y.Z item to
  `cua-spaces-latest`. Swift apps that have not updated stay on the Swift
  app.
- After: Sparkle is gone from migrated Macs, and Sparkle never downgrades.
  Users can reinstall the Swift DMG by hand. Their Swift settings are
  untouched, because the migration only copies them. Or ship a fix forward
  through the Electron stable feed.
