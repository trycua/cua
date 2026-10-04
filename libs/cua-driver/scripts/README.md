# cua-driver scripts

Install, uninstall, local-build, and VM sync helpers for cua-driver.

| Script | Purpose |
| --- | --- |
| `install.sh` / `install.ps1` | Install released cua-driver binaries |
| `install-local.sh` / `install-local.ps1` | Build this checkout as the separate `cua-driver-local` product |
| `uninstall-local.sh` / `uninstall-local.ps1` | Remove only the source-built `cua-driver-local` product |
| `uninstall.sh` / `uninstall.ps1` | Remove installed driver artifacts |
| `_install-common.sh` / `_install-common.psm1` | Shared install helper logic |
| `_install-rust.sh` / `_install-local-rust.sh` | Rust build/install internals |
| `sync-vm-worktree.sh` | Sync this checkout to verification VMs and pull artifacts back |
| `post-install-hints.txt` | User-facing hints printed by install scripts |

## Stable macOS local signing

macOS Accessibility and Screen Recording grants are tied to an app's
designated requirement. An ad-hoc signature uses a `cdhash` requirement that
changes on every rebuild, so its grants do not survive the next local install.
The installer now reports whether the installed requirement is
`certificate-backed` or `ad-hoc cdhash`; an ad-hoc install always prints a
prominent warning and bootstrap instructions.

For behavior or E2E verification, require the stable path:

```bash
bash libs/cua-driver/scripts/install-local.sh \
  --release --autostart --require-stable-signing
```

`CUA_DRIVER_REQUIRE_STABLE_SIGNING=1` is the environment equivalent. Strict
mode stops before replacing the live app when no usable certificate-backed
identity is available.

For the most reliable non-interactive rebuilds, use a dedicated keychain:

```bash
SIGNING_KEYCHAIN="$HOME/Library/Keychains/cua-driver-signing.keychain-db"
security create-keychain "$SIGNING_KEYCHAIN"  # first time only
security set-keychain-settings "$SIGNING_KEYCHAIN"
security unlock-keychain "$SIGNING_KEYCHAIN"
export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN="$SIGNING_KEYCHAIN"
```

To use an existing certificate without allowing the installer to select a
different identity from that keychain, also provide its exact SHA-1 fingerprint:

```bash
export CUA_DRIVER_LOCAL_SIGNING_IDENTITY="<40-hex-character SHA-1>"
```

The installer fails closed when that exact usable code-signing identity is not
present in `CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN`.

The first install creates `CuaDriver Local Signing (cua-driver-rs)` in that
keychain and then authorizes its private key for Apple's code-signing tools,
which `security import` alone does not do: since macOS 10.12 an unauthorized
key makes the first `codesign` prompt for a keychain password or fail with
`errSecInternalComponent`.

Whether that authorization needs the keychain password depends on the host. The
installer tries without one first, and when the host insists it asks for the
password on the terminal, uses it for that single `security` call and drops it
immediately: nothing is exported, so no password reaches the build. With no
terminal to ask on, such as a scripted install, the installer says the key is
not authorized and prints the command to run; it does not claim to have
authorized it. Later installs retry the passwordless form silently and never
prompt, so a key that needed the password is authorized once, by hand:

```bash
security unlock-keychain "$SIGNING_KEYCHAIN"
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -l 'CuaDriver Local Signing (cua-driver-rs)' -t private -s \
  -k '<keychain-password>' "$SIGNING_KEYCHAIN"
```

`-l` is what keeps the change to that one key; `-s` on its own matches every
signing key in the keychain, including unrelated identities in a login keychain.
Drop `-k` if the host does not ask for the password.

That command shape is covered by a test which is skipped by default because it
touches a real keychain. On macOS, run it in a throwaway keychain with:

```bash
CUA_DRIVER_LOCAL_SIGNING_REAL_KEYCHAIN_TEST=1 \
  python -m pytest libs/cua-driver/scripts/tests/test_install_local.py
```

Trusting the certificate in Keychain Access is optional. codesign then pins it
as `certificate root` rather than `certificate leaf`; both are stable across
rebuilds and both are accepted.

Then rerun the strict installer and grant Accessibility and Screen Recording
once. When the dedicated default keychain above exists, the installer prefers
it automatically; exporting `CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN` remains the
most explicit choice.

Released installers show a telemetry notice before asking the installed binary
to record anything. Telemetry is enabled by default and can be persistently
disabled with `cua-driver telemetry disable`. Installation events use the same
consent decision as routine events. A normal uninstall preserves the pseudonymous
installation ID and preference for a future reinstall; use `--purge` on Unix,
or set `CUA_DRIVER_RS_UNINSTALL_PURGE=1` on Windows, to delete them.

Keep source commits host-owned. Verification machines should sync from this
checkout and return artifacts, not push code.

Local and released installations are removed independently:

```bash
# macOS / Linux, from the checkout
libs/cua-driver/scripts/uninstall-local.sh

# Windows, from the checkout
libs/cua-driver/scripts/uninstall-local.ps1
```

The local uninstaller leaves `cua-driver`, `CuaDriver.app`, release services,
release state, and release TCC grants untouched. On macOS it revokes only
`com.trycua.driver.local`; pass `--keep-tcc` to retain that local grant.

The release Unix uninstaller shuts down the release service before removing
anything. It first requires the systemd/launchd supervisor to stop, then uses
the daemon PID file to validate the installed release process and invokes the
trusted installed helper as `cua-driver --expected-pid <pid> stop`. A helper
that supports this option reads daemon metadata and requires the daemon PID to
match before sending shutdown; older helpers reject this argv shape instead of
silently stopping an unrelated default-socket daemon. The uninstaller escalates
only the already-validated release PID if graceful shutdown is unavailable, and
verifies the daemon stays stopped before cleanup begins.

With a missing or stale PID file, the script performs only a narrow
release-executable process check. It never signals an ambiguous process. If
supervisor shutdown, ownership validation, process inspection, or final
shutdown verification cannot be proven safe, uninstall aborts non-zero while
the runtime is still in place.
