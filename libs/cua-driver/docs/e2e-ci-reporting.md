# Rust E2E CI Reporting

The Rust desktop harness is the behavioral source of truth. GitHub Actions only
selects the lane and publishes its results.

## Workflow UI

The Linux and Windows manual workflows split the expensive suites into
independent jobs. The workflow summary job collects their artifacts and writes
one Markdown table to the GitHub Actions run summary.

This means a failure in the shared Electron/Tauri lane does not prevent the
native or modality lane from running. The workflow still fails when a required
lane fails.

## Result files

When `CUA_E2E_RESULTS_FILE` is set, Rust shared behavior tests append one JSON
object per host and scenario:

```json
{
  "schema": "cua-e2e-result/v1",
  "platform": "windows",
  "host": "tauri",
  "scenario": "keyboard",
  "status": "FAIL",
  "message": "application state did not reach key_state=enter",
  "duration_ms": 24360
}
```

When `CUA_E2E_SUMMARY_FILE` is set, the same test writes a Markdown row for the
GitHub summary. Native and modality scripts add lane-level rows to the same
files and retain their full logs alongside them.

Statuses are deliberately explicit:

- `PASS`: the external application-state oracle passed.
- `FAIL`: the driver response or external oracle failed.
- `SKIP`: the fixture was unavailable and was not required by the invocation.

The CI scripts set `CUA_TEST_REQUIRE_FIXTURES=1`, so missing required fixtures
are reported as failures rather than silently becoming skips.

## Running locally

Windows:

```powershell
.\scripts\ci\windows\run-rust-e2e.ps1 -Suite shared -RequireGui
.\scripts\ci\windows\run-rust-e2e.ps1 -Suite native -RequireGui
```

Linux:

```bash
xvfb-run -a --server-args="-screen 0 1920x1080x24" \
  dbus-run-session -- bash -lc \
  "scripts/ci/linux/run-rust-e2e.sh --suite shared"
```

The generated files are under `artifacts/cua-driver/<platform>/` and should
not be committed.
