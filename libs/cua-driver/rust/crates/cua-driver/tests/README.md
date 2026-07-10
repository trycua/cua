# cua-driver Rust integration tests

Tests in this directory exercise the public driver interface. Headless protocol
tests run by default; GUI and modality tests are marked `#[ignore]` because they
need staged harness apps and an interactive desktop.

## Naming

| Prefix | Runs by default | Purpose |
| --- | --- | --- |
| `protocol_*_test.rs` | yes | MCP/CLI protocol and schema behavior |
| `schema_*_test.rs` | yes | Generated schema consistency |
| `harness_<toolkit>_test.rs` | no, `#[ignore]` | Toolkit-specific harness apps |
| `modality_<area>[_<os>]_test.rs` | no, `#[ignore]` | Background input, capture, desktop scope |
| `guard_*_test.rs` | usually ignored or self-skipping | UX guard and interactive desktop checks |

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

## Running

```bash
cargo test -p cua-driver --test protocol_handshake_test
cargo test -p cua-driver --test harness_appkit_test -- --ignored --nocapture
cargo test -p cua-driver --test modality_desktop_scope_macos_test -- --ignored --nocapture
```

Windows Rust run-all uses
`../../../../tests/runners/windows/run-all.ps1`. It builds repo-local fixtures
and runs the default, guard, harness, and modality suites. It intentionally
excludes optional external-app suites.

Legacy Windows Sandbox runs use
`../../../../tests/runners/windows-sandbox/run-tests-in-sandbox.ps1`, which
builds selected Windows harness apps and maps them into the sandbox. The current
Windows GUI validation path should use a real user desktop session through RDP
or an interactive scheduled task.

Windows GUI modality tests require a user desktop where the focus sentinel can
become the foreground window. SSH-launched commands start in Session 0 and
cannot drive the user's desktop directly; launch GUI tests through an
interactive scheduled task (`/IT`) or equivalent so they run in the logged-on
user session.

The Windows probe distinguishes two no-foreground states:

- `input_desktop=Default, foreground_hwnd=0`: the desktop is usable but idle.
  Tests now launch `focus-monitor-win` and require that sentinel HWND to become
  foreground before assertions start.
- `input_desktop` is not `Default` or cannot be opened: the session is usually
  locked/disconnected, for example after an RDP client drops. Reconnect, use
  `tscon /dest:console` on a disposable GUI VM, or boot the VM into an unlocked
  console session before running ignored GUI tests.

Set `CUA_REQUIRE_GUI=1` on dedicated GUI runners to turn these desktop
self-skips into hard failures with the full desktop-state diagnostic.

The repository-level runners are the preferred entrypoints for the canonical
matrix:

```bash
scripts/ci/linux/run-rust-e2e.sh --suite shared
```

```powershell
.\scripts\ci\windows\run-rust-e2e.ps1 -Suite shared -RequireGui
```

## Optional External Apps

These suites are Rust source-of-truth coverage for real installed apps, but they
are not part of the canonical run-all path because they depend on software or
desktop state outside the repo-local fixtures:

- `harness_libreoffice_test.rs`: Windows LibreOffice Writer/Calc. Requires
  LibreOffice installed, or `LO_SWRITER_EXE` / `LO_SCALC_EXE` pointing at the
  executables.
- `modality_launch_focus_macos_test.rs`: macOS Calculator/TextEdit launch focus
  checks. Requires a logged-in GUI session and usable System Events scripting.
