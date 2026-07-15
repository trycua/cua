# Run macOS GUI E2E in a Lume golden image

This is the maintainer-owned macOS GUI acceptance gate for `cua-driver`. It is
not a GitHub Actions job. Run it on an Apple Silicon Mac with Lume, from a
disposable clone of a stopped SIP-disabled golden image.

The golden image supplies the logged-in Aqua session, stable local signing
identity, and existing Accessibility and Screen Recording grants. Every run
installs the requested source commit before testing. The preflight rejects a
source marker, source-built binary, or installed daemon that does not identify
that exact commit.

## Golden-image contract

- Use the verified Lume Tahoe unattended image on an Apple Silicon host.
- Keep the named golden VM stopped and never run tests in it directly.
- Put no repository credentials, signing secrets, or maintainer private SSH
  keys in the guest. A host public key is sufficient for source sync.
- Grant TCC permissions through `CuaDriver.app`. Do not edit `TCC.db`.
- Require a certificate-backed local signature. An ad-hoc signature invalidates
  the inherited grants on the next build.
- Clone one worker per run, retrieve its evidence, then delete the worker.

SIP-off makes the behavior environment reproducible; it is not treated as
proof that the normal macOS permission flow works. The SIP-on check below owns
that separate claim.

## Build the golden image

On the host, create a mutable builder with the Tahoe unattended preset. The
preset enables SSH and autologin and initially uses `lume` / `lume`.
Install Lume's optional Recovery input helper before changing SIP:

```bash
python3 -m pip install --user vncdotool
```

```bash
IPSW_URL="$(lume ipsw | tail -n 1)"
curl -L "$IPSW_URL" -o ~/Downloads/macos-tahoe.ipsw
lume create cua-driver-macos-e2e-builder \
  --ipsw ~/Downloads/macos-tahoe.ipsw \
  --unattended tahoe
lume sip off cua-driver-macos-e2e-builder --yes
lume run cua-driver-macos-e2e-builder
```

Keep `lume run` open. In the VM display:

1. Finish any first-login prompts and keep autologin, sleep prevention, and
   screen-lock prevention enabled.
2. Install Xcode Command Line Tools, Rust, Node.js/npm, `ffmpeg`, and `jq`.
3. Verify that `xcrun swiftc`, `cargo`, `npm`, `ffmpeg`, `ffprobe`, and `jq`
   are available in a new Terminal window.
4. Add only the maintainer host's public SSH key to
   `~/.ssh/authorized_keys`.

From another host terminal, get the builder address and sync a clean committed
checkout. The sync intentionally omits `.git` and writes the exact commit to
`.cua-e2e-source-sha`.

```bash
VM_IP="$(lume get cua-driver-macos-e2e-builder --format json | jq -r '.ipAddress')"
libs/cua-driver/scripts/sync-vm-worktree.sh push "lume@${VM_IP}" '~/cua'
```

Back in Terminal in the VM display, install the app with the commit embedded
in the daemon, then grant permissions through the app-owned flow:

```bash
cd ~/cua
export CUA_DRIVER_SOURCE_SHA="$(cat .cua-e2e-source-sha)"
bash libs/cua-driver/scripts/install-local.sh --release --autostart
~/.local/bin/cua-driver permissions grant
```

Complete the Accessibility and Screen Recording prompts. Then verify the
daemon's own identity, live capture permission, stable signature, and SIP
state:

```bash
~/.local/bin/cua-driver permissions status --json | jq -e '
  .accessibility == true
  and .screen_recording == true
  and .screen_recording_capturable == true
  and .source.attribution == "driver-daemon"
'
codesign -d -r- /Applications/CuaDriver.app 2>&1 | grep 'certificate leaf'
csrutil status
```

All three commands must succeed, and `csrutil status` must report disabled.
Stop the builder and clone it to a date/version-named seed:

```bash
lume stop cua-driver-macos-e2e-builder
lume clone cua-driver-macos-e2e-builder cua-driver-macos-e2e-seed-YYYYMMDD
```

Treat `cua-driver-macos-e2e-seed-YYYYMMDD` as immutable. Record its name,
macOS build (`sw_vers`), Lume version, Xcode version, Rust version, Node version,
and signing-certificate hash in the release or maintainer log. Build a new seed
instead of updating this one in place.

## Run the acceptance gate

Start from a clean committed host checkout. Give the worker a unique name:

```bash
SEED=cua-driver-macos-e2e-seed-YYYYMMDD
WORKER="cua-driver-macos-e2e-$(date -u +%Y%m%dT%H%M%SZ)"
lume clone "$SEED" "$WORKER"
echo "$WORKER"
lume run "$WORKER"
```

Keep `lume run` open. From another host terminal, set `WORKER` to the printed
name and sync the exact host commit:

```bash
WORKER=cua-driver-macos-e2e-YYYYMMDDTHHMMSSZ
VM_IP="$(lume get "$WORKER" --format json | jq -r '.ipAddress')"
libs/cua-driver/scripts/sync-vm-worktree.sh push "lume@${VM_IP}" '~/cua'
```

Open Terminal in the VM display and run the single guest entrypoint. Do not run
it over SSH: GUI fixtures must inherit the logged-in console session.

```bash
cd ~/cua
libs/cua-driver/tests/runners/macos-lume/run-all.sh
```

The entrypoint refuses the wrong OS, user session, SIP state, dirty or
unidentified source, missing dependencies, ad-hoc signature, stale installed
daemon, or unusable TCC grants. It reinstalls the exact source commit and then
runs the canonical macOS matrix.

Pull evidence before deleting the worker, even after a failed run:

```bash
REMOTE_ARTIFACT_DIR=artifacts/cua-driver/macos \
  libs/cua-driver/scripts/sync-vm-worktree.sh pull-artifacts \
  "lume@${VM_IP}" '~/cua'
lume stop "$WORKER"
lume delete "$WORKER" --force
```

The host stores the pulled summary, typed JSONL rows, environment record,
logs, screenshots, and MP4 trajectories under
`libs/cua-driver/docs/vm-artifacts/`. A setup failure is an environment failure,
not permission to report a smaller green matrix.

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
   tccutil reset Accessibility com.trycua.driver
   tccutil reset ScreenCapture com.trycua.driver
   ~/.local/bin/cua-driver permissions grant
   ```

5. Complete the prompts and require the same four-field
   `permissions status --json` check used during image creation.
6. Run `scripts/ci/macos/run-rust-e2e.sh` directly to prove the canonical
   preflight and matrix with SIP enabled.
7. Pull artifacts, stop the worker, and delete it. Never promote this mutated
   worker back to the SIP-off seed.

## Repair or rotate the seed

Discard and rebuild the golden image when its signature becomes ad-hoc, the
permission check fails, the OS or toolchain needs an update, or the seed has
been booted for a test. Keep the last known-good stopped seed until its
replacement passes one full worker run and one SIP-on permission-flow check.
