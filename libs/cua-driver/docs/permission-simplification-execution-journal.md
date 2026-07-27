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

## Evidence index

Evidence links and exact-head workflow runs will be added as each environment
completes.
