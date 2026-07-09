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

## Running

```bash
cargo test -p cua-driver --test protocol_mcp_test
cargo test -p cua-driver --test harness_appkit_test -- --ignored --nocapture
cargo test -p cua-driver --test modality_desktop_scope_macos_test -- --ignored --nocapture
```

Legacy Windows Sandbox runs use
`../../../../tests/runners/windows-sandbox/run-tests-in-sandbox.ps1`, which
builds selected Windows harness apps and maps them into the sandbox. The current
Windows GUI validation path should use a real user desktop session through RDP
or an interactive scheduled task.

Windows GUI modality tests require a foreground interactive desktop where
`GetForegroundWindow` returns a real user window. SSH-launched commands,
scheduled tasks, and PsExec-launched commands can enter the right user session
while still exposing no foreground desktop; those tests self-skip in that state
because the focus oracle would otherwise report meaningless pid-0 results.
