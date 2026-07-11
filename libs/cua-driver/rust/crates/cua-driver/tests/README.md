# cua-driver Rust integration tests

Tests in this directory exercise the public driver interface. Headless protocol
tests run by default; GUI and modality tests are marked `#[ignore]` because they
need staged harness apps and an interactive desktop.

Start with the contributor overview in
`../../../../docs/test-harnesses-guide.md`, then use the matrix below as the
coverage map.

The cross-OS ownership map is maintained in
`../../../../docs/test-matrix.md`. Update that matrix when adding a harness,
action, addressing mode, delivery mode, or OS-specific window-system case.

## Naming

| Prefix | Runs by default | Purpose |
| --- | --- | --- |
| `protocol_*_test.rs` | yes | MCP/CLI protocol and schema behavior |
| `schema_*_test.rs` | yes | Generated schema consistency |
| `harness_<toolkit>_test.rs` | no, `#[ignore]` | Toolkit-specific harness apps |
| `desktop_scope_<os>_test.rs` | no, `#[ignore]` | Platform window/desktop scope contract |

## Harness Requirements

Build harness apps before running ignored tests:

```bash
# macOS
../../../../tests/fixtures/build/macos.sh

# Linux
../../../../tests/fixtures/build/linux.sh
```

```powershell
# Windows
..\..\..\..\tests\fixtures\build\windows.ps1
```

Staged outputs are read from `../../test-apps/harness-<name>/`.

The canonical cross-platform matrix is
`cross_platform_behavior_test.rs`. It runs the same external-state scenarios
against Electron and Tauri on each supported host. CI and VM runners set
`CUA_TEST_DRIVER_BIN`, `CUA_TEST_APPS_ROOT`, and
`CUA_TEST_WORKSPACE_ROOT` when artifacts are built outside Cargo's default
workspace paths. They also set `CUA_TEST_REQUIRE_FIXTURES=1`, turning a missing
fixture into a failure instead of a silent skip.

Each matrix row declares its action, AX/PX targeting, foreground or background
delivery, scope, driver route, external oracles, and required behavior in
`cases.jsonl`. `results.jsonl` records the observed behavior and derived test
status. The Rust reporter validates both files and renders `summary.md`.

The canonical OS runners also set
`CUA_E2E_RECORDINGS_ROOT`. Every testkit `McpDriver` then records its full
desktop trajectory to a unique directory containing `recording.mp4`, cursor
samples, action JSON, per-turn screenshots, and a `trajectory.json` test-label
manifest. Windows and Linux require
FFmpeg; macOS uses the installed driver's ScreenCaptureKit backend. The runner
validates each MP4 with `ffprobe` before reporting success. A separate Rust
preflight verifies the desktop, fixture, AX tree, screenshot, and video
lifecycle once before behavioral cells run.

## Running

```bash
cargo test -p cua-driver --test protocol_handshake_test
cargo test -p cua-driver --test harness_appkit_test -- --ignored --nocapture
cargo test -p cua-driver --test desktop_scope_macos_test -- --ignored --nocapture
```

The canonical Windows E2E entrypoint is
`scripts/ci/windows/run-rust-e2e.ps1 -RequireGui`.
It runs the complete Rust harness matrix; internal lane selectors are retained
only for focused diagnosis. Optional
external-app suites remain separate.

Legacy Windows Sandbox runs use
`../../../../tests/runners/windows-sandbox/run-tests-in-sandbox.ps1`, which
builds selected Windows harness apps and maps them into the sandbox. The current
Windows GUI validation path should use a real user desktop session through RDP
or an interactive scheduled task.

Windows GUI tests require a usable interactive desktop. SSH-launched commands start in Session 0 and
cannot drive the user's desktop directly; launch GUI tests through an
interactive scheduled task (`/IT`) or equivalent so they run in the logged-on
user session.

The testkit's native `DesktopObserver` records foreground-window, z-order,
cursor, and leaked-input state around rows that promise no desktop side effects.
If the input desktop is not `Default` or cannot be opened, the session is
usually locked or disconnected. Reconnect, use `tscon /dest:console` on a
disposable GUI VM, or boot the VM into an unlocked console session before
running ignored GUI tests.

Set `CUA_REQUIRE_GUI=1` on dedicated GUI runners to turn these desktop
self-skips into hard failures with the full desktop-state diagnostic.

The repository-level runners are the preferred entrypoints for the canonical
matrix:

```bash
scripts/ci/linux/run-rust-e2e.sh
```

```powershell
.\scripts\ci\windows\run-rust-e2e.ps1 -RequireGui
```

## Optional External Apps

These suites are Rust source-of-truth coverage for real installed apps, but they
are not part of the canonical run-all path because they depend on software or
desktop state outside the repo-local fixtures:

- `harness_libreoffice_test.rs`: Windows LibreOffice Writer/Calc. Requires
  LibreOffice installed, or `LO_SWRITER_EXE` / `LO_SCALC_EXE` pointing at the
  executables.
- `installed_app_launch_macos_test.rs`: macOS Calculator/TextEdit launch focus
  checks. Requires a logged-in GUI session and usable System Events scripting.
- `installed_app_textedit_macos_test.rs`: real TextEdit background AX write and
  verification. Requires a logged-in GUI session and Accessibility permission.
