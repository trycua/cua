# Cua-driver CI runners

These scripts are thin entrypoints around the Rust integration tests. They
build the repo-local fixture applications, run one strict Rust environment
preflight, set testkit paths, execute Rust targets, invoke the Rust report
validator, and collect artifacts. They do not define behavioral rows, push
code, or alter branches.

For the test layout and the distinction between unit tests, shared harnesses,
and native harnesses, see
`libs/cua-driver/docs/test-harnesses-guide.md`.

| Runner | Session | Canonical command |
| --- | --- | --- |
| `linux/run-rust-e2e.sh` | Existing Linux X11 or Wayland desktop | no selector |
| `linux/run-rust-e2e-wayland.sh` | Headless native Sway session | no selector |
| `linux/run-rust-e2e-inject.sh` | Nested `cua-compositor` session | no selector |
| `linux/run-rust-e2e-desktop.sh` | Existing representative Linux desktop | no selector |
| `windows/run-rust-e2e.ps1` | Windows console/RDP user session | `-RequireGui` |
| `macos/run-rust-e2e.sh` | Logged-in macOS session already prepared by the maintainer wrapper | no selector |

Use the command without a selector for the canonical complete run. CI sets the
private `CUA_E2E_INTERNAL_LANE` partition to `shared`, `native`, or `capture`
when it fans the same matrix into independent jobs. Those values are not public
alternate suites.

The maintainer-facing macOS command is
`libs/cua-driver/tests/runners/macos-lume/run-all.sh`. It verifies the private
Lume seed, installs the exact committed source, and then delegates to the thin
`macos/run-rust-e2e.sh` matrix runner above. There is no GitHub-hosted macOS GUI
job.

Run the Wayland wrapper through `nix develop .#cua-driver-wayland-e2e`. It
creates a pure Wayland session with Xwayland disabled and delegates every
scenario to `run-rust-e2e.sh`.

Run the nested compositor wrapper through
`nix develop .#cua-driver-inject-e2e`. This environment is experimental and
proves only the private compositor-owned route. Use `run-rust-e2e-desktop.sh`
for maintainer checks on representative GNOME, KDE, or real-Xorg sessions.

The GitHub-hosted Windows workflow is canonical when its strict preflight proves
an interactive desktop. The workflow also accepts a runner label so maintainers
can replay the same command on an Azure VM with an active RDP session for
environment parity; that replay is not a separate test definition or source of
behavioral truth.
