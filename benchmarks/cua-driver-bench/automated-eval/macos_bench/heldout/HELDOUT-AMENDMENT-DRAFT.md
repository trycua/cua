# Held-out amendment (DRAFT, not in force): a sealed held-out task set

**Status: draft for the owner's approval. Nothing here is in force, and no trial has run on these tasks.** When
the owner approves it, the text moves into `PREREGISTRATION.md` under the next free amendment number (written here as
`AH`), unchanged except for the values marked *to fill*, and it is committed before the first held-out trial.

The tasks themselves are not in this repository. `SEAL.json` (next to this file) holds their digests (private revision `268874ac5dd2`), committed
before any run, so anyone can later check that the tasks run are the tasks sealed here.

## AH.1 Why

Every Cua Driver fix since 7 Oct (v035 to v038) was diagnosed from trials of the same ten tasks it was then
measured on. The v038 result is therefore partly a measure of tuning to those tasks. A public claim needs tasks
nobody saw while tuning. This amendment seals twelve new tasks before anyone tuning the driver has seen them, and
fixes now how they will be run and read.

## AH.2 The tasks (sealed)

Twelve tasks, `HO-01` to `HO-12`, written on 9 Oct 2026 by someone not tuning the driver. None was used, run or
shown during v035 to v038 tuning. They cover:

| Kind | Tasks |
|---|---|
| Native AppKit apps (Notes, Reminders, Finder, Preview, System Settings, TextEdit, Contacts) | 7 |
| An Electron app (Obsidian, free) | 1 |
| Chrome web forms (local pages: a multi-step form with a file dialog and a JavaScript confirm, and an editable table) | 2 |
| A spreadsheet and a canvas app (LibreOffice Calc and Draw) | 2 |

Across them: drag and drop (Finder, Preview thumbnails, Draw), multi-window and dialog flows (open and save
panels, a new folder from a save panel, confirm dialogs), and two long chains of 25 or more GUI steps. LibreOffice
and Chrome were used in tuning; these tasks use them for different work. Everything is free or open source; no
asset with a restrictive licence, nothing derived from Amazon or Unity material.

Each task has, as the CDB tasks do: a brief, a reset with a verification of the reset state, a launch
descriptor, a deterministic state-based evaluator with `passed` (every check) and a partial `score`, and a
reference solution. The partial score is the share of the task's progress checks that pass, halved when a
preservation check fails (something the brief said to leave alone was changed); an untouched start scores 0.

Every checker was validated in a Lume VM before this draft (AH.9): untouched it fails with score 0, partly solved
it fails with a score between 0 and 1, and solved it passes with score 1.

## AH.3 Hold-out rules

1. **Where the tasks live.** In a private location outside this repository and outside every repository used for
   driver work. They are installed only into the VM clone that runs them (AH.6), with the evaluator, its oracle
   and the reference solutions in the evaluator user's home, unreadable by the agent's user, as for the CDB pack.
2. **Who may see them.** Nobody who tunes Cua Driver, writes its skill, or reviews its fixes, and no agent stream
   doing that work, sees the briefs, fixtures, apps, checkers or trial streams before the first held-out run is
   reported. After the run, per-task results and failure classes may be reported; the briefs stay private so the
   set stays held out for the next release.
3. **No tuning on the set.** After a held-out run, a fix motivated by a held-out failure makes that task tuned.
   It is marked as such in every later report, and the set is retired for public claims once more than three of
   its tasks are marked.
4. **Frozen before any run.** The digests in `SEAL.json` are checked by the preflight (`pins.json`
   `heldout_pack`, *to fill*). Any change to a task after the seal, including a checker fix, is a new amendment
   with a new seal, written before the next run, and the earlier results of that task are reported separately.
5. **One look per build.** A held-out run happens once per driver build named for a public claim. Mini-runs on
   the held-out set are not allowed; hill-climbing stays on the ten-task set.

## AH.4 Arms

Held-out run `vHO1` on the build that the public claim is about (*to fill*: the main commit, pinned as in
A8.1). Arms:

| Label | Runner arm | Tool layer |
|---|---|---|
| AX | `cc-cua-driver-script` | Cua Driver main at the pinned commit, `run_script` on (as in A8.1) |
| A | `cc-cua-driver-main` | the same build, `run_script` off |
| B | `cc-codex-cu` | OpenAI's `cua_repl` launcher, unchanged (26.930 build, as in v038) |
| A0 | `cc-cua-driver` | Cua Driver 0.34.0, the pinned private copy |

Optional, if the owner wants them in the same run (each adds 60 trials): arc-driver 0.1.1 (`cc-arc-driver`, A9)
and the Claude Desktop computer-use helper (`cc-claude-cu-helper`, A11), each in its own VM clone as before.

Everything else is as in v038 (A8.2): Sonnet 5.5 in Claude Code 2.1.289, the shared system prompt, ToolSearch on,
GUI-only tool surface (no Bash, Edit or Write: `"coding_tools": false`), side-door flags, the peek scan, pointer
parking, reset before and after every trial, recorder and sentinel.

## AH.5 Limits, order, runs

* **360 s and 45 turns**, as for every task of the protocol. Two tasks are long chains (25 or more GUI steps);
  hitting a limit is a failed trial with its partial score kept, as everywhere else.
* Order `HO-01` to `HO-12`. Arms interleaved in every task block, first arm `(task position + run index) mod 4`
  over (AX, A, B, A0).
* Phase 1 is runs 1 to 3, phase 2 runs 4 and 5 when phase 1 is complete and the stop rule allows. Only complete
  blocks are analysed. 12 tasks x 4 arms x 5 runs = 240 trials.
* Seats and the stop rule as in A8.3.

## AH.6 VM

A clone of the post-v038 `cdb-h2h` (`cdb-heldout`), with Obsidian 1.14.4 (sha256 of the DMG
`dcf818dd20ee5d9dd3e782eee0c0c4c47cc225383b051c2fc9af7d5772f59f70`) added, Automation permission for Terminal
(the runner) to Notes, Reminders, Contacts, Finder and System Events, and the held-out pack. Nothing else
differs from `cdb-h2h`. The clone is not used for tuning rounds.

## AH.7 Hypotheses (registered before any trial)

Primary set: the twelve held-out tasks. Pairing: trials of the same task and run. Tests use the A5.4 bootstrap
(10 000 resamples of runs within task, seed *to fill*), in `tools/analyze_heldout.py` (*to fill*, written before
the first trial).

* **H-HO1 (the tuned lead carries over).** Task-macro success of the best Cua Driver arm of v038 (AX) minus B.
  Supported when the lower bound of the 95% interval is above 0.
* **H-HO2 (not worse than Codex).** Same difference. Supported when the lower bound is at or above -0.10.
* **H-HO3 (turns).** Geometric mean over tasks of the ratio of mean turns AX/B. Supported when the upper bound is at
  or below 1.2 (the v038 done rule's turn limit).
* **H-HO4 (the 0.35 changes generalise).** Task-macro success A minus A0. Supported when the lower bound is at or
  above -0.15 (the A5.4 H3 margin); tokens A/A0 and turns A/A0 reported with intervals as in H1 and H2 of A5.4.
* **H-HO5 (overfitting check, descriptive).** The gap (AX minus B) on the held-out set against the same gap on the
  v038 set, both task-macro, with intervals. A gap that shrinks by more than half is reported as a sign that the
  v038 lead came from tuning.

Partial score, wall time, tokens and cost are reported per task and arm, not tested. No hover-only task is in the
set, so the MB-10/MB-11 caveat does not apply.

## AH.8 Reporting

* Public claims about Cua Driver against other tool layers cite the held-out result first, the v038 result
  second, and say which is which.
* Every report names the arms as in AH.4 (arm B is OpenAI's launcher driven by Claude, A10.1).
* Per-task results use the opaque ids and the one-line task kinds of AH.2, not the briefs.

## AH.9 Harness changes and validation

* `cdb_adapter.py`: an optional `semantics.export` command in a task's launch descriptor, run as the bench user
  after the agent and before the evaluator. It writes app state that only the bench user can read (Notes,
  Reminders, Contacts, system settings) into the workspace for the evaluator, which runs as `cdbeval`. Tasks
  without the field are unchanged (unit test in `tests/test_cdb_adapter.py`).
* Held-out task specs are installed into `probes/HO-*` in the VM only. They carry `"separate_run": true`, so the
  default schedule never picks them up.
* Validation (no model, no token), 9 Oct 2026, in `cdb-heldout`, through `cdb_adapter.CdbTask` with the export
  step: for each task, reset and verify, start the apps, evaluate untouched, apply a partial reference solution
  and evaluate, reset, apply the full reference solution and evaluate. All 12 tasks: untouched failed with score
  0, partial failed with a score between 0.14 and 0.75, solved passed with score 1; every export exited 0. The
  reference solutions write the end state through the apps' scripting interfaces, files, PDFKit or the local
  apps' HTTP endpoints, not through the GUI. Solved LibreOffice files were also re-saved by LibreOffice
  (headless) and still passed. A GUI pass on the checks that depend on what an app writes when driven by hand
  (System Settings keys, Preview's saved rotation, Finder's Compress) is open, and is done before the first run.
* VM preparation found and fixed before the seal: first-run welcome screens in Notes and Reminders (dismissed once
  in the clone; an agent would have met them too) and privacy prompts for Terminal's access to Reminders and
  Contacts (granted).
