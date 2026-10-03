# Spaces keychain migration investigation

Status: draft investigation; production fix not implemented.

## Report

Spaces setup displayed two keychain approval dialogs, both naming `run.cua.ai`.
The affected items' access lists have not been captured, so the precise cause
of that user's dialogs remains unverified. The source and isolated experiment
below establish a migration gap consistent with the report.

## Source findings

- `cua-auth/src/lib.rs` stores the sign-in session under service `run.cua.ai`,
  account `cua-cli`, and named secrets under accounts `cua-cli.<name>`.
- `cua-host/src/device.rs` stores the device identity as `device-key`.
  `key_or_create` returns an existing key without saving it again.
- `cua-auth/src/macos_keychain.rs` creates items with explicit trust for the
  creating executable and its companion executables. The marker is
  `cua-acl/v2`. Legacy items are migrated only by `set_generic_password`.
- Reading an existing device key does not migrate its access list. Token
  refresh can migrate the session while leaving the device key unchanged.
- The installed Spaces 0.6.0 inspected during investigation contains the
  marker in its bundled CLI. The app and CLI have distinct designated
  identifiers (`com.trycua.spaces.macos` and `com.trycua.cua`), both signed by
  team `YCK386LBJ7`. Neither real credential was present in the inspecting
  session's keychain; no real secrets were read.

## Native API experiment

A disposable explicit keychain held dummy data under two accounts. Three
separately ad-hoc-signed executables represented the creator, companion reader,
and unrelated reader. User interaction was disabled with
`SecKeychainSetUserInteractionAllowed(false)`.

Items were created through `SecKeychainItemCreateFromContent`, first with its
initial access argument NULL, then with a `SecAccessCreate` list naming both
creator and companion. Each reader used `SecKeychainFindGenericPassword` to
request the secret bytes. Migration recreated one legacy fixture at a time
with the explicit access list and unchanged dummy bytes.

| Fixture state | Companion: session | Companion: device key | Unrelated reader |
| --- | --- | --- | --- |
| Default creator-only access | `errSecAuthFailed` (-25293) | `errSecAuthFailed` (-25293) | Both denied |
| Only session migrated | Success | `errSecAuthFailed` (-25293) | Both denied |
| Both items migrated | Success | Success | Both denied |
| Explicit shared access from creation | Success | Success | Both denied |

The unrelated executable was denied for every item at every stage, including
all shared-access items. The throwaway keychain was deleted after each run.
No real credential or login keychain setting was changed.

This validates the native access-control mechanism, not production onboarding:
ad-hoc helpers are not Developer ID signed Spaces, and disabling interaction
means the experiment does not establish an exact dialog count.

## Proposed fix scope

Keep separate token and device-key items. Retain the existing explicit
companion trust policy rather than granting all applications access or merging
secrets with different lifecycles.

Migrate legacy permissions for both items at a deliberate authenticated setup
boundary, including existing device keys that will not otherwise be rewritten.
Do not indiscriminately turn background credential reads into prompting writes.

The existing write helper deletes before recreating a legacy item. Do not
simply invoke it on every successful read: failure after deletion could lose a
persistent device identity. Choose and test a migration strategy that preserves
the old item on failure, including whether an in-place access-list update and
its one-time macOS authorization is preferable to replacement. Do not promise
zero or one upgrade dialog before testing the signed application.

## Acceptance criteria before ready for review

- [ ] Implement explicit, idempotent migration for both legacy items.
- [ ] Preserve the device's exact key material and enrollment identity.
- [ ] Preserve usable credentials if migration is denied or fails; no deletion
      followed by unrecoverable creation failure.
- [ ] Coordinate concurrent app/daemon access and prevent migration races.
- [ ] Keep unrelated applications outside the trusted executable list.
- [ ] Cover legacy, partially migrated, and current-marker items.
- [ ] Verify signed-app fresh setup and upgrade from creator-only items using
      an isolated macOS user or VM, recording process, item, and dialog counts.
- [ ] Verify cancellation, locked keychain, and repeated launch behavior.
- [ ] Keep ordinary tests hermetic and live keychain tests explicitly opt-in.

## Worklog

1. Inspected authentication storage, companion trust construction, and device
   identity reuse; confirmed write-only migration on current main.
2. Checked installed app version and signing identities without reading secrets.
3. Ran the isolated native API experiment with an unrelated-reader control.
4. Opened an investigation draft; production implementation and signed-app
   acceptance evidence remain outstanding.
