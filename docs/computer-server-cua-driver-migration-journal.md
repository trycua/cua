# Computer Server to Cua Driver migration journal

## Objective

Audit every maintained repository surface that depends on
`cua-computer-server`, implement two opposing migration proofs of concept, and
use measured capability and compatibility evidence to recommend a product
direction.

The proofs of concept are:

1. preserve the computer-server remote compatibility boundary while delegating
   desktop automation to the Rust-backed Python `cua-driver` SDK; and
2. run the XFCE image with a local Cua Driver daemon and Python SDK consumer,
   without computer-server.

## Source preflight

- Worktree: repository worktree selected by the user
- Branch: `codex/computer-server-driver-migration-audit`
- Starting commit: `b8a0f32a06c75225ba24ebb5ab14f6507fa90d15`
- Upstream: `origin/main` at the same commit
- Starting status: clean
- Predecessor: PR #2341 merged at the starting commit after all checks passed

## Evidence log

### 2026-07-21 — initial inventory

- The computer-server implementation contains roughly 14,000 lines of Python.
- Its command registry mixes desktop automation with accessibility, shell,
  files, clipboard, window management, wallpaper, diorama, and version calls.
- Its network surface additionally owns `/status`, `/commands`, `/cmd`, `/ws`,
  PTY HTTP/WebSocket routes, `/responses`, `/playwright_exec`, and `/mcp`.
- Maintained consumers include Python and TypeScript computer clients, the
  sandbox SDK, Python and TypeScript CLIs, MCP/session wrappers, agent UI paths,
  Cua Bench, Fleet examples, and VM/container provisioning.
- Provisioned server environments include XFCE, Kasm, QEMU Linux/Windows/
  Android, Lume guests, Windows Sandbox, and WinArena images.
- The canonical Cua Driver contract exposes fourteen typed session/desktop
  operations. The SDK also exposes `call_tool` for the complete live Rust
  registry, but it deliberately speaks only to a local daemon socket/pipe.
- Therefore the SDK can replace desktop implementation code in-process, but it
  does not replace computer-server's authenticated remote transport or its
  non-GUI control-plane features.

### 2026-07-21 — delivery constraints

- XFCE images build for both `linux/amd64` and `linux/arm64`; Cua Driver ships
  Rust archives and Python wheel tags for both architectures.
- The current environment has no Docker, Podman, Buildah, or Hadolint binary.
  Container proofs can receive static, unit, packaging, and script validation
  here, but an actual image build must run in CI or on a Docker host.

### 2026-07-21 — POC A implementation and validation

- Added an opt-in `CUA_BACKEND=cua-driver` automation adapter and
  `cua-computer-server[driver]` package extra. Default native/VNC behavior is
  unchanged.
- The adapter uses the SDK's documented generic escape hatch for an
  application-owned server bridge, declares a desktop session lazily, translates
  portable GUI calls, and keeps unsupported operations on the native handler.
- Focused adapter/CLI suite: 7 passed.
- Complete computer-server suite: 60 passed with five pre-existing dependency
  deprecation warnings.
- Ruff and Black passed for changed Python files.
- The package built as sdist and wheel. The first metadata assertion in the
  verification script expected single quotes while wheel metadata correctly
  used double quotes; correcting the test expectation produced a passing check
  for the `driver` extra and version constraint. This was a test-harness issue,
  not a package defect.
- `uv lock --check` passed. An initial local `uv` invocation wanted to normalize
  many unrelated marker lines because the installed uv version differs from the
  lockfile's producer. Those unrelated mechanical edits were reversed, leaving
  only the new package/extra entries.
- Importing the local generated UniFFI package and constructing `CuaDriver`
  succeeded. The default daemon reported unavailable, so no live GUI action was
  claimed.

### 2026-07-21 — POC B implementation and validation

- Added a default-preserving XFCE build selector between computer-server and a
  direct Cua Driver daemon.
- Added an in-container verifier that uses generated typed Python SDK records,
  exercises session start, screen geometry, desktop capture, a harmless cursor
  input round-trip, and session end.
- Shell syntax and Python compilation passed; the verifier passed Black/Ruff.
- The launcher explicitly selects the daemon's `standard` authorization mode;
  the removed `--no-permissions-gate` flag only applies to macOS and had no
  meaning in this Linux image.
- PyPI metadata confirmed `cua-driver` 0.10.0 wheels for both XFCE target
  architectures (`manylinux_2_31_x86_64` and `manylinux_2_31_aarch64`).
- Container runtime validation remains open because this workstation has no
  supported image builder. The exact required command and expected verifier
  output are documented in `libs/xfce/Development.md`.

### 2026-07-21 — decision

- Use the driver-backed compatibility server as the near-term seam. It removes
  duplicated GUI implementation risk without breaking remote SDKs and images.
- Direct driver-only images are appropriate for co-located applications and
  agents, but are not a general replacement while host clients require a remote
  authenticated guest control plane.
- The long-term split is: generated driver SDK for client applications,
  `cua-driver mcp`/CLI for agents, and a small separately named guest gateway for
  remote health/auth/shell/files/PTY plus driver delegation.

### 2026-07-21 — independent architecture review

- The requested Claude Code Fable review could not start inference because its
  provider-side usage allowance was exhausted. Claude Code Opus completed the
  same read-only review as the available fallback.
- The review found no blocking correctness or security issue and agreed that
  POC A is the defensible migration seam while POC B cannot replace the remote
  guest boundary.
- Review findings were resolved by making the verifier executable in the
  image, exercising an actual input call, allowing a single-key hotkey, bounding
  drag duration and steps, correcting Linux authorization wording, and adding
  telemetry/middleware, the shared `/mcp` handler path, and the unchanged
  development Dockerfile to the audit.

## Open verification items

- Build and boot the direct-driver XFCE image on x86_64 and arm64 Docker hosts,
  then run `/usr/local/bin/verify-cua-driver-sdk.py` inside each image.
- Run the compatibility backend conformance suite on Linux X11, Windows, and
  macOS with a live daemon and compare it to the native backend.
- Resolve the session-per-remote-caller and shutdown cleanup design before
  promoting the adapter from POC to default.
