# E2E Results Summary Design

**Historical design record.** The implemented reporting contract is documented
in `e2e-ci-reporting.md`.

This document specifies the GitHub Actions presentation built from the Rust v2
case and result contract. Field semantics live in `e2e-ci-reporting.md`; the
convergence sequence lives in `test-harness-convergence-plan.md`.

## Goals

The typed artifacts and run summary together must answer:

- Was the desktop environment ready?
- Which declared behavioral cells delivered, refused, failed, or did not run?
- Which harness, action, targeting mode, delivery mode, and driver route did
  each cell cover?
- Which external and desktop oracles passed?
- Which lane archive owns that cell's recording evidence?
- Are any declared cells missing or duplicated?

Cargo test names and lane exit codes do not answer these questions and are not
behavioral rows.

## Summary Layout

The generated Markdown starts with aggregate behavioral totals, followed by a
declared-coverage grid and the detailed rows:

```markdown
# CUA Driver E2E

**Result:** 42 delivered, 3 refused, 2 failed, 0 skipped

**Source SHA:** `0123456789abcdef0123456789abcdef01234567`

## Declared Coverage

## Detailed Results
```

Environment readiness and source identity live in `environment.jsonl`, the
generated summary, and the workflow metadata. An environment failure prevents
behavioral cells and makes the reporter fail; it is not converted into a
partial green summary.

## Behavioral Table

One row represents one declared `cell_id`:

```markdown
| Cell | Harness | Action | Targeting | Delivery | Route | Expected | Observed | Oracles | Status | Time | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | ---: | --- |
| linux-electron-left-click-ax-background | Electron | Left click | AX | Background | AT-SPI action | Refuse | Refused | Focus, z-order, no leak | PASS | 1.5s | [recordings/.../recording.mp4] |
| linux-electron-left-click-ax-foreground | Electron | Left click | AX | Foreground | AT-SPI action | Deliver | Delivered | Fixture | PASS | 2.1s | [recordings/.../recording.mp4] |
```

The row does not label a refusal as pass unless the case declaration expects
refusal and every required no-side-effect oracle passed.

## Coverage Table

The reporter also renders declared coverage. A dash means that the Rust catalog
does not declare that combination; it is not a pass or an inferred result:

```markdown
| Harness | Action | AX/BG | AX/FG | PX/BG | PX/FG | Page | NotApplicable |
| --- | --- | --- | --- | --- | --- | --- | --- |
| electron | left_click | REFUSED | PASS | REFUSED | PASS | - | - |
| electron | drag | - | - | REFUSED | PASS | - | - |
```

The detailed table remains the authority for route, contract, oracle, and
failure information for every declared cell.

## Validation

The reporter fails before publishing a green summary when it finds:

- duplicate declarations or results;
- a declared cell with no result;
- a result with no declaration;
- a serialized status that contradicts expected and observed behavior;
- an unknown refusal code;
- a passing cell missing a required oracle;
- unfinalized, missing, or empty required video evidence;
- a behavioral video phase that never reached `finalized` after setup/posture;
- an invalid hosted-runner console cleanup status;
- more or fewer than one environment record.

The reporter validates the exact video it renders. It records the trajectory
path but does not currently validate that file independently. Workflow summary
jobs may concatenate validated platform summaries, but they do not recalculate
results.

## Evidence Packaging

Each internal workflow lane uploads one archive. Every cell owns a stable
subdirectory derived from `cell_id` inside that archive:

```text
recordings/<cell-label>-pid<pid>-<sequence>/
|-- recording.mp4
|-- trajectory.json
|-- session.json
|-- cursor.jsonl
`-- turn-*/
    |-- action.json
    `-- screenshot.png
```

GitHub artifact archives do not provide stable URLs to individual files. The
row therefore links its exact MP4 path text to the owning lane archive; the
trajectory rollup links the same archive and reports its video count. A future
static report may provide inline playback; it is not required for the GitHub
summary. Cargo target logs sit at the lane root and diagnose runner or test
failures; they are not invented as per-cell evidence.

## Unit And Protocol Results

Unit, schema, transport, and CLI tests remain separate workflow output. They do
not share the behavioral case schema, appear as invented behavioral rows, or
require desktop video.
