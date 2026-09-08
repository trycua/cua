# Permission Simplification Execution Journal

**Status:** Active

**Branch:** `codex/permission-simplification-0130`

**Starting source:** `e2c52d50ba331798a3da4871fdad3bbcdd399633`

**Starting Cua Driver version:** `0.12.6`

## Completion bar

- Implement the reconciled standard, bounded, and unrestricted contracts.
- Preserve explicit authorization for an existing authenticated Chromium
  profile.
- Remove Cua-owned consent modal and banner behavior.
- Preserve the vector semantic cursor and add a sanitized public-session badge.
- Update Rust, CLI, MCP, Python, TypeScript, installers, examples, and public
  documentation.
- Validate the exact review SHA locally and through representative macOS,
  Windows, Linux X11, Linux Wayland, and Linux headless environments.
- Record cursor and interaction videos on macOS, Windows, and Linux.
- Merge verified dependencies and the completed implementation.
- Do not create or publish a component release.

## Checkpoints

### Source synchronization

- Merged dependency PR #2603 after all required checks passed.
- Created the implementation branch from the resulting current `origin/main`.
- Verified the worktree is clean and contains the exact upstream commit.
- Preserved unrelated local planning files in their original worktree.

### Descriptor and provenance foundations

- Added an explicit allow, deny, manifest, or grant behavior matrix to every
  reviewed enforcement adapter.
- Made routine standard observation, input, file transfer, recording, browser
  input, and agent-adjustable configuration independent of the legacy consent
  broker.
- Kept unbounded page mutation and operating-system permission prompting
  denied outside their trusted boundaries.
- Replaced PID-only process ownership records with process fingerprints.
- Added launch-time running-process snapshots and post-launch attestation so
  only a newly observed process can enter the runtime ownership registry.
- Added dispatch-time fingerprint re-proof and stale-provenance removal before
  driver-owned process termination.
- Denied foreign-process termination in standard mode without opening a
  consent surface.
- Verification:
  - `cargo test -p cua-driver-core --lib`: 413 passed.
  - `cargo check -p cua-driver-core --all-targets`: passed.

### Practical bounded mode and terminal revocation

- Added manifest version 2 while keeping version 1 loadable.
- Added application identity grants, practical directory roots, browser
  profile kinds, and driver-owned versus foreign termination rules.
- Made path-root matching component-aware and canonical-path based.
- Enriched live window and process attestations with application identity
  before manifest matching.
- Allowed an existing-profile browser binding directly from a matching
  bounded manifest, without a consent provider or indicator.
- Added a direct no-provider bounded dispatch test.
- Added stable dispatch refusals for ended sessions, revoked authorization
  contexts, and a terminal runtime revoke-all latch.
- Made revoke-all reject later calls even when they introduce a new public
  session label.
- Verification:
  - `cargo check -p cua-driver-core --all-targets`: passed.
  - `cargo check -p cua-driver --all-targets`: passed.
  - Focused bounded no-provider and terminal-revocation tests: passed.

### Host integrations, UI removal, and session identity

- Added repeatable `--grant existing-profile` launch configuration to `mcp`
  and `serve`, including proxy forwarding and restart refusal when an
  incompatible daemon is already live.
- Added the public `DriverAuthorizationHost` callback and content-free
  `DriverActivityObserver` to the Rust, Python, and TypeScript SDK surfaces.
- Added activity events for authorized actions, authorization refusals,
  ordinary failures, grant lifecycle, and session lifecycle.
- Removed the native Cua authorization modal, banner, helper modes, platform
  consent renderers, and the complete `overlay-ui` crate.
- Preserved browser-owned Chromium connection prompts and operating-system
  permission flows.
- Added a renderer-owned public-session badge below the semantic cursor using
  bundled Inter artwork, sanitization, stable session color, and live backing
  scale.
- Added the badge to macOS, Windows, Linux X11 and layer-shell rendering, plus
  GNOME Wayland helper API v6.
- Regenerated Python and TypeScript UniFFI bindings.
- Verification:
  - Core, SDK, and cursor unit suites: 482 passed.
  - `cargo test -p cua-driver` unit suite: 136 passed.
  - TypeScript package suite: 6 passed.
  - Python package suite: 28 passed, 3 skipped because no bundled executable
    was staged for those optional checks.
  - `cargo fmt --all -- --check`: passed before the final naming cleanup and
    will be rerun on the review SHA.

### Public contract and migration guidance

- Replaced prompt-heavy standard-mode guidance with the promptless practical
  default.
- Added the mode matrix, bounded manifest v2 guide, launch-grant workflow,
  SDK host callback examples, activity-observer contract, revocation behavior,
  same-user boundary, session badge, and headless behavior.
- Updated the generated CLI and MCP references, package READMEs, embedded and
  browser skills, cursor authoring docs, update guide, and shared post-install
  hints.
- Removed the obsolete native consent UI screenshot from the public docs.

### Local review gate

- Verified the generated CLI and MCP reference is current.
- Verified the generated Python and TypeScript UniFFI bindings are current.
- Built the complete public documentation site and checked internal links and
  public-document hygiene.
- Rebuilt and staged the exact local SDK library before running package tests,
  so the package tests exercised the current ABI rather than a prior binary.
- Verification:
  - `cargo test -p cua-driver`: passed, including 136 binary unit tests and all
    non-ignored integration tests.
  - TypeScript typecheck and package suite: 6 passed.
  - Python package suite: 28 passed and 3 optional executable checks skipped.
  - Documentation production build: passed with 93 static pages.
  - Documentation link check: 0 errors.
  - Documentation hygiene check: passed.
  - `cargo fmt --all -- --check`: passed.
  - Focused Clippy checks cover the changed core and cursor crates. A strict
    all-platform, all-target workspace invocation also reports unrelated
    existing warnings in host-platform stubs; platform CI remains the
    authoritative cross-target lint and build check.

## Evidence index

Evidence links and exact-head workflow runs will be added as each environment
completes.
