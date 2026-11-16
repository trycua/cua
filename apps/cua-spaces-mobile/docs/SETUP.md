# Setting up Cua Spaces for iPhone and iPad

These are the steps that need an account holder: accounts, keys, certificates and production settings. Nothing in the repository creates or stores them. Each item says what to do and what it unblocks. The first two unblock everything else.

## 1. A public OAuth client for the phone (about 10 minutes)

**Why:** `cua-cli` only allows a loopback redirect, so the phone needs its own client. Until it exists, "Sign in with cua.ai" is refused by the provider and the app says so.

**Steps** (Keycloak admin console, realm `cyclops-cs` on auth.cua.ai; the pre-production environment has its own realm, do it there first):

1. Clients, Create client. Client type `OpenID Connect`. Client ID `cua-spaces-mobile` (or any id, then build the app with `EXPO_PUBLIC_CUA_OIDC_CLIENT_ID=<id>`).
2. Client authentication **off** (a public client). Authentication flow: **Standard flow** only (authorization code). Turn off Direct access grants and Implicit.
3. Valid redirect URIs: exactly `ai.cua.spaces:/oauth/callback`. Web origins: none.
4. Advanced, Proof Key for Code Exchange Code Challenge Method: **S256** (required).
5. Client scopes: add `offline_access` and `organization:*` (the scopes the app asks for: `openid profile offline_access organization:*`). Refresh tokens must be allowed (offline access).
6. Save. Nothing else changes.

**Unblocks:** any real sign-in on a device or Simulator, device enrollment, and every live path in the app.

## 2. Apple Developer account, App ID and TestFlight (about 1 hour, plus Apple's review waits)

**Why:** the app is built for the Simulator only. A phone needs a signed build; testers need TestFlight.

**Steps:**

1. Enroll the company in the Apple Developer Program (Organization, needs a D-U-N-S number) at developer.apple.com/programs. Accept the agreements in App Store Connect.
2. Certificates, Identifiers & Profiles, Identifiers: create an App ID with bundle id **`ai.cua.spaces.mobile`** (explicit). Enable the capability **Push Notifications**. No other capability is needed (microphone and speech recognition are permission prompts, not entitlements).
3. Create an Apple Distribution certificate and an App Store provisioning profile for that App ID (or let Xcode manage signing once the team is added to the project).
4. App Store Connect, My Apps, New App: name "Cua Spaces", bundle id `ai.cua.spaces.mobile`, SKU any, primary language English.
5. App privacy: the app collects account identifiers and message content that go to the user's own computers through the relay; it does not track. The microphone and speech purpose strings are already in the app (`NSMicrophoneUsageDescription`, `NSSpeechRecognitionUsageDescription`): dictation is the phone's own recognizer, no audio goes to Cua.
6. Build and upload: `pnpm core:ios` (device slice), `npx expo prebuild --platform ios`, open `ios/CuaSpaces.xcworkspace`, set the team, Product, Archive, Distribute App, App Store Connect. (Or send the exported `.ipa` through Transporter.)
7. TestFlight: add internal testers (up to 100, no review), or an external group (needs Beta App Review).

**Unblocks:** running on a real iPhone, TestFlight, and push to a real device (the Simulator cannot receive a real APNs push).

## 3. An APNs key for push to a closed app (about 15 minutes, after item 2 step 2)

**Why:** the relay needs a key to send the alert; the phone side (registration, tap routing, Settings) is built and tested against a mock provider.

**Steps:**

1. developer.apple.com, Certificates, Identifiers & Profiles, Keys, Create a key. Name "Cua Spaces push", enable **Apple Push Notifications service (APNs)**, Continue, Register.
2. Download the `.p8` **once** (Apple will not show it again) and note the **Key ID** (10 characters) and your **Team ID** (Membership details, 10 characters).
3. Put the three values on the relay as secrets (never in git): `CUA_PUSH_APNS_KEY_FILE` (path of `AuthKey_<id>.p8` in the relay's secret mount), `CUA_PUSH_APNS_KEY_ID`, `CUA_PUSH_APNS_TEAM_ID`; `CUA_PUSH_APNS_TOPIC` is the bundle id (default `ai.cua.spaces.mobile`).
4. Start the relay with push on: `CUA_RELAY_PUSH=on` (off by default; a relay without it answers 404 on `/v1/push` and the app says push is not set up).

**Unblocks:** "Needs you" and "Done" alerts on a closed app, with Allow and Decline on the lock screen.

## 4. Deploy the relay with the phone's support (a production change)

**Why:** the relay code for phones (device enrollment, push routes, viewer tickets for `/viewer` paths, so the live desktop works in the phone's web view) is in the daemon and relay work that ships with this app; it must be deployed before the phone can use these features against production. This is a production infrastructure change, so it needs a decision by whoever runs the relay.

**Steps:** after that work merges, deploy the relay as you normally do (the relay release), with `CUA_RELAY_PUSH=on` once item 3 is done. Roll out to the pre-production environment first; check `GET /v1/push` answers `"configured": true` with a phone's account token.

**Unblocks:** push registration for real phones, the live desktop through the relay, and (with items 1 and 2) the first real end to end run.

## 5. Not account work, listed so nothing is missed

- Android: needs an Android SDK and `cargo-ndk` on a build computer and a Play Console account. Not part of this app yet.
- The Cua Spaces macOS icon is used as is; a full-bleed iOS icon is a design asset.
- App Store screenshots and the store listing text are marketing assets (real Simulator screenshots of the sample build can seed them; they are labelled sample data).

## 6. What the relay and daemon need

Everything below is a production change or an Apple-side action; nothing in the repository does it.

**Relay deploy .** Two additive changes in `cua-relay`:

1. Push routes `/v1/push/...`: off unless `CUA_RELAY_PUSH=on`; a relay deployed without it behaves as before (404).
2. Viewer support for a phone's web view: `GET /m/<id>/viewer...` is forwarded without a credential (static page), and a computer-minted viewer ticket in `x-cua-env-authorization: Bearer v1....` is forwarded instead of stripped (revocation checked at the relay, signature and scope at the computer). This changes what an unauthenticated request may reach (only the viewer's static page) and what a ticket may do (also on header calls). Review `libs/cua-spacesd/crates/cua-relay/src/server.rs` (`viewer_page`, `has_forwardable_credential`) and `grants.rs` (`header_ticket`) before deploying; tests are `tests/viewer_ticket.rs` and `tests/push.rs`. Deploy to the pre-production environment first.

**APNs key, exactly what is needed** (after the Apple Developer account and the App ID `ai.cua.spaces.mobile` with Push Notifications, section 2 and 3 above):

- the `.p8` file, mounted on the relay as a secret (not in the image, not in git): `CUA_PUSH_APNS_KEY_FILE=/path/AuthKey_XXXXXXXXXX.p8`
- `CUA_PUSH_APNS_KEY_ID` (10 characters), `CUA_PUSH_APNS_TEAM_ID` (10 characters), `CUA_PUSH_APNS_TOPIC` (the bundle id; default `ai.cua.spaces.mobile`)
- `CUA_RELAY_PUSH=on`, and a persistent volume path for `CUA_RELAY_PUSH_FILE` (default: next to the state file; it holds device tokens, 0600)
- The relay reads the key at start and holds it in memory; a named but unreadable key, or one missing its ids, stops the relay at start instead of running half set up. A phone registered with `env: sandbox` goes to Apple's sandbox endpoint (development builds), `production` to the production one.
- Check: `GET /v1/push` with a phone's token says `"provider": "apns", "configured": true`.

**The daemon side needs no setup:** each computer's daemon reports events to the relay with the account it is signed in as.

## 7. Export compliance (TestFlight and the App Store)

`app.json` sets `ITSAppUsesNonExemptEncryption` to `false`, so App Store Connect does not ask the export compliance questions for each new build. The answers behind it (the owner's, given once in App Store Connect): the app uses standard encryption only (HTTPS through the platform, and AES-256-GCM, a standard algorithm, from a standard library, to seal the on-disk store in the Rust core, `core/src/store.rs`), and the France-specific question is answered no. If the app ever adds encryption of its own design, or a non-standard algorithm, change the key back (remove it) and answer the questions again for that build, and update this section.

## 8. Uploading a build

`scripts/testflight.sh` builds the core for a device, archives, signs and uploads. It reads credentials from the environment (`ASC_KEY_PATH`, `ASC_KEY_ID`, `ASC_ISSUER_ID`) or uses the Xcode account that is signed in; nothing is read from the repository, and the `.gitignore` keeps `*.p8`, `*.p12` and `*.mobileprovision` out. `scripts/testflight.sh --dry-run` makes an unsigned archive to prove the build.
