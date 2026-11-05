# Bench v2 pack tasks of the 42-task set

GUI-only tasks written from scratch on 11 Oct 2026, one directory each: `MA-*` multi-app, `WF-*` web apps
and forms, `NA-*` native macOS apps, `DS-*` documents and spreadsheets, `PR-*` precision, `IR-*` interruptions.
`task.v2.json` gives each task's id, category and apps; `PROVENANCE.md` states that it is original. Together with the
ten core tasks and any held-out tasks (kept out of this repository) they make the 42-task set listed in [`../../v2-42-taskset.json`](../../v2-42-taskset.json).

Each task follows the launch-descriptor contract (`platform/launch.macos.json`):

* `reset/setup.py` builds the workspace, `reset/verify.py` checks it; native-app tasks pass `--apply-app-state`, so the
  reset also sets the app's own data (Calendar, Reminders, Notes) through JavaScript for Automation;
* an optional `semantics.export` step reads app data the evaluator needs into the workspace after the agent;
* `evaluator/evaluate.py` writes `passed` (every required check), a partial `score` and named `checks`. It reads
  only final state (the saved document, the local web app's saved record and event log, the exported app data,
  file metadata), never the screen. An untouched start fails with score 0.

Every task is self-contained: its brief, fixture, app, reset, descriptor, evaluator, tests and scripted solutions
live in its own directory. The only shared code is [`_kit/`](_kit) (local web server, ODF/RTF/image readers, Finder
metadata, JXA app-data helpers, dev tools), which holds no task content. A task can therefore be moved to a separate
held-out pack with a copy of `_kit/` and run unchanged (see `automated-eval/macos_bench/tools/taskset.py split`).

Agent-visible subset in the macOS VM: everything except `evaluator/` and `tests/`.

## OS availability

Every task here has a macOS descriptor today and runs in a macOS VM. Tasks that use only LibreOffice, or only Chrome
and a local web app, or both, are cross-OS candidates: a Linux or Windows descriptor needs no change to the task.
Tasks on macOS's own apps (Calendar, Reminders, Notes, Finder, Preview, TextEdit) are macOS-only. The manifest's
`os` and `cross_os_candidate` fields, and the table in the bench README, give this per task.

## Tests

```bash
python3 run_tests.py            # every task's checker tests, one process per task
python3 run_tests.py --e2e      # also replays each web task's reference solution in a headless Chrome
```

In a macOS VM, `_kit/devtools/vm_validate.py` runs every task's reset, export and evaluator without a model, and
applies each native-app task's scripted solution (`tests/vm_solve.py`) straight to the app's data to check that the
evaluator passes a solved state.

## Licence

FSL-1.1-MIT, under the benchmark's [LICENSE](../../../LICENSE). Copyright (c) 2026 Cua AI, Inc.
