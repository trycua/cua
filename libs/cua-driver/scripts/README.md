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
keychain. If `codesign` cannot use its private key non-interactively, unlock
the keychain, trust the certificate in Keychain Access, and authorize Apple
code-signing tools:

```bash
read -r -s -p 'Keychain password: ' KEYCHAIN_PASSWORD; echo
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" \
  "$SIGNING_KEYCHAIN"
unset KEYCHAIN_PASSWORD
```

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

Every uninstaller stops the running daemon before it deletes anything:
`uninstall.ps1` stops each `cua-driver.exe`, and `uninstall.sh` stops
`cua-driver serve` on macOS and Linux by matching argv[0] over the release
install paths, gated on the same on-disk Rust marker that guards every other
shared-path removal. Selecting candidates by argv[0] also covers versioned
release paths the installer has already pruned, since a daemon that survived an
upgrade runs from a directory that no longer exists. Whether a candidate is a
daemon is then decided by scanning its real command line the way the CLI does,
not by pattern: flags may precede the subcommand (`cua-driver --no-overlay
serve` is a daemon) and only a value-taking flag consumes the token after it,
so `cua-driver --socket serve mcp` is an MCP child whose socket is named
`serve`, and `cua-driver --no-overlay call serve` is a `call`. Neither is
signalled.

That scan needs real argument boundaries, which Linux provides through
`/proc/<pid>/cmdline`. macOS exposes no equivalent to a shell — `ps` joins the
arguments with spaces — so when a value-taking flag precedes the subcommand
there, the uninstaller cannot tell `--socket "/tmp/a serve" mcp` from
`--socket /tmp/a serve`. It reports `daemon_stop_undetermined` and leaves the
process alone rather than guessing. Daemons started the way the product starts
them are unaffected: the LaunchAgent, the systemd unit, `install-local.sh
--autostart` and the runtime's own relaunch all put `serve` ahead of any flag.

A daemon started from PATH has argv[0] `cua-driver`, which any build on the
machine can have, so those candidates are kept only when the executable behind
the pid resolves into the install — a vendored or hand-built `cua-driver serve`
is not the uninstaller's to stop.

`cua-driver mcp` runs as a stdio child of a live MCP client and exits with that
client, so a surviving one is reported rather than killed.

Stopping a pid is not the same as stopping the daemon: the autostart teardown
is best-effort, so after the pids are confirmed gone the scan runs again and a
supervisor that survived it is caught by the new pid it started. A daemon that
outlives SIGTERM and SIGKILL, or one that keeps coming back, is reported on
stderr as `daemon_stop_incomplete`; a process whose subcommand could not be
resolved is reported as `daemon_stop_undetermined`. In either case the removal
still completes, but the closing line says a cua-driver process is still
running and the script exits non-zero — the uninstaller never reports a stop it
did not achieve, and a script that chains a reinstall should not proceed against
a live daemon holding the old socket.
