# Try the Computer History desktop preview

This guide shows you how to enable Cua Driver's encrypted Computer History,
inspect it, let an authorized agent read it, and remove it. It applies to
nightly macOS, Windows, and Linux builds that include the experimental preview.

Computer History is off by default. The preview records metadata for actions
performed through Cua Driver. It does not watch unrelated desktop activity.

## Before you start

- Use macOS, Windows, or Linux in an interactive desktop session.
- Install a Cua Driver nightly that includes Computer History.
- Grant Cua Driver the usual permissions required for the actions you want it
  to perform. Computer History does not add permission to perform an action.
- On Linux, run a desktop Secret Service implementation such as GNOME
  Keyring. The preview fails closed if no unlocked Secret Service is available.

## Install or switch to the nightly channel

For a fresh macOS or Linux installation, run:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)" -- --channel nightly
```

For a fresh Windows installation, run in PowerShell:

```powershell
& ([scriptblock]::Create((irm https://cua.ai/driver/install.ps1))) -Channel nightly
```

To switch an existing installation, run:

```bash
cua-driver channel set nightly
cua-driver update --apply
```

Confirm the selected and installed channels:

```bash
cua-driver channel status
```

## Enable Computer History

Run:

```bash
cua-driver history enable
```

The command admits the experimental feature, initializes its device-local
native credential, verifies an encrypted write and read, and then enables
capture. If the installed daemon must restart, Cua Driver preserves its
existing permission mode, capability manifest, approval flags, compatibility
mode, and launch grants.

Check the result:

```bash
cua-driver history status
```

The default retention period is 7 days and the default encrypted-store quota
is 100 MiB. Queries never return events older than 7 days. The writer seals a
live chunk at least hourly and prunes sealed chunks while it is running. If
history is disabled or the daemon is offline, expired ciphertext can remain on
disk until the next enable or query checkpoint, although it is never returned
after the query-visible cutoff.

## Inspect history locally

List recent events:

```bash
cua-driver history list 50
```

Show one event by sequence number:

```bash
cua-driver history show 42
```

Use `--json` with `status`, `list`, or `show` when another local program needs
structured output.

## Let an agent hydrate from history

Connect the agent through the normal Cua Driver tool path. The preview exposes
two read-only tools:

- `history_status`, which requires `history.status`;
- `history_query`, which requires `history.query`.

The existing Cua permission mode, policy ceiling, and capability manifest make
the decision for each call. A permitted query returns at most 200 metadata
events. The result can enter the agent's model context, and the encrypted store
records a fixed access-audit event.

The agent cannot enable, pause, resume, disable, delete, export, or obtain the
encryption key through these tools.

## Pause, resume, or disable capture

Pause capture without clearing the enabled preference:

```bash
cua-driver history pause
```

Resume it:

```bash
cua-driver history resume
```

Disable capture and stop the writer:

```bash
cua-driver history disable
```

These commands preserve existing encrypted history.

## Delete Computer History

Delete the encrypted store and destroy its exact namespace native key:

```bash
cua-driver history delete --yes
```

This is cryptographic deletion. Cua Driver does not claim physical erasure from
APFS snapshots, backups, copied ciphertext, SSD wear leveling, or memory that a
process already decrypted.

To purge history while uninstalling on macOS or Linux, run:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)" -- --purge
```

On Windows, run:

```powershell
$env:CUA_DRIVER_RS_UNINSTALL_FORCE = '1'
$env:CUA_DRIVER_RS_UNINSTALL_PURGE = '1'
irm https://cua.ai/driver/uninstall.ps1 | iex
```

Run the uninstaller as the interactive login user. Do not prefix the Unix
uninstaller with `sudo`: native credential and history cleanup must remain in
that user's context.

## Return to the stable channel

Disable capture before switching binaries:

```bash
cua-driver history disable
cua-driver channel set stable
cua-driver update --apply
```

Switching channels does not delete encrypted history. A stable build that does
not understand the preview store must leave it untouched.

## What the preview stores

The encrypted event allowlist contains:

- time and monotonic sequence;
- opaque session and action identifiers;
- a fixed Cua capability;
- an optional fixed-field platform application identifier and display name;
- fixed action outcome, delivery, route, evidence, and escalation categories;
- lifecycle, access-audit, and writer-health events.

The preview never stores screenshots, video, audio, typed text, raw keystrokes,
clipboard contents, raw tool arguments or results, accessibility trees, file
paths, window titles, URLs, or free-form diagnostics.

Files use the Cua History Profile: a CBOR Sequence of COSE_Encrypt0 records with
CloudEvents JSON inside each encrypted payload. ChaCha20-Poly1305 authenticates
each record. A namespace root key protected by macOS Keychain, Windows
Credential Manager, or Linux Secret Service and per-chunk HKDF keys protect
data at rest. There is no plaintext fallback and history performs no network
I/O.

The filesystem can still reveal that a history directory exists, its total
size, and file modification times. The native key is bound to the current
user's credential store, so copying only the encrypted files to another
machine does not provide recovery.

Default encrypted-store locations are:

- macOS: `~/Library/Application Support/cua-driver/computer-history`;
- Windows: `%LOCALAPPDATA%\cua-driver\computer-history`;
- Linux: `$XDG_STATE_HOME/cua-driver/computer-history`, or
  `~/.local/state/cua-driver/computer-history` when `XDG_STATE_HOME` is unset.

Local-development installs use `cua-driver-local` instead of `cua-driver` in
both filesystem and native-key namespaces.

## Troubleshooting

**`history_preview_not_admitted`**

The running local-development daemon was not started with the preview flag.
Restart that daemon with `cua-driver serve --experimental-history`, then run
`cua-driver history enable` again. Installed nightly builds handle the verified
relaunch automatically.

**`history_key_locked` or `history_key_unavailable`**

Unlock the login session and its native credential store (Keychain, Credential
Manager, or Secret Service), then retry. On Linux, also confirm that a Secret
Service implementation is running. Capture stays disabled and Cua Driver does
not create plaintext files.

**`history_quota_reached`**

Delete history if you no longer need it. The preview does not allow unbounded
growth and does not block the computer action that encountered the full store.

**`history_storage_corrupt`**

The reader found an invalid, incomplete, reordered, or unauthenticated record.
It refuses the affected store instead of repairing or skipping evidence. If
you accept permanent data loss, `cua-driver history delete --yes` destroys the
namespace key and removes the corrupt encrypted store so capture can start
fresh.

## Architecture and format

See [Computer History for Cua Driver](computer-history-architecture.md) for the
staged architecture, security limits, storage profile, launch gates, and later
NVIDIA OpenShell integration plan.
