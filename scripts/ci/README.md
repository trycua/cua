# Cua-driver CI runners

These scripts are thin entrypoints around the Rust integration tests. They
build the repo-local fixture applications on demand, set the testkit path
overrides, and collect logs. They do not push code or alter branches.

| Runner | Session | Canonical command |
| --- | --- | --- |
| `linux/run-rust-e2e.sh` | Linux X11/Wayland desktop | `--suite shared` |
| `windows/run-rust-e2e.ps1` | Windows console/RDP user session | `-Suite shared -RequireGui` |

The Windows workflow accepts a runner label so the same command can execute on
the Azure VM's active-RDP runner when that self-hosted label is configured. A
GitHub-hosted Windows runner is useful for smoke validation, but it is not a
substitute for the Azure user-session run.
