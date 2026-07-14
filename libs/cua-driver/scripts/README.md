# cua-driver scripts

Install, uninstall, local-build, and VM sync helpers for cua-driver.

| Script | Purpose |
| --- | --- |
| `install.sh` / `install.ps1` | Install released cua-driver binaries |
| `install-local.sh` / `install-local.ps1` | Build and install from this checkout |
| `uninstall.sh` / `uninstall.ps1` | Remove installed driver artifacts |
| `_install-common.sh` / `_install-common.psm1` | Shared install helper logic |
| `_install-rust.sh` / `_install-local-rust.sh` | Rust build/install internals |
| `sync-vm-worktree.sh` | Sync this checkout to verification VMs and pull artifacts back |
| `post-install-hints.txt` | User-facing hints printed by install scripts |

Released installers show a telemetry notice before asking the installed binary
to record anything. Telemetry is enabled by default and can be persistently
disabled with `cua-driver telemetry disable`. Installation events use the same
consent decision as routine events. A normal uninstall preserves the pseudonymous
installation ID and preference for a future reinstall; use `--purge` on Unix,
or set `CUA_DRIVER_RS_UNINSTALL_PURGE=1` on Windows, to delete them.

Keep source commits host-owned. Verification machines should sync from this
checkout and return artifacts, not push code.
