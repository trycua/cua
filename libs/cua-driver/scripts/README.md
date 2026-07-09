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

Keep source commits host-owned. Verification machines should sync from this
checkout and return artifacts, not push code.
