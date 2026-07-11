# Rust E2E CI Reporting

Rust owns behavioral case declarations, observations, result classification,
and report validation. OS runners build the environment, execute Rust targets,
and upload the validated artifacts.

## Canonical Invocation

The contributor-facing invocation runs the complete matrix on every OS and
takes no suite selector:

```text
Windows: scripts/ci/windows/run-rust-e2e.ps1 -RequireGui
Linux:   scripts/ci/linux/run-rust-e2e.sh
macOS:   scripts/ci/macos/run-rust-e2e.sh
```

Workflows set a private lane value to fan that matrix into shared, native, and
capture jobs so one failure does not hide another lane. These are execution
partitions, not public suite choices or separate behavioral sources of truth.

## Execution Order

Each runner follows the same sequence:

1. Build the Rust driver and required repo-local fixtures.
2. Run `e2e_environment_preflight_test` once.
3. Abort before behavioral cells if the desktop, permissions, fixture, capture,
   or video lifecycle is unavailable.
4. Run the selected Rust integration targets.
5. Validate declarations, results, and evidence with `cua-e2e-report`.
6. Upload the report, logs, and recordings.

The preflight prevents a single TCC or desktop-session problem from appearing
as the same failure on every behavioral cell.

## Artifact Files

Each OS lane writes:

```text
artifacts/cua-driver/<platform>/
|-- environment.jsonl
|-- cases.jsonl
|-- results.jsonl
|-- summary.md
|-- environment-preflight.log
|-- <rust-target>.log
`-- recordings/<cell-label>-pid<process>-<sequence>/
    |-- recording.mp4
    |-- trajectory.json
    |-- session.json
    |-- cursor.jsonl
    `-- turn-*/
        |-- action.json
        `-- screenshot.png
```

`cases.jsonl` is the executed catalog. `results.jsonl` contains one result for
every declared cell. The reporter rejects duplicates, missing results,
undeclared results, contradictory statuses, and missing required videos.

## Environment Record

The preflight emits exactly one record:

```json
{
  "schema": "cua-e2e-environment/v2",
  "platform": "macos",
  "display_server": "quartz",
  "status": "ready",
  "duration_ms": 912,
  "message": ""
}
```

An error record produces an environment section in `summary.md` and aborts the
lane before case declarations are executed.

## Case And Result Contract

A case declaration uses `cua-e2e-case/v2`. A result flattens the same contract
fields into `cua-e2e-result/v2` and adds the observation:

```json
{
  "schema": "cua-e2e-result/v2",
  "cell_id": "windows-tauri-left-click-ax-background",
  "platform": "windows",
  "display_server": "win32",
  "harness": "tauri",
  "toolkit": "platform-webview",
  "action": "left_click",
  "targeting": "ax",
  "delivery": "background",
  "scope": "window",
  "driver_route": "uia_invoke",
  "expected_behavior": { "kind": "deliver" },
  "oracles": ["fixture_state", "focus", "z_order", "no_leaked_input", "cursor"],
  "test_status": "pass",
  "observed_behavior": "delivered",
  "passed_oracles": ["fixture_state", "focus", "z_order", "no_leaked_input", "cursor"],
  "duration_ms": 1482,
  "message": "",
  "evidence": {
    "video": "recordings/windows-tauri-left-click-ax-background-pid123-001/recording.mp4",
    "trajectory": "recordings/windows-tauri-left-click-ax-background-pid123-001/trajectory.json"
  }
}
```

`targeting` describes how the action identifies its target: `ax`, `px`,
`page`, or `not_applicable`. It is separate from screenshot/tree capture.

## Classification Rules

`test_status` answers whether the cell met its declared contract.
`observed_behavior` records what the driver did.

| Expected | Observed | Required oracles | Status |
| --- | --- | --- | --- |
| Deliver | Delivered | Passed | Pass |
| Deliver | Refused | Any | Fail |
| Deliver | No effect or error | Any | Fail |
| Refuse | Allowed structured refusal | Passed | Pass |
| Refuse | Different refusal code | Any | Fail |
| Refuse | Delivered | Any | Fail pending contract review |

The structured refusal enum currently recognizes
`background_unavailable`, `background_occluded`, and
`background_uipi_blocked`. Each refusal case declares the exact subset allowed
by its controlled setup. String-prefix or message-substring matching is not
accepted.

A refusal contract also requires focus, z-order, and leaked-input observations.
An honest refusal does not satisfy a case whose contract requires delivery.

## Summary Ownership

`cua-e2e-report` is the only behavioral Markdown renderer. Shell and
PowerShell runners must not parse `test ... ok` output or create `host=cargo`
and `host=lane` rows. Cargo logs remain available for unit-test annotations and
lane diagnostics, outside the behavioral population.

The summary reports delivered passes, refused passes, failures, and skips
separately. Shared and native targets emit the same typed records; a compile or
runner failure that prevents declaration remains visible as a failed job and
cannot be mistaken for a green behavioral row.

## Evidence Links

Every canonical behavioral cell records a validated MP4 and trajectory. The
recording directory also contains its session metadata and turn-level action
and screenshot files; Cargo target logs remain lane-level diagnostics. GitHub
cannot deep-link to a file inside a multi-cell artifact archive, so each summary
row displays the exact MP4 path as a link to the owning lane archive while the
trajectory rollup provides bulk-download links and video counts.
