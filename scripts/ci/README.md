# Cua-driver CI runners

These scripts are thin entrypoints around the Rust integration tests. They
build the repo-local fixture applications, run one strict Rust environment
preflight, set testkit paths, execute Rust targets, invoke the Rust report
validator, and collect artifacts. They do not define behavioral rows, push
code, or alter branches.

For the test layout and the distinction between unit tests, shared harnesses,
native harnesses, and modality tests, see
`libs/cua-driver/docs/test-harnesses-guide.md`.

| Runner | Session | Canonical command |
| --- | --- | --- |
| `linux/run-rust-e2e.sh` | Linux X11/Wayland desktop | no selector |
| `windows/run-rust-e2e.ps1` | Windows console/RDP user session | `-RequireGui` |
| `macos/run-rust-e2e.sh` | Logged-in macOS session with TCC | no selector |

Use the `all` command for the canonical run. The `shared`, `native`, `guard`,
and `modality` selectors are retained only when diagnosing one lane locally or
when the CI workflow fans the matrix out into independent jobs.

The Windows workflow accepts a runner label so the same command can execute on
the Azure VM's active-RDP runner when that self-hosted label is configured. A
GitHub-hosted Windows runner is useful for smoke validation, but it is not a
substitute for the Azure user-session run.
