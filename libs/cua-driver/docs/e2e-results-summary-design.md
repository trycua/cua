# E2E Results Summary Design

This document specifies the GitHub Actions presentation built from the Rust v2
case and result contract. Field semantics live in `e2e-ci-reporting.md`; the
convergence sequence lives in `test-harness-convergence-plan.md`.

## Goals

The run summary must answer:

- Was the desktop environment ready?
- Which declared behavioral cells delivered, refused, failed, or did not run?
- Which harness, action, targeting mode, delivery mode, and driver route did
  each cell cover?
- Which external and desktop oracles passed?
- Where is that cell's video and trajectory evidence?
- Are any declared cells missing or duplicated?

Cargo test names and lane exit codes do not answer these questions and are not
behavioral rows.

## Summary Layout

The generated Markdown starts with the environment and rollup:

```markdown
# CUA Driver E2E: Windows

**Environment:** Ready, `windows-latest`, Win32 interactive desktop
**Source:** `0e9f1a48`, Rust `0.7.1`
**Result:** 42 delivered, 3 refused, 2 failed, 0 skipped

| Harness | Delivered | Refused | Failed | Skipped | Total |
| --- | ---: | ---: | ---: | ---: | ---: |
| Electron | 18 | 1 | 0 | 0 | 19 |
| Tauri | 17 | 2 | 0 | 0 | 19 |
| WPF | 7 | 0 | 2 | 0 | 9 |
```

An environment failure replaces the behavioral table with one error section.
Behavioral cells do not run in that state.

## Behavioral Table

One row represents one declared `cell_id`:

```markdown
| Cell | Harness | Action | Targeting | Delivery | Route | Expected | Observed | Oracles | Status | Time | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | ---: | --- |
| windows-electron-click-ax-bg | Electron | Left click | AX | Background | UIA Invoke | Deliver | Delivered | Fixture, focus, z-order | PASS | 1.5s | [Cell artifact] |
| windows-tauri-hotkey-ax-bg | Tauri | Hotkey | AX | Background | PostMessage | Deliver | Refused | Focus, z-order, no leak | FAIL | 2.1s | [Cell artifact] |
```

The row does not label a refusal as pass unless the case declaration expects
refusal and every required no-side-effect oracle passed.

## Coverage Table

The reporter also renders declared coverage, including omitted combinations:

```markdown
| Action | AX/BG | AX/FG | PX/BG | PX/FG |
| --- | --- | --- | --- | --- |
| Left click | PASS | PASS | PASS | PASS |
| Right click | PASS | Equivalent: PX/BG | PASS | PASS |
| Drag | Unsupported | Unsupported | REFUSED | PASS |
```

An omitted combination must name an equivalent cell or unsupported contract
reason in the Rust catalog. Missing output is never displayed as `N/A`.

## Validation

The reporter fails before publishing a green summary when it finds:

- duplicate declarations or results;
- a declared cell with no result;
- a result with no declaration;
- a serialized status that contradicts expected and observed behavior;
- an unknown refusal code;
- a passing cell missing a required oracle;
- missing or empty required evidence;
- more or fewer than one environment record.

The reporter validates the exact files it renders. Workflow summary jobs may
concatenate validated platform summaries, but they do not recalculate results.

## Evidence Packaging

Each cell uses a stable artifact name derived from `cell_id`. Its bundle
contains:

```text
recording.mp4
trajectory.json
action.json
turn-*/screenshot.png
cell.log
```

The GitHub row links to the cell artifact download. GitHub artifact archives do
not provide stable URLs to individual files, so the displayed row also lists
the exact internal paths. A future static report may provide inline playback;
it is not required for the initial GitHub summary.

## Unit And Protocol Results

Unit, schema, transport, and CLI tests appear after the E2E section as suite
rollups or JUnit annotations. They never share the behavioral case schema and
do not require desktop video.
