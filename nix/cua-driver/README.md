# Linux driver checks

Nix is the Linux test environment. It is used for reproducible builds and for
containerized or compositor-backed Linux checks; Windows and macOS checks do
not belong here.

## Check tiers

| Tier | Check family | Trigger | Evidence |
| --- | --- | --- | --- |
| Source | `cua-driver-linux-rust-unit` | Linux Rust/Nix changes | Cargo unit tests and compilation |
| Contract | `cua-driver-integration`, `cua-driver-set-config`, `cua-driver-screenshot` | Maintainer or Linux CI | service, config, and capture assertions |
| Behavioral | `e2e-rust-linux` and the shared Rust matrix | Manual maintainer dispatch | external app state, AX trees, logs, failure screenshots |
| Supporting | legacy background GUI and compatibility checks | Explicitly selected | toolkit-specific diagnostics |

The behavioral matrix is owned by
`libs/cua-driver/rust/crates/cua-driver/tests/cross_platform_behavior_test.rs`.
Nix checks must not create a second scenario list in Nix expressions.

The current automatic Nix lane builds the driver and Rust workspace from source.
The manual Linux desktop runner builds Electron/Tauri from the repo-local
fixture sources in the desktop session. Moving those app builds fully inside
Nix requires committing and hashing their dependency lockfiles, especially the
Electron npm graph, and is intentionally a follow-up rather than an opaque
prebuilt download.

## Naming and artifacts

Use `cua-driver-linux-<surface>-<scenario>` for new checks. Do not add `gif`
to new check names. A failing behavioral check should retain structured output,
the driver log, an AX/UIA tree where available, and a PNG when the scenario is
pixel-based. GIF recorders remain legacy supporting diagnostics until their
behavior is covered by the Rust matrix.

The old `linux-background-gui.nix` and compositor matrix are not deleted in one
step. They contain useful toolkit and regression evidence, but read-only
skeleton entries must not be reported as equivalent to a passing user workflow.
