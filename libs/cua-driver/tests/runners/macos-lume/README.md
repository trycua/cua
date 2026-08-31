# Run macOS GUI E2E in a Lume golden image

This is the maintainer-owned macOS GUI acceptance gate for `cua-driver`. It is
not a GitHub Actions job. Run it on an Apple Silicon Mac with Lume, from a
disposable clone of a stopped SIP-disabled golden image.

The golden image supplies the logged-in Aqua session, stable local signing
identity, and existing Accessibility and Screen Recording grants. Every run
installs the requested source commit before testing. The preflight rejects a
source marker, source-built binary, or installed daemon that does not identify
that exact commit.

## Before you start

- Use an Apple Silicon Mac with enough free space for the 150 GB sparse guest.
- Install Lume and `jq` on the host.
- Start from a clean, committed checkout of this repository.
- Keep the public base, private seed, and workers on host-local Lume storage.

## Image contract

- Use the versioned public base `macos-tahoe-cua:26.5.2`; do not use `latest`
  for an acceptance result.
- Treat the public image as a sanitized base. It contains macOS Tahoe 26.5.2,
  SIP disabled, Xcode Command Line Tools 26.6, autologin, and SSH. It does not
  contain repository source, TCC grants, or a local signing identity.
- Build one private, host-local seed from the public base. The private seed owns
  the remaining toolchain, signing identity, and TCC grants and must never be
  pushed to a registry.
- Keep the named golden VM stopped and never run tests in it directly.
- Put no repository credentials, signing secrets, or maintainer private SSH
  keys in the guest. A host public key is sufficient for source sync.
- For reusable private seeds, request the app-owned Accessibility and Screen
  Recording grants through `CuaDriverLocal.app`. For disposable SIP-off workers
  that are not cloned from a granted seed, use the checked-in `seed-tcc.sh`
  helper below. Do not hand-edit `TCC.db`.
- Require a certificate-backed local signature. An ad-hoc signature invalidates
  the inherited grants on the next build.
- Clone one worker per run, retrieve its evidence, then delete the worker.

SIP-off alone does not grant Accessibility, Screen Recording, Automation, or
direct-capture consent. The reusable private seed still carries grants approved
through the normal `CuaDriverLocal.app` prompt flow. The helper below is only for
disposable SIP-off workers that need the same app-owned Accessibility and Screen
Recording grants without preserving a granted seed. The SIP-on check below proves
the normal user-facing permission flow still works with platform protection.

## Reuse a prepared private seed

Check the host-local inventory before building from the public base. Reuse a
seed only when its versioned maintainer log records the expected public base,
macOS build, toolchain versions, signing-certificate hash, TCC grants, and
Chrome consent state. The seed and both backups must be stopped:

```bash
SEED=cua-driver-macos-e2e-seed-26.5.2-YYYYMMDD
BACKUP_A="${SEED}-backup-a"
BACKUP_B="${SEED}-backup-b"

for VM in "$SEED" "$BACKUP_A" "$BACKUP_B"; do
  lume get "$VM" --format json | jq -e '.[0].status == "stopped"'
done
```

If those checks and the maintainer log match, skip image creation and clone a
worker from the seed under [Run the acceptance gate](#run-the-acceptance-gate).
Never boot a seed to inspect or update it. Clone a disposable worker and let the
acceptance preflight verify the recorded state there. Build a new versioned seed
when the inventory is missing, incomplete, or stale.

## Create the private seed

Pull the versioned public base into a mutable local builder. The initial guest
credentials are `lume` / `lume`:

```bash
IMAGE=macos-tahoe-cua:26.5.2
BUILDER=cua-driver-macos-e2e-builder-26.5.2
lume pull "$IMAGE" "$BUILDER"
lume run "$BUILDER"
```

Keep `lume run` open. In Terminal in the VM display, verify the immutable base
properties before installing anything:

```bash
sw_vers
csrutil status
xcode-select -p
xcrun swiftc --version
```

Require macOS 26.5.2 build 25F84, disabled SIP, and
`/Library/Developer/CommandLineTools`. Stop if any value differs.

Install Homebrew from the pinned, checksum-verified upstream installer. Run it
from Terminal in the VM display so macOS can request the `lume` password when
needed:

```bash
HOMEBREW_INSTALL_COMMIT=4b0227cf8416504142d23893368c2e1d211d5191
HOMEBREW_INSTALL_SHA256=99287f194a8b3c9e6b0203a11a5fa54518be57209343e6bb954dec4635796d9d
HOMEBREW_INSTALLER="/tmp/homebrew-install-${HOMEBREW_INSTALL_COMMIT}.sh"
curl -fsSL \
  "https://raw.githubusercontent.com/Homebrew/install/${HOMEBREW_INSTALL_COMMIT}/install.sh" \
  -o "$HOMEBREW_INSTALLER"
printf '%s  %s\n' "$HOMEBREW_INSTALL_SHA256" "$HOMEBREW_INSTALLER" \
  | shasum -a 256 -c -
/bin/bash "$HOMEBREW_INSTALLER"

eval "$(/opt/homebrew/bin/brew shellenv)"
brew install node ffmpeg jq rust

printf '\n%s\n' 'eval "$(/opt/homebrew/bin/brew shellenv)"' \
  >> "$HOME/.zprofile"
```

The pinned Homebrew installer pauses for a Return confirmation after its sudo
check. Keep this step in the VM display and confirm the prompt there. A detached
SSH run can look stalled while it is waiting for that input.

Open a new Terminal window and require each command to succeed:

```bash
xcrun swiftc --version
cargo --version
node --version
npm --version
ffmpeg -version | head -1
ffprobe -version | head -1
jq --version
```

Keep autologin, sleep prevention, and screen-lock prevention enabled. Add only
the maintainer host's public SSH key to `~/.ssh/authorized_keys`; never copy a
private key or registry credential into the guest.

The public base must already have the Command Line Tools agreement accepted.
If macOS presents that agreement while installing the tools, accept it in the
VM display, then rerun the four base-property checks above. Do not freeze a
builder while `xcrun` or `swiftc` still requests an agreement.

From another host terminal, get the builder address and sync a clean committed
checkout. The sync intentionally omits `.git` and writes the exact commit to
`.cua-e2e-source-sha`. It also excludes ignored credential files such as
`.env.local`.

```bash
BUILDER=cua-driver-macos-e2e-builder-26.5.2
VM_IP="$(lume get "$BUILDER" --format json | jq -r '.[0].ipAddress')"
libs/cua-driver/scripts/sync-vm-worktree.sh push "lume@${VM_IP}" '~/cua'
```

Back in Terminal in the VM display, install the app with the commit embedded
in the daemon. Tahoe Lume images can unlock their login keychain while still
rejecting the partition ACL that non-interactive `codesign` needs, so keep the
golden identity in a dedicated keychain. Create it with the same password as
the VM account so maintainers have only one local credential to enter:

```bash
SIGNING_KEYCHAIN="$HOME/Library/Keychains/cua-driver-signing.keychain-db"
security create-keychain "$SIGNING_KEYCHAIN"
security set-keychain-settings "$SIGNING_KEYCHAIN"
security list-keychains -d user -s "$SIGNING_KEYCHAIN"
security unlock-keychain "$SIGNING_KEYCHAIN"
export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN="$SIGNING_KEYCHAIN"
```

The first strict local install creates and imports the self-signed identity,
then stops without replacing the app if the new certificate is not usable yet.
Run it once, then open Keychain Access in the VM display, select the
`cua-driver-signing` keychain, open
`CuaDriver Local Signing (cua-driver-rs)`, and set Trust to Always Trust. Back
in Terminal, give Apple tooling access to the private key without storing the
keychain password in the image or repository:

```bash
cd ~/cua
export CUA_DRIVER_SOURCE_SHA="$(cat .cua-e2e-source-sha)"
bash libs/cua-driver/scripts/install-local.sh \
  --release --autostart --require-stable-signing

read -r -s -p 'Keychain password: ' KEYCHAIN_PASSWORD; echo
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" \
  "$SIGNING_KEYCHAIN"
unset KEYCHAIN_PASSWORD
```

Rerun the installer and require its `signed staged app with a stable local
identity` message before granting permissions through the app-owned flow:

```bash
bash libs/cua-driver/scripts/install-local.sh \
  --release --autostart --require-stable-signing
~/.local/bin/cua-driver-local permissions grant
```

Record the certificate hash after the successful install and keep that identity
for the life of the seed:

```bash
security find-certificate \
  -c 'CuaDriver Local Signing (cua-driver-rs)' -Z "$SIGNING_KEYCHAIN"
codesign -d -r- /Applications/CuaDriverLocal.app 2>&1 \
  | grep 'certificate leaf'
```

Recreating the certificate changes the app identity and invalidates inherited
TCC grants, even when the bundle identifier stays the same.

Complete the Accessibility and Screen Recording prompts. Tahoe 26 also asks
separately for Automation and direct ScreenCaptureKit access. Trigger all
remaining consent paths before freezing the seed:

```bash
# Terminal owns the test sentinel's read-only frontmost-state probes.
osascript -e \
  'tell application "System Events" to get name of first application process whose frontmost is true'

# The installed app owns driver-side app enumeration. This uses NSWorkspace and
# must not require Automation access to System Events.
~/.local/bin/cua-driver-local list_apps '{}'

# The app-owned grant flow probes a desktop capture and triggers Tahoe's
# direct-capture/private-window prompt.
~/.local/bin/cua-driver-local permissions grant
```

Choose Allow for `Terminal` -> `System Events` and on the CuaDriverLocal
direct-capture prompt. CuaDriverLocal app enumeration must not ask for System
Events. Target-specific Automation prompts may still appear later when a user
explicitly requests an Apple Events-backed browser or app operation; do not
pre-grant those in the seed. These are normal macOS consent flows; do not edit
`TCC.db` by hand while building a reusable seed. Rerun the commands and require
them to finish without another prompt.
Notification banners can cover Tahoe's consent controls; swipe the banners away
before acting instead of clicking through them. After enabling Screen Recording,
restart the app-owned daemon once before checking status so the live process
observes the new grant:

```bash
~/.local/bin/cua-driver-local stop
open -a CuaDriverLocal
```

Then verify the daemon's own identity and the read-only status contract before
running the explicit
LaunchServices-hosted grant flow. The first command must not raise a dialog;
the second is intentionally prompt-capable and must be run by the human:

```bash
~/.local/bin/cua-driver-local permissions status --json | jq -e '
  .accessibility == true
  and .screen_recording == true
  and .screen_recording_capturable == null
  and .direct_capture_status == "not_checked"
  and .source.attribution == "driver-daemon"
'
~/.local/bin/cua-driver-local permissions grant
~/.local/bin/cua-driver-local permissions status --json | jq -e '
  .accessibility == true
  and .screen_recording == true
  and .screen_recording_capturable == null
  and .direct_capture_status == "not_checked"
  and .direct_capture_verification.source == "permissions_grant"
  and (.direct_capture_verification.verified_at | endswith("Z"))
  and .direct_capture_verification.bundle_id == "com.trycua.driver.local"
'
codesign -d -r- /Applications/CuaDriverLocal.app 2>&1 | grep 'certificate leaf'
csrutil status
```

All five commands must succeed, and `csrutil status` must report disabled.

### Seed app-owned grants in disposable SIP-off workers

The public [Run Cua Driver in a macOS Lume VM](https://cua.ai/docs/how-to-guides/driver/run-in-macos-lume-vm)
guide grants macOS consent through the VM display. Keep using that prompt flow
for reusable private seeds. For automated disposable workers that are not cloned
from a granted seed, run the host helper after `CuaDriverLocal.app` is installed
in each running worker with a certificate-backed identity:

```bash
WORKER_A=cua-driver-macos-e2e-a
WORKER_B=cua-driver-macos-e2e-b
libs/cua-driver/tests/runners/macos-lume/seed-tcc.sh "$WORKER_A" "$WORKER_B"
```

Add `--storage <name-or-path>` when the workers live outside the default Lume
storage.

The wrapper resolves each VM's IP with `lume get`, copies
`seed-tcc-guest.sh` with `scp`, then runs it with `ssh` and guest `sudo`.
Use the default `lume` SSH password, set `LUME_SSH_PASSWORD`, or pass
`--ssh-password`. Password mode uses OpenSSH's askpass path; leave the password
empty when the VM accepts host SSH keys.
The guest helper refuses to write unless `sysctl -n hw.model` reports a
`VirtualMac*` VM and `csrutil status` reports disabled SIP. It derives the
permission identity from `/Applications/CuaDriverLocal.app`, writes only the
Accessibility and Screen Recording entries for that app, restarts `tccd`, and
verifies both entries.

After seeding, restart `CuaDriverLocal.app` before checking permission status if
`install-local --autostart` or an earlier probe may have started the daemon:

```bash
~/.local/bin/cua-driver-local stop
open -a CuaDriverLocal
```

If the VM sudo password is not the default `lume`, pass it without putting it
on the command line:

```bash
printf '%s\n' "$VM_SUDO_PASSWORD" \
  | libs/cua-driver/tests/runners/macos-lume/seed-tcc.sh \
      --sudo-password-stdin "$WORKER_A" "$WORKER_B"
```

This helper is for SIP-off disposable VMs only. Keep the SIP-on permission-flow
check below as the proof that the normal user-facing consent path still works.

Stop the builder and clone it to a date/version-named private seed plus two
stopped backups:

```bash
SEED=cua-driver-macos-e2e-seed-26.5.2-YYYYMMDD
BACKUP_A="${SEED}-backup-a"
BACKUP_B="${SEED}-backup-b"
lume stop "$BUILDER"
lume clone "$BUILDER" "$SEED"
lume clone "$SEED" "$BACKUP_A"
lume clone "$SEED" "$BACKUP_B"
lume ls
```

Require the seed and both backups to show `stopped`. Treat them as immutable and
local-only. Record their names, the public base tag, macOS build (`sw_vers`),
Lume version, CLT version, Rust version, Node version, and signing-certificate
hash in the maintainer log. Also record that the following consent paths were
granted and then rerun without prompts:

- `CuaDriverLocal.app`: Accessibility and Screen Recording
- Terminal controlling System Events
- CuaDriverLocal direct screen capture without the system picker. macOS labels
  this combined consent as screen and system-audio access even though Cua
  Driver's current ScreenCaptureKit recorder does not enable audio capture.

Build a new seed instead of updating one in place.

## Run the acceptance gate

Start from a clean committed host checkout. Give the worker a unique name:

```bash
SEED=cua-driver-macos-e2e-seed-26.5.2-YYYYMMDD
WORKER="cua-driver-macos-e2e-$(date -u +%Y%m%dT%H%M%SZ)"
lume clone "$SEED" "$WORKER"
echo "$WORKER"
lume run "$WORKER"
```

Keep `lume run` open. From another host terminal, set `WORKER` to the printed
name and sync the exact host commit:

```bash
WORKER=cua-driver-macos-e2e-YYYYMMDDTHHMMSSZ
VM_IP="$(lume get "$WORKER" --format json | jq -r '.[0].ipAddress')"
libs/cua-driver/scripts/sync-vm-worktree.sh push "lume@${VM_IP}" '~/cua'
```

Open Terminal in the VM display and run the single guest entrypoint. Do not run
it over SSH: GUI fixtures must inherit the logged-in console session. The
runner asks for the dedicated keychain password and then the console user's
login Keychain password after each worker boot. The explicit second unlock is
required because a fresh public image can leave the login Keychain locked even
after automatic GUI login. Use the same local VM credential for both prompts,
and do not put it in the repository, image, logs, or artifacts.

```bash
cd ~/cua
libs/cua-driver/tests/runners/macos-lume/run-all.sh
```

When Chrome or Edge is installed in the disposable worker, include the optional
standalone browser-tool matrix in the same exact-source run:

```bash
cd ~/cua
libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser
```

This adds the declared adversarial installed-browser rows and writes their
separate typed results and MP4 evidence under
`artifacts/cua-driver/macos-standalone-browser/`. Missing external browsers are
a hard failure for this option; they never shrink the reported matrix. On a
repeat run, the entrypoint preserves the previous standalone-browser evidence
in a temporary archive before creating a fresh artifact directory. The
entrypoint temporarily restarts the disposable worker daemon in unrestricted
mode for the authorized existing-profile success rows, then restores its
standard autostart daemon even when a browser row fails.

On macOS Tahoe, first-use Chrome can present a native local-network discovery
prompt over `chrome://inspect/#remote-debugging`. The standalone-browser lane
uses loopback DevTools and does not need LAN discovery. Before freezing a seed
that will run this optional lane, complete Chrome's welcome screen without
signing in and leave the default-browser and usage-reporting choices disabled.
Launch Chrome on that exact page in the VM display, choose **Don't Allow**, quit
Chrome, then relaunch the page and require that the prompt does not return. Do
not answer or dismiss OS consent UI while the behavior matrix is running. If an
existing immutable seed lacks these decisions, clone it to a new versioned seed,
complete this setup there, stop it, and use that new seed for workers; never
update the original seed in place.

The entrypoint refuses the wrong OS, user session, SIP state, dirty or
unidentified source, missing dependencies, ad-hoc signature, unavailable login
Keychain, stale installed daemon, unusable TCC grants, or missing
Terminal/CuaDriver Automation grants. It reinstalls the exact source commit and
then runs the canonical macOS matrix.

## Build isolation across reruns

The entrypoint builds in a Cargo target directory owned by one invocation,
nested under the exact source SHA:

```
~/Library/Caches/cua-driver-e2e/cargo-target/<source-sha>/<run-id>
```

Nothing is ever deleted to make that true. The seed image's `rust/target`, and
every previous invocation's namespace stay untouched and unused. A rerun cannot
inherit build state from another commit or an earlier run of the same commit,
which is the shape that produced duplicate Swift bridge symbols during 0.13.0
certification. The Tauri harness inherits the same run-owned Cargo namespace.
The Electron harness archives any inherited `node_modules` tree and reinstalls
from its lockfile for the current run.

- `CUA_E2E_CARGO_TARGET_ROOT=<absolute dir>` moves the namespace root, for
  example onto a larger volume.

The runner records the namespace it used in
`artifacts/cua-driver/macos/cargo-target-dir.txt`, and the matrix reads the
built driver binary out of that same directory. Before a new invocation starts,
the runner moves the previous canonical artifact directory to
`artifacts/cua-driver/macos-history/<run-id>`, so logs and retry evidence from
the prior run cannot be mistaken for the current run.

## Retry exactly one flaky cell

The runner has no automatic retries. A retry has to be authorized on the command
line for one exact cell, and it can never convert a real persistent failure into
a pass.

```bash
cd ~/cua
# Full matrix, plus at most one retry of one cell if that cell is the only failure.
libs/cua-driver/tests/runners/macos-lume/run-all.sh \
  --retry-cell macos-electron-drag-px-foreground \
  --retry-harness electron \
  --retry-attempts 2
```

Cell ids are the ones in `results.jsonl` and `summary.md`, such as
`macos-electron-drag-px-foreground`. `--retry-attempts` accepts 1-3 and defaults
to 1. `--retry-harness` is optional and must match the failing row's harness.

After a failing full matrix the runner retries only when all of these hold:

- exactly one typed cell did not pass, and it is the `--retry-cell` selection;
- the only failing lane is `shared-app-matrix` or the one SwiftUI test that
  owns the selected cell;
- the environment preflight, typed report validation, and trajectory-video
  checks all passed;
- the failure record contains exactly one failure signal.

Otherwise it prints why the retry was refused and exits with the matrix's
failure. Shared web-action cells and the five typed SwiftUI cells are retryable.
The runner routes a `macos-swiftui-*` selection to the native lane and invokes
only the test that owns that exact cell. Reproduce other native, capture, or
embedded-browser failures with a full rerun.

To rerun just that cell later in the same booted worker after the first run
already restored standard mode, use `--retry-only`. It reinstalls the exact
commit, then starts and verifies the unrestricted worker daemon before running
the selection, so the retry cannot silently execute against the standard daemon:

```bash
cd ~/cua
libs/cua-driver/tests/runners/macos-lume/run-all.sh \
  --retry-only \
  --retry-cell macos-swiftui-left-click-ax-background \
  --retry-harness swiftui \
  --retry-attempts 3
```

Every attempt keeps its own evidence under `artifacts/cua-driver/macos/`:

- `attempts/attempt-1-full-matrix/` holds the failing full matrix's typed rows,
  summary, logs, and MP4 trajectories.
- `attempts/retry-<n>/` holds each superseded retry attempt.
- The artifact root holds the final attempt.
- `retry-record.json` records the selection, the initial failing cells and
  lanes, every attempt with its exit code and evidence path, and a
  `final_status` of `pass-after-retry`, `fail-persistent`, `pass-retry-only`, or
  `fail-retry-only`.
- `summary.md` gains a **Retry provenance** section, so the human-facing report
  still states the initial failure that a green retry followed.
- `failures.json` classifies what failed in the last attempt: failing lanes,
  video failures, preflight, and report validation.

The run exits non-zero when the selected cell fails every allowed attempt.
It also exits non-zero if the filter produces anything other than exactly that
one cell, so the retry record never overstates the precision of the rerun.
Report a retried pass as a retried pass, with the record above; a cell that
needs a retry on every certification run is a defect, not a flake.

Pull evidence before deleting the worker, even after a failed run:

```bash
REMOTE_ARTIFACT_DIR=artifacts/cua-driver/macos \
  libs/cua-driver/scripts/sync-vm-worktree.sh pull-artifacts \
  "lume@${VM_IP}" '~/cua'
# Also retrieve this directory when --standalone-browser was used.
REMOTE_ARTIFACT_DIR=artifacts/cua-driver/macos-standalone-browser \
  libs/cua-driver/scripts/sync-vm-worktree.sh pull-artifacts \
  "lume@${VM_IP}" '~/cua'
lume stop "$WORKER"
lume delete "$WORKER" --force
```

The host stores the pulled summary, typed JSONL rows, environment record, logs,
screenshots, MP4 trajectories, and any retry record under
`artifacts/cua-driver/vm/`, which Git ignores. A setup failure is an environment
failure, not permission to report a smaller green matrix.

The runner owns the worker daemon's lifecycle through one code path: it unloads
the standard autostart agent, starts the unrestricted daemon, verifies the mode,
watchdogs it, and restores the standard autostart daemon from an `EXIT` trap
after success or failure. A restoration that does not come back in standard mode
fails an otherwise green run. Every matrix attempt, including a targeted retry,
goes through that same owner.

If the GUI login session ends while the trap is running, launchd has no GUI
domain in which to start the daemon. The runner records
`standard-daemon-restore-deferred.txt` and leaves the installed LaunchAgent in
place so macOS loads it in standard mode at the next GUI login. This is the only
deferred restoration case; a live GUI session that cannot return to standard
mode still fails the run.

## Test the runner itself

The runner's shell helpers, including status matching, build-namespace selection,
option validation, retry bookkeeping, evidence archiving, and daemon restoration, are
covered by host-side tests that need no VM:

```bash
python -m pytest libs/cua-driver/scripts/tests/test_macos_lume_runner.py -v
bash -n libs/cua-driver/tests/runners/macos-lume/run-all.sh
bash -n scripts/ci/macos/run-rust-e2e.sh
```

## Validate the SIP-on permission flow

Run this disposable lane before a release and after macOS, Lume, signing, or
TCC-related changes. It verifies the supported user flow rather than the
golden image's inherited grants.

1. Clone the golden seed to a new worker while both are stopped.
2. Run `lume sip on <worker> --yes`.
3. Boot it with a display, sync the exact source as above, and install it with
   `CUA_DRIVER_SOURCE_SHA` set to `.cua-e2e-source-sha`.
4. In the VM display, reset only the disposable worker's grants:

   ```bash
   tccutil reset Accessibility com.trycua.driver.local
   tccutil reset ScreenCapture com.trycua.driver.local
   ~/.local/bin/cua-driver-local permissions grant
   ```

5. Complete the prompts and require the same four-field
   `permissions status --json` check used during image creation.
6. Run `scripts/ci/macos/run-rust-e2e.sh` directly to prove the canonical
   preflight and matrix with SIP enabled.
7. Pull artifacts, stop the worker, and delete it. Never promote this mutated
   worker back to the SIP-off seed.

## Repair or rotate the seed

Discard and rebuild the private seed when its signature becomes ad-hoc, the
permission check fails, the public base or toolchain needs an update, or the
seed has been booted for a test. Keep the last known-good stopped seed until its
replacement passes one full worker run and one SIP-on permission-flow check.
