## Amendment 1 (6 Oct 2026): the task set changed

Made before any analysed trial; see `PREREGISTRATION.md`, Amendment 1. Where this section and the text below disagree, this section wins.

| Set | Tasks | Role |
|---|---|---|
| **Primary: CDB suite** | CDB-S01, CDB-S02, CDB-S03, CDB-S04 | The original cua-driver-bench suite, ported. Task-macro mean of success. |
| Secondary: probes | MB-09, MB-10, MB-11 | Reported separately, never mixed into the primary result. |
| **Not run** | MB-01 to MB-08 | Dropped 6 Oct: unpublished original; reconstruction can't confirm or refute the claim. Files kept. |
| **Not run** | MB-12 | Dropped 6 Oct: its live validation failed (below). Files kept. |
| Separate run | CDB-G02, CDB-G03, CDB-G04 | GUI-only variant, Amendment 2. Reported separately, never pooled. |
| **Not run** | the pack's fifth task (macOS-native, iOS Simulator) | Needs Xcode and an iOS Simulator runtime. The VM image has Command Line Tools only. Four of the suite's five tasks run. |

Naming. "CDB-S01" is the name used for the whole suite in the request that changed the task set. The pack's first task is also called `cdb-s01`. In this repository the suite is the **CDB suite** and its tasks are CDB-S01 to CDB-S04, the pack's `cdb-s01` to `cdb-s04`.

### The CDB suite

The tasks are the original shared tasks of the private `trycua/cua-driver-bench` repository, at revision `16a1937a79ff2ee2e48f5b5d9bb5e6f944119b5a` (main, 6 Oct 2026). That material is proprietary and confidential (`../../PROVENANCE.md`), so this repository holds none of it: no brief, fixture, evaluator, oracle or hidden test. What is public is the generic adapter (`cdb_adapter.py`), the task selectors (`probes/CDB-S0x/task.json`), the digests of the four task trees (`pins.json`, `cdb_pack`) and the evidence rows. Anyone with access to the pack at that revision can rerun the suite with this runner; the preflight refuses a pack whose tree digest differs.

| ID | Pack task | Surfaces (no more is public, see above) | Wall (s) | Max turns |
|---|---|---|---|---|
| CDB-S01 | `shared/cdb-s01` | desktop apps, a local web app and a browser | 360 | 45 |
| CDB-S02 | `shared/cdb-s02` | four desktop apps, one of them a spreadsheet | 360 | 45 |
| CDB-S03 | `shared/cdb-s03` | three desktop apps, one of them a spreadsheet | 360 | 45 |
| CDB-S04 | `shared/cdb-s04` | desktop apps, a local service and a browser | 360 | 45 |

What is the same as in the original: the participant brief, the reset (the pack's own `reset/setup.py` and byte-level `reset/verify.py` before every trial), the apps and their launch commands and window frames, and the evaluator (the pack's `evaluator/evaluate.py`, final state only, every required check, run unchanged).

What is different, and why. Each item is a deliberate, documented adaptation:

1. **Where it runs.** In a Lume macOS VM on a separate Mac Studio, not on the owner's desktop and not in the original protected Lume adapter. The same VM image serves both arms, one VM, trials in series. Apps and workspace are reset per trial; the VM is not cloned per trial.
2. **Tools.** The original suite assumes a complete agent with a coding harness. Both arms therefore get `Bash`, `Edit` and `Write` in addition to `Skill`, `Read`, `ToolSearch` and their computer-use MCP server. The probes keep the first three only. The adaptation is the same in both arms. Because the evaluator grades final state, a run can pass without touching the GUI; the number of computer-use calls per trial is recorded and reported (GUI use is not enforced).
3. **Brief.** The pack's `brief.md` verbatim, with two edits: "the configured `cua` MCP server" becomes "the configured computer-use MCP server" (arm B's server is not called `cua`), and one line is appended with the workspace path, because the runner's working directory is not the workspace. The runner then adds its usual line with the time and turn limits.
4. **Descriptor apps not started.** The descriptor's `terminal` and `editor` apps are not started (both arms have `Bash` and `Edit`). Everything else in the descriptor is started in its order, and windows are moved to the descriptor's frames through Cua Driver's recorder daemon. The screen is 1920x1080, which the frames assume.
5. **Evaluator isolation.** The pack's evaluator, oracle, hidden tests, unit tests and READMEs sit in the home of a separate OS user that the agent's user cannot read. The agent-visible copy holds the apps, the fixture, the reset scripts, the launch descriptor and the brief. The runner calls the evaluator through one `sudo -u` entry point. This is weaker than the original protected adapter (host-side evaluation); it blocks reading, not running. Every tool input of every trial is scanned for evaluator, oracle, hidden-test and runner-path strings and the trial row carries `evaluator_peeks`; the analysis is reported with and without trials that have a peek.
6. **Participation receipt.** The original's driver-participation receipt was non-scoring and needs the protected mediator. It is not computed. Replaced by the tool-call counts above.
7. **Limits.** 360 s and 45 turns for every task, as everywhere in this study. The original private runs gave these tasks 1200 to 1800 s. A low success rate here is therefore not comparable with the original's.

Validation without a model (`validation/cdb_results.json`, produced by `tools/validate_cdb.py` inside the VM): for each of the four tasks the reset runs and verifies, the evaluator fails the pristine workspace, passes a scripted correct solution with score 1.0, and fails again after a second reset. The scripted solutions are the helpers of the pack's own unit tests. The pack's own unit tests (evaluator, reset, and bundle cases) also pass in the VM except the cases that need the pack's repository files outside `tasks/` (conformance briefs, git ignore rules), which are not part of a trial.

### Probes MB-09 to MB-11, and why MB-12 is out

MB-09, MB-10 and MB-11 are unchanged from the sections below.

MB-12 needed one live validation with a model-free background actor. It failed (`validation/mb12_live.json`). The actor drives BenchLab through Cua Driver 0.34.0 with background accessibility actions only: no pointer, no focus change. Every task check passes, the pointer does not move and no input leaks. But BenchSentinel resigns key focus for about 2 s while the Category popup menu is open, so the required check `front_unchanged` fails. The popup cannot be set without opening its menu (`set_value` is refused: the popup has no accessibility children until the menu opens). A task that a clean background run cannot pass cannot validate its checker, and changing the checker after seeing this is not something to do quietly. MB-12 is therefore dropped, not changed. The finding itself is reported: opening a popup menu in a background app takes key focus from the front window for the time the menu is open.

### Amendment 2: GUI-only variant (CDB-G02 to CDB-G04)

Same pack tasks as CDB-S02 to CDB-S04 with no `Bash`, `Edit` or `Write` (the tool surface of the probes). A separate, later run (`RUN_ID=gui`), 5 runs per arm per task, reported separately and never pooled. Side doors (Terminal, Script Editor, Automator, Shortcuts, shell escapes) are flagged, not blocked. The rules and the analysis plan are in `PREREGISTRATION.md`, Amendment 2.

### Order

CDB-S01, CDB-S02, CDB-S03, CDB-S04, MB-09, MB-10, MB-11. Runs 1 to 3 of every task come first (phase 1), then runs 4 and 5 (phase 2). In each task block the two arms alternate in the order given in `PREREGISTRATION.md`.

