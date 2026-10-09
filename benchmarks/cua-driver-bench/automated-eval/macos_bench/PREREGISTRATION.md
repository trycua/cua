# Pre-registration: Claude Code with Cua Driver vs Claude Code with Codex computer use

Status: frozen when this file is committed, before any trial that enters the analysis.
Date: 2026-10-05 (UTC evening). Scope: head-to-head, macOS host, one Mac. Owner: Cua Driver Bench maintainers.
Supersedes the pilot pre-registration (private trycua/cua-driver-bench PR #59: Codex CLI with gpt-6-astra, Cua Driver arm only).

**Amended on 6 Oct 2026, before any analysed trial: see Amendment 1 immediately below (Amendment 2, the GUI-only variant, after it, Amendment 3, the 7 Oct before/after check of the Cua Driver 0.35.0 changes, Amendment 4, named Electron bundles and a skill-in-prompt arm, Amendment 5, the v036 validation run of the 0.36 fixes, Amendment 6, the mini-runs of the overnight hill-climb, Amendment 7, the full rerun v037-full, and Amendment 8, the full rerun v038). Where sections 0 to 12 differ from them, the amendments win.**

## Amendment 1 (6 Oct 2026, before any analysed trial)

Committed before trial 1 of the analysis. No analysed trial has run. The smoke trials (two Haiku 4.5 and three Sonnet 5.5 trials of MB-05 on 5 Oct on the owner's Mac, and a few Haiku 4.5 trials of the ported CDB tasks on 6 Oct inside the VM) are marked `smoke` in the ledger and are never analysed. The text of sections 0 to 12 below is left as committed on 5 Oct for the record. Where it differs from this amendment, **this amendment wins**. Everything not named here is unchanged: both arms run `claude -p --model claude-sonnet-5-5`; arm A is official Cua Driver 0.34.0, pinned, with its 0.34.0 skill; arm B is OpenAI's shipped `cua_repl` launcher as MCP server `codex-cu` (computer surface only); ToolSearch is on in both arms; 360 s and 45 turns; the seeds rule; the stop rule.

### A1.1 What changed and why

1. **The primary result is now the original cua-driver-bench suite**, ported into this runner (the "CDB suite", below). It existed before the public dispute, so it is not a reconstruction. The owner approved this change on 6 Oct 2026.
2. **The eight AF reconstructions (MB-01 to MB-08) are dropped from the analysis.** Reason, in the words of the decision: unpublished original; a reconstruction can't confirm or refute the claim. The files stay in the repository and are marked "not run" in `TASKS.md`.
3. **The probes MB-09, MB-10 and MB-11 are secondary**, reported separately and never pooled with the primary result. **MB-12 is dropped**: its one live validation with a model-free background actor failed (a clean background run loses key focus for about 2 s when it opens the Category popup menu, so a required disturbance check fails; see `validation/mb12_live.json`). It is dropped, not changed.
4. **Where the trials run changed.** They run in a Lume macOS VM on a separate Mac Studio, not on the owner's desktop (section 3's "one Mac, no human at the desk" is replaced by A1.3).

### A1.2 Tasks

| Set | Tasks | Role |
|---|---|---|
| Primary | CDB-S01, CDB-S02, CDB-S03, CDB-S04 | task-macro mean of success; per-task tables |
| Secondary | MB-09, MB-10, MB-11 | reported separately; MB-10 and MB-11 keep their "no Codex hover tool" caveat |
| Not run | MB-12 | dropped after its live validation failed, see A1.1 item 3 |
| Not run | MB-01 to MB-08 | dropped, see A1.1 |
| Not run | the suite's fifth task (iOS Simulator, macOS-native) | needs Xcode and an iOS Simulator runtime, which the VM image lacks; four of the suite's five tasks run |

"CDB-S01" was the name used for the whole suite when the task set was changed; the suite's first task is also called `cdb-s01`. Here the suite is the **CDB suite** and its tasks are CDB-S01 to CDB-S04.

The CDB tasks are the shared tasks of the private `trycua/cua-driver-bench` repository at revision `16a1937a79ff2ee2e48f5b5d9bb5e6f944119b5a`. Their brief wording and success checkers are kept (brief verbatim except two neutral-name edits, checker unchanged). The material is proprietary, so this repository carries only the adapter, the selectors and the tree digests (`pins.json`, `cdb_pack`). `TASKS.md` lists every adaptation and why. In short: both arms also get `Bash`, `Edit` and `Write` on the CDB tasks (the original assumes a coding harness); the descriptor's terminal and editor apps are not started; the driver-participation receipt, which was non-scoring in the original, is not computed; the evaluator runs as a separate OS user and every tool input is scanned for evaluator paths (`evaluator_peeks`).

The 360 s and 45-turn limits are kept for the CDB tasks even though the original private runs allowed 1200 to 1800 s. Low success rates here are not comparable with the original's.

### A1.3 Environment

* One Lume macOS VM (`cdb-h2h`), cloned from the cached Cua base image, on a Mac Studio (Apple M3 Ultra) that is not the owner's desk. 6 CPU, 12 GB, 1920x1080, macOS 26.5.2 (25F84). The pins for macOS in `pins.json` now name the VM's version. The same VM, one at a time, serves both arms.
* Installed in the VM: Claude Code 2.1.289 (npm), Cua Driver 0.34.0 (the pinned private copy, same binary sha256 as before), the ChatGPT app 26.930.51102 and its shipped plugin (copied from the owner's Mac as signed bundles, never launched in the VM, because the app updates itself when it starts), BenchLab and BenchSentinel (built in the VM), Google Chrome (base image), LibreOffice 26.2.5.2 and GnuCash 5.16-3 (copied from the owner's Mac), Node 24.21.0, Electron 43.4.0 (the pack's lockfile), ffmpeg 9.0.2.
* Reset: the workspace and apps are reset before and after every trial, as before. A VM clone or snapshot restore per trial is **not** done: the runner lives inside the VM and cannot restore its own VM, and a restore per trial would not fit the quota window. State that could leak between trials is limited to the VM's filesystem outside the reset scope.
* Same launch: the runner starts from a Terminal.app shell inside the VM, both arms are its children with the scrubbed environment of section 2.
* Screen recording: Cua Driver's recorder inside the VM, one video per trial. The preflight's 3-second recording test now passes at 0.5 s of video instead of 1.0 s, because the recorder writes a variable-frame-rate file and an idle VM desktop produces few frames.
* The owner's Mac is not used for trials, and its HOLD file stays.

### A1.4 Credentials and the quota seat

* Claude Code in the VM authenticates with a long-lived OAuth token created with `claude setup-token` and approved by the owner's logged-in browser. The runner hands it to `claude` through a file descriptor (`CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR`); it is never in an environment variable, a command line, a log or a commit.
* The quota rule (stop starting trials at a 7-day utilization of 0.95, or on `rejected`) is unchanged and applies to the seat that the VM's token uses, as reported by `rate_limit_event`. That seat is not the one the owner's Mac uses (readings at 07:19 to 07:43 UTC on 6 Oct: Mac 0.35 with reset on 12 Oct 07:00 UTC; VM token 0.19 with reset on 9 Oct 13:00 UTC). The quota log in the report names the seat of every reading.

### A1.5 Order, runs, analysis

* Order: CDB-S01, CDB-S02, CDB-S03, CDB-S04, then MB-09, MB-10, MB-11. Arms are interleaved inside each task block as in section 6.
* Runs: five per task per arm is the target, three the guaranteed minimum. Phase 1 runs 1 to 3 of every task in that order, phase 2 runs 4 and 5 only when phase 1 is complete for every task and the quota rule allows it.
* Only complete task blocks are analysed. The primary analysis is section 8 applied to the CDB suite; the secondary analysis is section 8 applied to MB-09 to MB-11; there is no AF analysis. For the CDB suite, success is the pack evaluator's `passed` (every required check); its partial `score` is reported next to it. Trials with `evaluator_peeks` are reported separately and the headline is shown with and without them.
* Stop rule, unchanged: no new trial once seven-day utilization is 0.95 or higher, or on any `rejected`. The trial in flight finishes. A cutoff time (`CUTOFF_UTC`) is set so that nothing runs into the owner's evening.

### A1.6 Harness changes since the 5 Oct commit (all before any analysed trial)

* `cdb_adapter.py` and `probes/CDB-S0x/task.json` (generic adapter, no task content); `tools/validate_cdb.py`, `tools/smoke_cdb_apps.py`; `tools/validate_mb12_live.py`; MB-12 `task.json` marked dropped.
* `claude_arms.py`: coding tools for tasks that ask for them; `open_token_fd`. `claude_driver.py` and the quota probe pass the token descriptor.
* `run_bench.py`: CDB path in `run_attempt`, window placement, final-workspace snapshot, evaluator-path scan, preflight checks for the pack, the apps and the isolation; tasks with `"status": "dropped"` are skipped; recording-test threshold 1.0 s to 0.5 s.
* `bench_core.py`: priority order puts `CDB-*` before `MB-*`.
* `pins.json`: VM macOS version and build, `cdb_pack` revision and digests.
* `launch/launch_bench.command`: `--cutoff-utc` is passed as a zsh array (the first VM launch at 08:56 UTC failed on it before any trial), and the launcher minimises its own Terminal window so it does not cover task windows. The real run started at 08:57:55 UTC with this launcher; no trial had completed when this line was added.
* `cdb_adapter.py`: the substitution `${workspace_uri}` (used by two launch descriptors for the LibreOffice profile) was missing, so LibreOffice never opened and the first CDB-S02 trials ended as harness exceptions before the agent started (3 attempts, 09:05 to 09:10 UTC). The runner was stopped after the CDB-S01 block (6 trials, all run with the same adapter and unaffected), the 3 harness-exception rows were moved to `runs/main/excluded-harness-bug/` (kept for audit, not counted), the substitution was added, the apps of CDB-S02, S03 and S04 were started once without a model to check windows and then the run resumed at 09:24 UTC with the same run id.

### A1.7 Validation done before the first trial (no model call)

* CDB-S01 to CDB-S04: reset, byte-level verify, evaluator fails the pristine workspace, passes a scripted correct solution (score 1.0) and fails again after a second reset. Run through the adapter, inside the VM, with the evaluator isolated (`validation/cdb_results.json`).
* MB-12: live validation with a model-free background actor failed (`validation/mb12_live.json`); MB-12 is dropped.
* Runner preflight inside the VM: pins, both MCP servers, recording, sentinel, daemon permissions, and the CDB pack and isolation checks.


## Amendment 2 (6 Oct 2026, before any GUI-only trial): a GUI-only variant of three CDB tasks

Written and committed before the first GUI-only trial. The approved plan of Amendment 1 keeps running unchanged and finishes first, phase 2 included. This amendment adds a separate, later run. It changes nothing for the trials of Amendment 1.

### A2.1 Why, and what was seen before writing it

In phase 1 of the Amendment 1 run (runs 1 to 3), about 10:00 UTC on 6 Oct, the CDB results were: CDB-S01 3/3 in arm A and 3/3 in arm B; CDB-S02 0/3 and 0/3; CDB-S03 2/3 and 3/3; CDB-S04 in progress (the first runs passed in both arms). In all 12 CDB-S02 and CDB-S03 trials neither arm made a single computer-use call. With `Bash`, `Edit` and `Write` available, both arms read and edited the apps' data files directly ("I edited the data files directly. I didn't open LibreOffice Calc."). CDB-S01 and CDB-S04 do use the GUI (the brief asks for a ticket read and a browser check). So on S02 and S03 the Amendment 1 result measures coding, not the computer-use tool layer. The owner approved a GUI-only variant. The rules and the analysis plan below are fixed now, before any GUI-only data exists.

### A2.2 A note on the 0/6 in CDB-S02 (not a rule; the evaluator is unchanged)

The evaluator's reasons, per trial: in all six trials `blocker_recorded` failed ("expected one LHP-C17 blocker, found 0") and `blocker_audit` failed (audit replay found 0 entries and does not match the blocker store). The structured blocker has to be recorded in the chat app, whose store keeps an audit chain; nobody opened the app, so none was recorded. In addition `calendar_final` and `preservation` failed in one arm A trial (the cancelled session was left in the calendar) and `workbook_agreement` in another (the blocked row named the protected hold instead of the conflict id). The evaluator did not reject a correct end state. No evaluator change was made. If one is ever needed it will be a new amendment and will apply only to trials run after it.

### A2.3 Tasks

CDB-G02, CDB-G03 and CDB-G04 are the pack's `cdb-s02`, `cdb-s03` and `cdb-s04` with no coding tools. Brief wording (with the two edits of Amendment 1), evaluator, reset, apps, window frames, 360 s and 45 turns, the VM and the evaluator isolation are the same as for CDB-S02 to CDB-S04. CDB-S01 is not repeated GUI-only: its required end state includes a source-code fix and a regression test, which needs a coding tool.

### A2.4 Tool surface

Identical to the probes in both arms: `Skill`, `Read`, `ToolSearch` and the arm's computer-use MCP server. No `Bash`, `Edit` or `Write`. Arm A keeps its Cua Driver skill, arm B has none, as everywhere else.

### A2.5 Closing the GUI side door: flag, do not block

Chosen: **flag and report separately**. Blocking is not possible for both arms: the runner itself starts from Terminal.app (arm B's per-app approvals need it), and Terminal, Script Editor, Automator and Shortcuts are system apps that cannot be removed. The two arms are also not gated alike: arm B's host loop already declines per-app approvals for apps outside the task's list (Terminal and the others are declined), Cua Driver has no equivalent gate. Detection is therefore the same for both arms and a flagged trial is reported, not recoded.

A trial is **flagged** when any of these happens during its agent phase:

1. the frontmost app, sampled every 0.5 s, is Terminal, iTerm, Script Editor, Automator or Shortcuts;
2. a process named Script Editor, Automator, iTerm2 or Shortcuts exists;
3. a tool input of the computer-use server contains `Terminal`, `iTerm`, `Script Editor`, `ScriptEditor`, `Automator`, `Shortcuts`, `osascript`, `child_process`, `execSync`, `spawnSync`, `/bin/sh`, `/bin/zsh`, `/bin/bash`, `bash -c`, `zsh -c`, `subprocess` or `os.system`.

Rows carry `frontmost_seen`, `side_door` and `side_door_flag`. A flagged trial stays in the success counts. The results are shown twice, with all trials and with unflagged trials only, and every flagged trial is listed with its evidence.

### A2.6 Run protocol

A separate run (`RUN_ID=gui`) that starts after the Amendment 1 run has ended (done, or stopped by the stop rule or the cutoff), on the same VM, with the same preflight and the same stop rule: no new trial once the VM seat's seven-day utilization is 0.95 or higher, or on any `rejected`, or after `CUTOFF_UTC`. Five runs per arm per task (30 trials) in one phase, in the order CDB-G02, CDB-G03, CDB-G04, arms interleaved inside each task block as in section 6. Only complete task blocks are analysed; the guaranteed minimum is three runs per arm.

### A2.7 Analysis plan

* Per task and arm: successes over trials with Wilson 95% intervals, the pack evaluator's `passed` as success, the partial score, wall time, turns, tokens, equivalent cost, and the failed checks with the evaluator's reasons.
* Per task, the paired difference of success rates with the descriptive Fisher exact p-value of `analyze.py` (no multiplicity correction, not a decision threshold); across the three tasks the task-macro mean per arm with the hierarchical bootstrap interval of `analyze.py`, group `CDBG`.
* Computer-use calls per trial by tool name and arm, as in the main CDB table.
* All of the above for all trials and for unflagged trials only (A2.5).
* The GUI-only result is **never pooled** with the CDB suite of Amendment 1 or with the probes, and is reported in its own section. No directional hypothesis is registered. The wording of the conclusions stays descriptive: what happened in these trials, how many, how wide the intervals are.

### A2.9 Procedural deviation during the GUI-only run

At 11:20 UTC (after 23 of the 30 trials) the first seat's five-hour window was exhausted (`rate_limit_event` status `rejected`, `five_hour` 1.00, `seven_day` 0.29, reset 12:40 UTC). The pre-registered rule is to wait for the reset. At the owner's request the VM's Claude credential was switched at 11:54 UTC to a second seat (five-hour 0.00, seven-day 0.85 at the first call) instead of waiting, and the run resumed with the same run id, model and settings. Only an access token was copied (no refresh token). The seven-day stop line of 0.95 applies to the second seat from then on. Trials before the switch ran on the first seat, trials after on the second; each row carries its own quota readings. The deviation is recorded in `runs/gui/account-switch.txt` and in the report.

### A2.8 Harness changes for this amendment

`probes/CDB-G02` to `CDB-G04` (`"coding_tools": false`, `"separate_run": true`, group `CDBG`); the default schedule skips tasks with `separate_run`, so the Amendment 1 run cannot pick them up; `FrontWatcher` and `side_door_scan` in `run_bench.py` with the row fields of A2.5; tests in `tests/test_cdb_adapter.py`.


## Amendment 3 (7 Oct 2026, before any trial of this run): before/after check of the Cua Driver 0.35.0 changes

Written and committed before the first trial of run `v035`. It adds a new, separate run on the same VM. It changes nothing about the trials, analyses or reports of Amendments 1 and 2.

### A3.1 Why

The 6 Oct results showed arm A (Cua Driver 0.34.0) passing the GUI tasks it passed at 1.6 to 2 times the wall time and 3 to 4 times the equivalent cost of arm B, mostly through more turns and the cache-read tokens that come with them, and losing the GUI-only tasks to the turn cap. Cua Driver's main branch now carries changes aimed at that: trimmed tool descriptions and up-front workflow guidance (trycua/cua #4742), a lean `get_window_state` default with a `since` diff and a `full_output` opt-out (#4743), a `run_actions` batch tool (#4737), fixes for the failing calls seen on 6 Oct (#4739 element_token without `pid` and next steps after refusals, #4740 `set_value` on native and Chromium popups, #4741 submitted `type_text`), an update-check fix (#4757) and new cursor-motion planning (#4758, #4759, #4767). None is released. This run asks whether the main build is more efficient than 0.34.0 on the same tasks without losing success, before 0.35.0 ships.

### A3.2 Arms

| Label | Runner arm | Tool layer | Skill |
|---|---|---|---|
| A | `cc-cua-driver-main` | Cua Driver built from trycua/cua main at `365f5e3c5b92f9457dbd560ddea8ec0268565724` | `libs/cua-driver/rust/Skills/cua-driver` at that commit |
| A0 | `cc-cua-driver` | Cua Driver 0.34.0, the same pinned private copy as on 6 Oct | the 0.34.0 skill, as on 6 Oct |
| B | `cc-codex-cu` | OpenAI's `cua_repl` launcher, unchanged, as on 6 Oct | none |

Everything else is as in Amendments 1 and 2: `claude -p --model claude-sonnet-5-5` (Claude Code 2.1.289), the same system prompt, ToolSearch on, the same scrubbed environment, `--setting-sources project`, the skill delivered as a project skill, 360 s and 45 turns, the same VM (`cdb-h2h`, macOS 26.5.2, 1920x1080), the same reset, recorder, sentinel, evaluator isolation, side-door flagging and evaluator-path scan.

The main build:

* Built inside the VM from a `git archive` of `libs/cua-driver`, `libs/cua` (the telemetry crate's workspace) and two files that crate includes, at the commit above: `cargo build --locked --release -p cua-driver -p cursor-theme-cli` with the repository's toolchain (rustc 1.97.1), `CUA_DRIVER_GIT_SHA` set to the commit. The source tree still says version 0.34.0 (the release pull request bumps it), so the build reports `cua-driver 0.34.0`; it is identified by commit and hash, which `health_report` returns (`git_sha` = the commit, `exe_sha256` below).
* Wrapped in a private app `CuaDriverBenchMain.app` (the 0.34.0 Info.plist and icon, bundle id `com.trycua.driver.benchmain`), ad-hoc signed with the hardened runtime and the release's Apple Events and screen-capture entitlements. Binary sha256 `af059a7f06f0df00e0b1d7521936f3bc88750b9ed957fa3a321cd1a93d2bb2c1`, skill tree sha256 `e0d64711489f8d2c29747b41be8361e902ba62e1361c2e62d945f7884897bd64` (`pins.json`, `cua_main`). Preflight fails if either differs or the daemon reports another commit.
* Permissions: the VM has SIP disabled, so Accessibility and Screen Recording (system TCC database) and the Automation grant for Chrome (user database) were written for `com.trycua.driver.benchmain`, bound to this build's code hash, mirroring the grants 0.34.0 has. `health_report` shows both TCC checks passing; its `bundle_identity` check fails by design (not the release bundle id) and is not gated.
* Its own daemon socket and state (`/tmp/cdb-bench-cua-main.sock`, `WORK/cua-main/daemon-state`). Only one agent daemon runs at a time: the runner stops the other build's daemon before a Cua Driver trial (`ensure_agent_daemon`). The recorder keeps using the 0.34.0 build with no overlay, as before.
* `DO_NOT_TRACK=1` is added to both Cua Driver daemons' environment (the main build's shared telemetry crate honours it; `CUA_DRIVER_RS_TELEMETRY_ENABLED=false` stays).

### A3.3 Tasks, order and runs

One run, `RUN_ID=v035`, task blocks in this order: CDB-S01, CDB-S04, CDB-G02, CDB-G03, CDB-G04, MB-09, MB-10, MB-11, then CDB-S02, CDB-S03. The first eight are the **GUI-heavy set**: the tasks where 6 Oct trials used the computer-use tools. S02 and S03 come last because on 6 Oct no trial used a computer-use call in them. Each task keeps its own tool surface from its `task.json` (CDB-S0x with `Bash`, `Edit`, `Write`; CDB-G0x and MB-xx without).

Three arms are interleaved inside every task block. The first arm of a run rotates: `(task position + run index) mod 3` over the list (A, A0, B), so every arm goes first equally often across three runs. All three arms share the run's seed.

Phase 1 is runs 1 to 3 of every task; phase 2, runs 4 and 5 of every task, only when phase 1 is complete and the stop rule and cutoff allow it. Only complete task blocks are analysed; three runs per arm is the guaranteed minimum, five the target.

### A3.4 Hypotheses (registered before any trial)

Primary set: the eight GUI-heavy tasks. Pairing: A and A0 trials of the same task and run (same seed).

* **H1 (tokens).** A processes fewer tokens per trial than A0. Metric: total tokens per trial = uncached input + output + cache read + cache write, from the result event. Cache read and cache write are also reported separately.
* **H2 (turns).** A uses fewer turns per trial than A0 (the row field `turns`, the "Turns" column of the 6 Oct tables; `num_turns` of the result event is reported next to it).
* **H3 (success not worse).** A's success is not worse than A0's. Success is the evaluator's `passed`, as before.

Tests (`tools/analyze_v035.py`, written before the first trial):

* H1, H2: for each task, the ratio of means A / A0. Across the eight tasks, the geometric mean of the per-task ratios, with a 95% bootstrap interval (10 000 resamples of runs within task, seed 20261007). **Supported** when the upper bound is below 1.0. Also reported: per-task ratios, the share of pairs where A is lower, and the same numbers for all ten tasks.
* H3: the task-macro success difference A minus A0 over the eight tasks, with the same bootstrap. **Supported** when the lower bound of the 95% interval is at or above -0.15 (non-inferiority margin, fixed now). Also reported: every task where A passed at least two fewer trials than A0, with the evaluator's reasons.
* All three are reported whatever the outcome. No other hypothesis is registered. The comparisons with arm B are descriptive, as in Amendments 1 and 2.

### A3.5 Secondary measures and breakdowns

Per task and arm: success (with Wilson intervals), score, wall time, turns, computer-use calls, failed calls, uncached input, output, cache-read and cache-write tokens, equivalent cost (`total_cost_usd`), pointer moved, focus stolen, flags, peeks. The same post-hoc breakdowns as on 6 Oct, now labelled as planned for this run: failed Cua Driver calls by cause, and actions versus observes per trial (a `run_actions` call counts each inner action as an action). New for A: how often `run_actions`, `since` and `full_output` are used. The CDB-S, CDB-G and MB sets are shown in separate tables; the only pooling is the pre-registered H1 to H3 over the eight GUI-heavy tasks.

### A3.6 Credentials and stop rule

* The VM authenticates with the **access token only** of one Claude seat chosen from the owner's account switcher (`cswap`) by headroom at launch (seat "account 3": five-hour 5%, seven-day 64% at 21:04 UTC on 6 Oct). It is copied from the switcher's keychain item without its refresh token into a 0600 file in the VM and handed to `claude` by file descriptor, as in Amendment 1. The access token expires after a few hours; when the switcher rotates it, the new access token (again without the refresh token) replaces the file, after a fresh check of the switcher's readings. Nothing is printed, logged or committed; the file is deleted after the run.
* Stop rule, unchanged: no new trial once that seat's seven-day utilization is 0.95 or higher or on a seven-day `rejected`; a five-hour rejection waits for its reset. `CUTOFF_UTC` for new trials: 2026-10-07T05:30:00Z.
* Arm B keeps the OpenAI terms caveat of the 6 Oct report: its numbers stay internal until that review.

### A3.7 Harness changes for this amendment

`claude_arms.py`: a `CuaBuild` table with the two Cua Driver builds (binary, socket, state, skill), per-build MCP config and project skill, optional binary for the daemon helpers, `DO_NOT_TRACK=1`. `arms.py`: `cc-cua-driver-main` added to the Claude arms. `run_bench.py`: one agent daemon at a time (`ensure_agent_daemon(ctx, arm)`, `stop_agent_daemons`), per-build version fields in each row (`cua_driver_version`, `cua_driver_sha256`, new `cua_driver_git_sha`, skill hash), preflight pins and daemon checks for the main build, the skill-presence check for both Cua Driver arms. `pins.json`: `cua_main`. `tools/analyze_v035.py`: the analysis of A3.4 and A3.5. Tests: `tests/test_bench.py` (main-build MCP config, three-arm rotation).


### A3.8 Procedural deviation during run v035 (recorded during the run)

At 23:06 UTC on 6 Oct, after 50 counted trials (blocks CDB-S01 to CDB-G04 complete, MB-09 in progress), the first seat (switcher account 3) returned `rejected` for its five-hour window (five-hour 1.00, seven-day 0.73). The registered rule is to wait for the reset (about 01:50 UTC). At the owner's instruction (use capacity where it exists) the run was stopped at 23:09 UTC with nothing in flight and the VM credential was switched to the access token, without its refresh token, of switcher account 6 (five-hour 0.17, seven-day 0.52); accounts 1, 2, 4 and 5 were at seven-day 0.91 to 0.96 and were not used. The old token file was deleted. The run resumed at 23:10 UTC with the same run id, model and settings; the rate-limited attempt is an `infra_error` row and its trial was rerun. Because account 6 is also the owner's active seat, the run additionally stops at 0.95 on its five-hour window (host watchdog), not only at the registered seven-day 0.95. Each row carries its own quota readings; the switch is recorded in `runs/v035/account-switch.txt`. `tools/analyze_v035.py` was corrected at the same time to read the stream of each trial's counted attempt (it had read attempt 1); this changes no rule.

### A3.9 Carry-over found in phase 1: the pointer was not reset between trials (recorded during the run)

Reading the phase 1 results at 23:50 UTC on 6 Oct (before any phase 2 probe trial): in MB-10 (hover-only toolbar) all nine trials passed, including arm B, which has no hover tool and failed all five MB-10 trials on 6 Oct. Eight of the nine passed without moving the pointer. The first MB-10 trial (arm A, run 1) moved the real pointer onto the toolbar and left it there; the toolbar then stayed visible for every later trial, so the hover check was met without a hover. The harness reset apps and workspace between trials but not the pointer position. The same carry-over could have happened on 6 Oct; there the first arm A trial's pointer position did not happen to land on the toolbar in a way that let arm B pass.

Decision, made before any phase 2 probe trial: from phase 2 on, the runner moves the real pointer to an empty point of the menu bar (1000, 12) through the recorder daemon before every trial and records the read-back in the row (`pointer_parked`, flag `--park-pointer`). Nothing else changes. In the report, the phase 1 MB-10 trials are shown but marked as confounded and are not used for success comparisons; H1 to H3 are reported as registered (all eight tasks, all complete runs) and, labelled post-hoc, without MB-10. MB-11 (tooltip) is less exposed (the tooltip needs a fresh hover), but its phase 1 and phase 2 results are also shown separately.

The restart to load this change exposed a harness gap: a resumed run had no block timings, so the registered "phase 2 block must fit before the cutoff" check (unknown estimate means do not start) stopped it at once (`STOPPED_TIME`, 23:51 UTC, no trial lost). The runner now seeds the block timings of a resumed run from the summed trial wall times of finished blocks. The rule itself is unchanged.

## Amendment 4 (7 Oct 2026, before any trial it covers): named Electron bundles and a skill-in-prompt arm

Written and committed before the first trial run under it. It changes nothing about the trials, analyses or reports of Amendments 1 to 3. It applies to every run started after this commit (the first is a smoke run, A4.4; the validation run of the Cua Driver 0.36 fixes will get its own amendment with its builds, order and hypotheses before it starts).

### A4.1 Why

* **Electron addressing (CUA-1224).** The pack's launch descriptors start every Electron app with `npx electron .`. All of them were therefore processes of one `Electron.app`, bundle id `com.github.Electron`, process and app name "Electron". Codex computer use addresses apps by name or bundle id; in the 6 Oct GUI-only run it failed with `Ambiguous app identifier 'com.github.Electron'` 13 times and `Invalid app: <the app's window title>` 5 times, in CDB-G02 (three Electron windows of one app directory) and CDB-G03. Arm B's G02 and G03 results of 6 Oct and of run `v035` are therefore not a clean reading of its tool layer. Cua Driver addresses apps by pid and window id, so the shared bundle id did not block it in the same way.
* **Skill delivery (CUA-1226).** Arm A's skill reached the model only as a Claude Code project-skill listing. On 6 Oct it was opened in 1 of 35 main-run arm A trials (7 of 15 GUI-only). "Arm A with the skill" was mostly arm A without it, so the effect of the skill is unmeasured.

### A4.2 Electron apps as named bundles (all arms)

For every app of kind `electron` in a task's launch descriptor, the runner (`cdb_adapter.py`) replaces `npx electron` with the `Electron` executable of a copy of the pack's own `node_modules/electron/dist/Electron.app` (Electron 43.4.0, from the pack's lockfile). The copy:

* is named after the app's window title in the descriptor (`<title>.app`; role, then id, if there is no title);
* has `CFBundleName` and `CFBundleDisplayName` set to that name and `CFBundleIdentifier` `com.trycua.cdbbench.<slug of the name>`; its helper apps get the same id with their original suffix (`.helper`), as electron-packager does;
* keeps `CFBundleExecutable` `Electron`, so the helpers resolve as before and the runner's leftover cleanup (`pkill -x Electron`) is unchanged;
* is ad-hoc re-signed (`codesign --force --deep --sign -`) and registered with LaunchServices; it is made once per name under `$CDB_BENCH_WORK/electron-apps` and reused.

The binary, the app code, the arguments, the working directory, the environment, the window frames, the brief and the evaluator are unchanged, and so is the pack: still revision `16a1937a79ff2ee2e48f5b5d9bb5e6f944119b5a` with the tree digests in `pins.json`. No pack change was needed because the bundle identity is set by the runner at launch. Every row records `electron_named_bundles`. `CDB_ELECTRON_SHARED=1` restores the old shared launch, for reproducing earlier runs only.

Check before any trial (no model call): in the VM, start the apps of CDB-G02 and CDB-G03 through the adapter and confirm that each Electron app appears as its own app, with its own name and bundle id, in Cua Driver's `list_apps` and in Codex computer use's `cua.listApps()`, and that `cua.getApp(<name>)` resolves each one. The result is recorded in A4.6.

### A4.3 A skill-in-prompt arm

| Label | Runner arm | Tool layer | Skill delivery |
|---|---|---|---|
| AS | `cc-cua-driver-main-skill` | the same build, private app, daemon and MCP config as `cc-cua-driver-main` | the build's `SKILL.md`, verbatim, appended to the system prompt; the project skill also stays in `.claude/skills/cua-driver` |

The system prompt of arm AS is the shared system prompt, then this fixed text, then the full `SKILL.md` of the build's skill tree:

> The instructions of the Cua Driver skill follow. They apply to the `cua` MCP server. The skill's other files, which these instructions refer to, are in .claude/skills/cua-driver/ and can be read with the Read tool.

Nothing else differs from `cc-cua-driver-main`: model, tools, ToolSearch, limits, environment, daemon, MCP server. The other arms keep the shared system prompt. Each row records `system_prompt_sha256` and `skill_in_prompt`; the manifest records the prompt hash of every arm. The arm is not in the runner's default arm set (`--arms` must name it), so a run without `--arms` is unchanged. Comparing AS with A on the same build and seeds measures the effect of putting the skill in front of the model; that comparison is descriptive until a run registers a hypothesis for it.

### A4.4 Smoke run

One smoke run, `RUN_ID=v036-smoke`, rows marked `smoke` and never analysed: CDB-G02 and CDB-G03, one run each, all four arms (A `cc-cua-driver-main`, A0 `cc-cua-driver`, AS `cc-cua-driver-main-skill`, B `cc-codex-cu`), first arm rotating as in A3.3 over that list. Purpose: check A4.2 and A4.3 end to end (each arm starts, arm B can address each Electron app, AS's init event shows the longer prompt). Its numbers are not a result.

### A4.5 Credentials, stop rule, arm B

* Credentials as in A3.6: the access token only, without its refresh token, of one seat chosen from the owner's account switcher by its latest readings, in a 0600 file handed to `claude` by file descriptor, deleted afterwards. No new trial once that seat's five-hour or seven-day utilization is 0.95 or higher, or on `rejected`.
* Arm B's numbers stay internal until the OpenAI terms review (CUA-1225). No arm B number from a run under this amendment is committed to this repository.

### A4.6 Verification record

Done in the VM `cdb-h2h` on 7 Oct 2026 between 09:30 and 09:45 UTC, before the smoke run, with `tools/check_named_electron.py` (no model call). The output names the pack's apps, so it is kept with the private run data, not in this repository.

| Task | Electron apps | LaunchServices | Cua Driver `list_apps` | Codex computer use |
|---|---|---|---|---|
| CDB-G02 | 3 | 3 processes, 3 distinct names and bundle ids `com.trycua.cdbbench.*`, each from its own bundle | all 3 listed by name; no entry named "Electron" | `cua.getApp(<title>)` resolved all 3; one approval request per app, by name, accepted from the task's allowlist; `cua.listApps()` lists the 3 as running, with their own ids |
| CDB-G03 | 1 | 1, own name and id | listed by name | `cua.getApp(<title>)` resolved it; approval by name |

`cua.listApps()` also still lists two `com.github.Electron` entries as recently used and not running (LaunchServices history from earlier runs). They cannot be confused with the running task apps, which have their own names and ids.

### A4.8 BenchLab must be unoccluded at trial start (added 7 Oct 2026, before any trial under this amendment except the smoke run)

CUA-1219 found that MB-10 (hover toolbar) fails for both Cua Driver builds whenever another window covers BenchLab's hover zone at trial start (0/5 covered, 5/5 uncovered, in a controlled VM test; a Terminal window left open after boot was the cover). From now on, before every BenchLab probe (MB-xx and PROBE-xx; not the CDB tasks), after the pointer is parked and the sentinel activated, the runner reads the window list through the recorder daemon. Every normal-layer window of another app (BenchSentinel included) that is above BenchLab's window and overlaps it is moved to x = 840 (right of BenchLab's frame), keeping its size and y. The runner then reads the list again. The row records `lab_occlusion` (BenchLab's window, the occluders at start, the windows moved, the occluders after) and `lab_unoccluded`. A trial whose `lab_unoccluded` is false or unknown is flagged and reported separately, like A3.9's pointer carry-over. `--no-check-occlusion` turns it off, for reproducing earlier runs only. Earlier MB-10 and MB-11 results are not recomputed; the report notes that they may include covered starts.

### A4.7 Harness changes for this amendment

`cdb_adapter.py`: `named_electron_app`, `electron_argv`, `app_display_name`; `start_apps` uses them for `electron` apps. `claude_arms.py`: `CuaBuild.skill_in_prompt`, arm `cc-cua-driver-main-skill`, `system_prompt_for`. `arms.py`: the arm added to `CLAUDE_ARMS`, `DEFAULT_CLAUDE_ARMS` keeps the three arms of Amendment 3. `run_bench.py`: the per-arm system prompt in the trial, init check, manifest and dry run; row fields `system_prompt_sha256`, `skill_in_prompt`, `electron_named_bundles`; the main-build pin check runs for any arm on the main build. `tools/tasks_amendment.md` and `TASKS.md`: adaptation 8. `run_bench.py` (A4.8): `lab_occluders`, `ensure_lab_unoccluded`, `--check-occlusion`, row fields `lab_occlusion`, `lab_unoccluded`. Tests: `tests/test_cdb_adapter.py` (named bundle, argv rewrite, occluders), `tests/test_bench.py` (skill arm differs only in the system prompt).

## Amendment 5 (7 Oct 2026, before the first trial of run v036): validation of the Cua Driver 0.36 fixes, and the skill-in-prompt arm

Written and committed before the first trial of run `v036`. It adds a new, separate run on the same VM. It changes nothing about the trials, analyses or reports of Amendments 1 to 4. The harness is the one of Amendment 4: named Electron bundles (A4.2), the skill-in-prompt arm (A4.3) and the BenchLab occlusion check (A4.8).

### A5.1 Why

Run v035 (Amendment 3) found that the main build cut tokens per turn but not turns, and did not support H1 (tokens) or H2 (turns). The fixes made since then for those findings are now on main: #4812 (lean-read failure modes, batching guidance), #4821 (hidden helper windows, serialized foreground), #4831 (scroll dx/dy, `get_window_state` inside `run_actions`), #4835 (browser activation and field focus before foreground typing), #4760 (docs) and #4820 (`run_actions` steps that find, wait for and check by name). This run asks again whether main is more efficient than 0.34.0 without losing success, and whether putting the skill in the system prompt changes success or turns.

### A5.2 Arms

| Label | Runner arm | Tool layer | Skill |
|---|---|---|---|
| A | `cc-cua-driver-main` | Cua Driver built from trycua/cua main at `ccaacd8fda023f55a299689beefcc76389a617b1` (the merge of #4820, which contains #4812, #4821, #4831, #4835 and #4760) | `libs/cua-driver/rust/Skills/cua-driver` at that commit, as a project skill |
| A0 | `cc-cua-driver` | Cua Driver 0.34.0, the same pinned private copy as on 6 and 7 Oct | the 0.34.0 skill, as a project skill |
| AS | `cc-cua-driver-main-skill` | the same build, app and daemon as A | the same skill, with its `SKILL.md` also in the system prompt (A4.3) |
| B | `cc-codex-cu` | OpenAI's `cua_repl` launcher, unchanged | none |

The main build is made as in A3.2 (the scripts are in `tools/vm_main_build/`): built inside the VM from a `git archive` of that commit (`libs/cua-driver`, `libs/cua` and the few files outside them that the workspace includes) with `cargo build --locked --release -p cua-driver -p cursor-theme-cli` (rustc 1.97.1), `CUA_DRIVER_GIT_SHA` set to the commit; wrapped in the same private app `CuaDriverBenchMain.app` (bundle id `com.trycua.driver.benchmain`), ad-hoc signed with the hardened runtime and the same entitlements; the TCC rows re-bound to the new code. The source still says version 0.34.0, so the daemon reports `0.34.0`; the build is identified by commit and hash. `pins.json` `cua_main` now holds this build: binary sha256 `bffa727c840c85128d53e5fca0dab2fdd27014f23b69c3603896f3762d8dce77`, skill tree sha256 `668c0235269abd40553a98840eb800f30627a838c1df1d56ff3418d95fd60a0a`. The v035 values remain in Amendment 3 and in the history of `pins.json`. The preflight fails if either hash differs or the daemon reports another commit.

Everything else is as in Amendments 1 to 4: `claude -p --model claude-sonnet-5-5` (Claude Code 2.1.289), the shared system prompt (AS adds the skill), ToolSearch on, `--setting-sources project`, the scrubbed environment, 360 s and 45 turns, the VM `cdb-h2h` (macOS 26.5.2, 1920x1080), reset, recorder, sentinel, evaluator isolation, side-door flagging, evaluator-path scan, pointer parking before every trial (`--park-pointer`), named Electron bundles, and the BenchLab occlusion check.

### A5.3 Tasks, order and runs

`RUN_ID=v036`. The ten tasks and order of A3.3: CDB-S01, CDB-S04, CDB-G02, CDB-G03, CDB-G04, MB-09, MB-10, MB-11, CDB-S02, CDB-S03; the first eight are the GUI-heavy set. Four arms interleaved inside every task block, arm list (A, A0, AS, B); the first arm of a run is `(task position + run index) mod 4` over that list. All arms share the run's seed. Phase 1 is runs 1 to 3 of every task; phase 2 is runs 4 and 5, only when phase 1 is complete and the stop rule and cutoff allow it. Only complete task blocks are analysed; three runs per arm is the guaranteed minimum, five the target. `CUTOFF_UTC` for new trials: 2026-10-08T09:00:00Z.

### A5.4 Hypotheses (registered before any trial)

Primary set: the eight GUI-heavy tasks. Pairing: trials of the same task and run (same seed). Tests in `tools/analyze_v036.py`, written before the first trial, with the bootstrap of A3.4 (10 000 resamples of runs within task, seed 20261007).

A against A0, kept from A3.4 with the same metrics and rules:

* **H1 (tokens).** A processes fewer tokens per trial than A0: geometric mean over tasks of the ratio of means A/A0; supported when the upper bound of the 95% interval is below 1.0.
* **H2 (turns).** A uses fewer turns per trial than A0: same test on `turns`.
* **H3 (success not worse).** Task-macro success A minus A0; supported when the lower bound of the 95% interval is at or above -0.15.

AS against A (new, two-sided, because no direction is predicted):

* **H4 (skill, success).** Task-macro success AS minus A with its 95% interval. Read as "AS better" when the interval is above 0, "AS worse" when below 0, otherwise no resolvable difference.
* **H5 (skill, turns).** Geometric mean of the per-task ratio of mean turns AS/A with its 95% interval. Read as "AS fewer turns" when the interval is below 1, "AS more turns" when above 1, otherwise no resolvable difference. Tokens AS/A are reported with the same interval, not tested.

All five are reported whatever the outcome, for the primary set, and also for all ten tasks (reported, not tested). Arm B is descriptive only, as before.

### A5.5 Secondary measures

Per task and arm: success with Wilson intervals, score, wall time, turns, computer-use calls, failed calls by tool, tokens (total and by kind), equivalent cost, pointer moved, focus stolen, flags, peeks, and BenchLab covered at start (A4.8). Use of `run_actions` and the lean-read options for A, AS and A0. A descriptive comparison of each arm with the same arm in run v035 (different runs, so not paired and not tested): task-macro success, turns, tokens and cost.

### A5.6 Credentials and stop rule

* Seats chosen from the owner's account switcher (`cswap`) by the headroom in its latest poll, preferring seats other than the owner's active one. Only the access token is copied, never the refresh token, into a 0600 file in the VM read by file descriptor (A1.4). A watchdog on the host reads each reading the runner records and moves to the next seat with headroom when the current one reaches 0.94 on either window (one trial short of the 0.95 line) or is rejected, or when its access token is near expiry, so the runner does not pause; every switch is logged in `runs/v036/account-switch.txt`. The registered stop is unchanged: no new trial once the seat in use is at 0.95 or more on its seven-day window, or on a seven-day `rejected`. A five-hour rejection still waits for its reset if it happens.
* The token file is deleted at the end, before the VM is shut down gracefully.
* Arm B's numbers stay internal until the OpenAI terms review (CUA-1225). No arm B number from this run is committed to this repository; the public-facing summaries use `analyze_v036.py --no-arm-b`.

### A5.7 Harness changes for this amendment

`tools/analyze_v036.py` (the analysis of A5.4 and A5.5); `tools/vm_main_build/` (the build, assembly and TCC scripts used for A3 and A5, kept for the record); `pins.json` `cua_main` (the new build).

## Amendment 6 (8 Oct 2026, before the first mini-run): repeatable mini-runs for the overnight hill-climb

Written and committed before the first trial of any mini-run. It changes nothing about earlier runs. Mini-runs are screening runs: they tell the people fixing the driver where it fails, and they decide when a full pre-registered rerun is worth running. They are never pooled with each other or with any other run, and no claim about Cua Driver against Codex computer use rests on them.

### A6.1 Protocol

* **Tasks and order:** CDB-G02, CDB-G03, CDB-G04, MB-10, MB-11. Three runs each, in one phase (`--phase1-runs 3 --phase2-runs 0`).
* **Arms**, interleaved in every task block. The first arm is `(task position + run index) mod 3` over the list (A, AX, B):

| Label | Runner arm | What |
|---|---|---|
| A | `cc-cua-driver-main` | Cua Driver built from main at the mini-run's commit (the A5.2 recipe, `tools/vm_main_build/`), with that commit's skill as a project skill |
| AX | `cc-cua-driver-script` | the same build, copied into its own app `CuaDriverBenchScript.app` (bundle id `com.trycua.driver.benchscript`, its own TCC rows, socket and daemon state, `tools/vm_main_build/assemble_script.sh`), with `CUA_DRIVER_EXPERIMENTAL_SCRIPT=1` in the daemon's environment and the MCP server's environment, and the `run_script` addendum of `claude_arms.RUN_SCRIPT_ADDENDUM` appended to the system prompt (the text proposed for CUA-1214 in Stream D's arm note) |
| B | `cc-codex-cu` | unchanged |

* **Preflight:** `run_script` must be listed in AX's `tools/list` and absent from A's. Both apps' binary hashes must match `pins.json` `cua_main`. The script app is the same build re-signed under its own identifier, so its file hash is pinned separately (`script_binary_sha256`). Both daemons must report the mini-run's commit.
  * If main at that commit has no `run_script` tool (#4822 not merged yet), AX is left out and the entry says so.
* **Everything else as in v036 (Amendments 4 and 5):**
  * Sonnet 5.5 (Claude Code 2.1.289), 360 s and 45 turns, the shared system prompt, ToolSearch on;
  * the same VM `cdb-h2h`, named Electron bundles, BenchLab occlusion check, pointer parking;
  * the same reset, recorder, sentinel, evaluator isolation, side-door flags and peek scan.
* **Run ids:** `v037a`, `v037b`, and so on. Before its first trial, each mini-run gets one line in A6.5: id, commit, the two binary hashes, the skill tree hash, and arms run.

### A6.2 When a mini-run is started

When a fix is merged to main and logged in the status log of the 0.36 plan, a mini-run is run on that main. Several merges that landed close together share one mini-run.

### A6.3 What is reported per mini-run (`tools/analyze_mini.py`, internal)

* Per arm: successes overall and per task; mean turns and mean equivalent cost; their ratios to arm B.
* The indicative match check (A6.4).
* The most frequent failure classes of the Cua Driver arms, with trial paths: turn cap, failed evaluator checks, and failed tool calls grouped by tool and normalised error text.

Arm B's numbers stay internal (CUA-1225).

### A6.4 When the full rerun is triggered

A mini-run "suggests a match" when, for at least one Cua Driver arm, all three hold on that mini-run:
* successes ≥ arm B's overall;
* successes ≥ arm B's on each of G02, G03 and G04;
* mean turns ≤ 1.2 × arm B's.

Then a full rerun `v037-full` is registered as its own amendment before its first trial:
* the ten v036 tasks, 3 + 2 runs;
* arms A, AX, A0 (0.34.0) and B;
* the definition of done above as its pre-registered decision rule, plus H1 to H3 of A5.4 for A against A0.

Fixes are general driver or guidance changes. Nothing is tuned to a task's evaluator.

### A6.5 Mini-run log (one line per mini-run, written before its first trial)

| Id | Main commit | A binary sha256 | AX binary sha256 | Skill tree sha256 | Arms | Written (UTC) |
|---|---|---|---|---|---|---|
| v037a | `45469a59c931` (#4822 merged: run_script, off by default) | `170dc2a748cb` | `c2e8a11eb1cb` | `668c0235269a` | A, AX, B | 2026-10-07T23:42Z |
| v037b | `4c2403fc5574` (#4861, #4864, #4865, #4881, #4886 since v037-full) | `a23ed88fad78` | `eecef73963aa` | `310811c091a1` | A, AX, B. AX addendum adds one sentence: pass `"timeout_ms": 120000` on every run_script call (the tool's maximum; its default is 30 s). The binary is unchanged | 2026-10-08T15:44Z |
| v037c | `c1c2b5fee428` (#4891 and #4892, plus GNOME-only #4879/#4880, since v037b) | `bf879098fe67` | `5d958f585b68` | `0d60e4f271c9` | A, AX, B; AX addendum as in v037b (timeout_ms 120000). A first launch at 19:29Z on 0188a554d (without #4892) was stopped by the coordinator's request after 1 trial and is kept aside as runs/v037c.aborted-0188, not analysed | 2026-10-08T19:43Z |

### A6.6 Harness changes for this amendment

* `claude_arms.py`:
  * `CuaBuild.daemon_env`, `prompt_addendum` and `binary_pin`;
  * arm `cc-cua-driver-script`;
  * `build_env`;
  * `start_cua_daemon(env_extra=...)`.
* `run_bench.py`:
  * the daemon is started with the build's environment;
  * the `run_script` listed or absent preflight check;
  * per-app binary pins.
* `arms.py`: the new arm.
* `tools/analyze_mini.py`.
* `tools/vm_main_build/assemble_script.sh`, and `tcc_grant.sh` with app and id arguments.
* Tests in `tests/test_bench.py`.

## Amendment 7 (8 Oct 2026, before the first trial of run v037-full): the full rerun triggered by mini-run v037a

Written and committed before the first trial of run `v037-full`. Mini-run v037a (A6.5) met the A6.4 trigger for both Cua Driver arms, with three runs per cell. This is the full pre-registered rerun that decides the overnight definition of done. It changes nothing about earlier runs.

### A7.1 Arms

| Label | Runner arm | Tool layer |
|---|---|---|
| A | `cc-cua-driver-main` | Cua Driver built from trycua/cua main at `7b3a8ee656a62a5d490e47c2dec1f534f2d53aca` (the A5.2 recipe), with that commit's skill as a project skill. Binary sha256 `8a25586ff515b1e3e8a27f07d05a12ed296ff343e45c87f22dcb2f3ecc1896b0`, skill tree `e25749c4eca5282184780935e1bdbf917b86cdcb5bc904195a4d7f0c1a99430b` |
| AX | `cc-cua-driver-script` | the same build in its own app (A6.1), with `CUA_DRIVER_EXPERIMENTAL_SCRIPT=1` in the daemon and MCP environment and the `run_script` addendum in the system prompt. Binary sha256 `4eb5243d7cfb40f29e6cb0726da74097f8c117892ba198909999a59bca3436d0` (the same build, re-signed under its own identifier) |
| A0 | `cc-cua-driver` | Cua Driver 0.34.0, the pinned private copy, as in every earlier run |
| B | `cc-codex-cu` | OpenAI's `cua_repl` launcher, unchanged |

The main commit is the main of the time of writing. It includes everything v037a ran (45469a59c), plus the fixes merged since: #4857 (invoke_menu lists the items of a missing segment), #4854 (a mouse-moved event after a desktop pointer warp), #4858 (common key-name spellings), #4855 (get_window_state query searches the whole window), #4859 (run_actions takes the shapes models send and recovers from stale tokens), #4856 (refuse background submits into a Chromium address bar) and #4862 (accept the invoke_menu and zoom shapes models send). `pins.json` `cua_main` holds these hashes, and the preflight fails if any of them, the commit reported by the daemons, or the `run_script` listing (present in AX only) differs.

### A7.2 Tasks, order, runs, limits

* The ten tasks of v036, in the A3.3 order: CDB-S01, CDB-S04, CDB-G02, CDB-G03, CDB-G04, MB-09, MB-10, MB-11, CDB-S02, CDB-S03.
* Four arms interleaved in every task block. The first arm is `(task position + run index) mod 4` over the list (A, AX, A0, B).
* Phase 1 is runs 1 to 3 of every task. Phase 2 is runs 4 and 5, only when phase 1 is complete and the stop rule and cutoff allow it. Only complete task blocks are analysed.
* `CUTOFF_UTC` for new trials: 2026-10-08T16:00:00Z.
* Everything else is as in v036 (Amendments 4 and 5): Sonnet 5.5 (Claude Code 2.1.289), 360 s and 45 turns, the shared system prompt, ToolSearch on, the VM `cdb-h2h`, named Electron bundles, the BenchLab occlusion check, pointer parking, reset, recorder, sentinel, evaluator isolation, side-door flags and the peek scan.

### A7.3 Seats and stop rule

* As in A5.6: cswap seats, access tokens only, a host watchdog that switches seats at 0.94 on either window and spares the owner's active seat and account 7 when others have headroom.
* The registered stop is unchanged: no new trial once the seat in use is at 0.95 or more on its seven-day window, or on a seven-day `rejected`.
* At the end of the run the token file is deleted, with sync, before a graceful shutdown.

### A7.4 Decision rule (the overnight definition of done), registered before any trial

Over all counted trials of complete blocks:

1. **The best Cua Driver arm** is whichever of A and AX has the higher share of passed trials over the ten tasks; a tie goes to the arm with fewer mean turns. Choosing it on the same data favours Cua Driver slightly. That is part of the owner's definition and is reported as such.
2. **Done** when all three hold for that arm:
   * its share of passed trials is at least arm B's;
   * on each of CDB-G02, CDB-G03 and CDB-G04, its share of passed trials is at least arm B's;
   * its mean turns per trial are at most 1.2 × arm B's.

These are point comparisons with no interval. The report gives the per-task numbers next to them, and says that five runs per cell cannot resolve small differences.

### A7.5 Other analyses

* H1 to H3 of A5.4 (A against A0, GUI-heavy set, same tests).
* AX against A, descriptive: task-macro success difference and turns ratio, with the A5.4 bootstrap.
* Per task and arm: success, score, turns, wall time, tokens and cost.
* The A6.3 failure classes.

These are produced by `tools/analyze_v037.py`, written before the first trial. Arm B's numbers stay internal (CUA-1225). Only the yes/no outcome of the decision rule and the Cua Driver arms' own numbers may go into anything shared outside the team.

### A7.7 Restart before any trial completed (recorded 8 Oct 2026, 02:30 UTC)

The first launch at 02:12 UTC passed every preflight check. At 02:14:56, during the first trial, it was stopped by a quoting bug in Stream C's host-side post-processing script. That script stopped the VM because it wrongly concluded the run had ended. No trial completed and `results.jsonl` was empty. The run folder was kept as `runs/v037-full.aborted-0214`, and the run was relaunched under the same id, build, arms, order and rules. No rule changed.

### A7.8 Seat handling during the run (recorded 8 Oct 2026; no rule of the analysis changes)

* **05:51 UTC.** The host watchdog refreshed the access token from the Mac's live Claude Code login. cswap had just switched that login from account 6 to account 7, so about two trials ran on account 7 while the log said account 6. The rows' own quota readings show it. That route was removed: from then on, tokens come only from the switcher's per-account items, and are always access tokens only.
* **05:58 to 07:21 UTC.** Account 4's five-hour window was full. The runner waited for its reset under the registered rule. One attempt was an `infra_error` and was rerun.
* **From about 08:10 UTC.** At the owner's instruction, any seat with headroom may be used, most headroom first. A seat is left at five-hour 0.94, or at seven-day 0.92 for account 6 and 0.94 for the others, or on any rejection, without pausing. The run waits only if no seat qualifies.
* Every switch is in `runs/v037-full/account-switch.txt`.

### A7.6 Harness changes for this amendment

`tools/analyze_v037.py`; `pins.json` `cua_main` (this build).

## Amendment 8 (9 Oct 2026, before the first trial of run v038): second full rerun on main with the 0.36 hill-climb fixes

Written and committed before the first trial of run `v038`, at the owner's approval (8 Oct). Mini-run v037c (A6.5) met the A6.4 trigger for arm A. Since v037-full, main has gained fixes for the failure classes logged after each mini-run. This rerun repeats Amendment 7 on that main, under the same decision rule. It changes nothing about earlier runs.

### A8.1 Arms

| Label | Runner arm | Tool layer |
|---|---|---|
| A | `cc-cua-driver-main` | Cua Driver built from trycua/cua main at `5a364bbe60e1f8a901ceacd889606b6367dc96ab` (the A5.2 recipe), with that commit's skill as a project skill. Binary sha256 `b2a12eca1eca8f258d1da38ff496ca9d259e221190a0f4fd448ac817e223fd62`, skill tree `5f7ee957e347ef743d1f47a2d75e0dcd919c5a480473f92aff2717b3e34588cc` |
| AX | `cc-cua-driver-script` | the same build in its own app (A6.1), with `CUA_DRIVER_EXPERIMENTAL_SCRIPT=1` in the daemon and MCP environment and the `run_script` addendum in the system prompt, including the v037b sentence asking for `"timeout_ms": 120000` on every call. Binary sha256 `1e7d4295a3c4826bc1371590f2b85567f32534833f1cfe070afe7a27ceb3ccb7` (the same build, re-signed under its own identifier) |
| A0 | `cc-cua-driver` | Cua Driver 0.34.0, the pinned private copy, as in every earlier run |
| B | `cc-codex-cu` | OpenAI's `cua_repl` launcher, unchanged |

The main commit is the main of the time of writing, at or after the merge of #4897 (keeps a failed run_actions report readable within client error limits). Since v037-full (7b3a8ee65) it adds #4861, #4864, #4865, #4881, #4886, #4891, #4892 and #4897 (plus GNOME-only #4879 and #4880). `pins.json` `cua_main` holds these hashes, and the preflight fails if any of them, the commit reported by the daemons, or the `run_script` listing (present in AX only) differs.

### A8.2 Tasks, order, runs, limits

* The ten tasks of v036, in the A3.3 order: CDB-S01, CDB-S04, CDB-G02, CDB-G03, CDB-G04, MB-09, MB-10, MB-11, CDB-S02, CDB-S03.
* Four arms interleaved in every task block. The first arm is `(task position + run index) mod 4` over the list (A, AX, A0, B).
* Phase 1 is runs 1 to 3 of every task. Phase 2 is runs 4 and 5, only when phase 1 is complete and the stop rule and cutoff allow it. Only complete task blocks are analysed.
* `2026-10-09T18:00:00Z_UTC` for new trials: 2026-10-09T18:00:00Z.
* Everything else is as in v036 (Amendments 4 and 5): Sonnet 5.5 (Claude Code 2.1.289), 360 s and 45 turns, the shared system prompt, ToolSearch on, the VM `cdb-h2h`, named Electron bundles, the BenchLab occlusion check, pointer parking, reset, recorder, sentinel, evaluator isolation, side-door flags and the peek scan.

### A8.3 Seats and stop rule

* As in A7.8: cswap seats, access tokens only from the switcher's per-account items, most headroom first (accounts 1, 4, 5 and 7 preferred). A seat is left at five-hour 0.94, or at seven-day 0.92 for account 6 and 0.94 for the others, or on any rejection, without pausing; the run waits only if no seat qualifies.
* The registered stop is unchanged: no new trial once the seat in use is at 0.95 or more on its seven-day window, or on a seven-day `rejected`.
* At the end of the run the token file is deleted, with sync, before a graceful shutdown.

### A8.4 Decision rule (the overnight definition of done), registered before any trial

Over all counted trials of complete blocks:

1. **The best Cua Driver arm** is whichever of A and AX has the higher share of passed trials over the ten tasks; a tie goes to the arm with fewer mean turns. Choosing it on the same data favours Cua Driver slightly. That is part of the owner's definition and is reported as such.
2. **Done** when all three hold for that arm:
   * its share of passed trials is at least arm B's;
   * on each of CDB-G02, CDB-G03 and CDB-G04, its share of passed trials is at least arm B's;
   * its mean turns per trial are at most 1.2 × arm B's.

These are point comparisons with no interval. The report gives the per-task numbers next to them, and says that five runs per cell cannot resolve small differences.

### A8.5 Other analyses

* H1 to H3 of A5.4 (A against A0, GUI-heavy set, same tests).
* AX against A, descriptive: task-macro success difference and turns ratio, with the A5.4 bootstrap.
* Per task and arm: success, score, turns, wall time, tokens and cost.
* The A6.3 failure classes.

These are produced by `tools/analyze_v037.py` (the A7.4 rule, unchanged; written before v037-full). Arm B's numbers stay internal (CUA-1225). Only the yes/no outcome of the decision rule and the Cua Driver arms' own numbers may go into anything shared outside the team.

### A8.6 Harness changes for this amendment

`pins.json` `cua_main` (this build). No code change.

### A8.7 A follow-on arm

An arc-cua arm (`cc-arc-driver`, CUA-1241) may later run the same tasks on the same build. It needs its own amendment, written before its trials, and it is reported against this run's arms as a follow-on, not pooled into this run's decision.

## 0. Decisions made before the first trial, and why

These were fixed before any analysed trial. Several came from the owner during the build phase.

1. Model. Both arms run `claude -p --model claude-sonnet-5-5`. The first plan used Opus 5.5 (the public claim that
   motivated this work used Opus 5.5). The owner switched to Sonnet 5.5 for cost before any trial ran. This is a
   deliberate difference from the public claim, and the report says so. An Opus 5.5 confirmation subset may be run at the
   end if time remains; it is labelled a supplement and never mixed into the headline.
2. Run count. The target is five runs per task per arm. The guaranteed minimum is three runs per arm for each task
   that is reported. Runs are scheduled as task blocks, in priority order, with the two arms interleaved inside
   the block (section 6), so whatever completes is balanced. Runs 4 and 5 are scheduled only after every task has its
   first three runs, and only while the quota rule in item 3 allows. Only complete task blocks enter the analysis.
3. No dollar cap, but a quota cap. `claude -p` runs on a subscription seat, so there is no per-token bill. The seat was
   at 0.89 of its 7-day quota at 22:50 UTC on 5 Oct and 0.94 at 23:18 UTC (reset 7 Oct 21:00 UTC; the 5-hour window
   resets at 03:30 UTC), and burning it to the cap
   would lock the owner out for about two days. The runner therefore stops starting new trials when the 7-day
   utilization reported in the stream's `rate_limit_event` reaches 0.95, or on any `rejected` status. The trial in flight
   finishes. Tokens and the list-price equivalent cost (`total_cost_usd` from `--output-format json`) are recorded for
   every trial and reported, because readers will ask. The 5-hour window is handled by waiting (section 6).
4. Arm A uses the official Cua Driver 0.34.0 release exactly (section 2). The installed 0.28.2 and the 0.33.4
   development build are not used.
5. Tasks (section 4) were written before any trial. The eight "AF" tasks are our reconstruction from a public
   description. The original eight tasks were never published.
6. Limits. 360 s wall time and 45 turns for every task, the same for both arms. The first plan was 240 s and 30 turns. A
   plumbing run on a small model showed that Claude Code's tool-schema loading (ToolSearch) and skill reads can use about
   half of a 30-turn cap in arm A before the first action, which would bias the comparison. The change was made before any
   analysed trial. Turns and tool calls are both reported.
7. The real run is gated and has not started. The launcher refuses to run while a `HOLD` file exists or the 7-day utilization
   is at or above 0.95. It starts when the owner removes `HOLD` after the quota resets, or on a credential the owner supplies.
   Nothing analysed has run yet. Harness plumbing tests used two Haiku 4.5 trials and three Sonnet 5.5 trials of MB-05
   (an accidental use of the real binary while testing the runner). They are listed in the ledger, marked smoke, and are
   never analysed.

## 1. Questions

Q1 (tool layer). Same model, same agent harness, same prompt and budgets: does the Codex computer-use tool layer
complete macOS desktop tasks as reliably, as fast and as cheaply as the Cua Driver MCP tool layer?

Q2 (disturbance). While the agent works, does either tool layer take focus, leak keystrokes or clicks into the
frontmost app, or move the real pointer?

Q3 (the public claim). A post of 3 Oct 2026 says Opus 5.5 on Codex's engine scored 6/8, Codex 6/8 and Cua Driver 3-4/8
on an unpublished 8-task test, and that Cua Driver's background mode cannot land canvas clicks or drags. Does an
open, rerunnable test of the abilities the post names (custom-drawn clicks, drags, scroll, right-click) reproduce the
gap, in either direction?

Nothing here claims which system is better. The analysis is fixed below so a result in either direction is
reported the same way.

## 2. Arms (the intended difference is the computer-use tool layer and, for arm A, its skill)

| Held constant | Value |
| --- | --- |
| Agent harness | Claude Code CLI `claude -p` (version in `manifest.json`), `--output-format stream-json` |
| Model | `claude-sonnet-5-5`, default effort, both arms |
| System prompt | One shared text (sha256 in `manifest.json`), replacing the default Claude Code prompt |
| Built-in tools | The same fixed list in both arms (`Skill`, `Read`, `ToolSearch`); no shell, no file edit, no web |
| Settings | No user CLAUDE.md, auto-memory, hooks, plugins, slash commands or other MCP servers |
| Step limit | `--max-turns` per task (section 4) |
| Wall-time limit | Per task (section 4); the process group is killed at the limit |
| Machine | One Mac, same display and OS build, trials serial, no human at the desk |

| Arm | Tool layer | Vendor guidance |
| --- | --- | --- |
| A `cc-cua-driver` | Cua Driver 0.34.0, the official release (`cua-driver-rs-v0.34.0`, published 5 Oct 2026), artifact `cua-driver-rs-0.34.0-darwin-universal.tar.gz` (sha256 `2d0ade531c07b4d16e8078844fe1b63a0dfa0ee19677c9dcc079b4d3460ab387`), run as a private app and daemon, MCP server called by absolute path. `cua-driver --version` prints `cua-driver 0.34.0`; binary sha256 `54e5d48128fb62323366c49b6be480ccef83cdee23bd7fc1f19431347a5df86b`; release git sha `b0968e1b12834e485dda68789541a3cc57664a9f`. The preflight fails unless the binary arm A invokes reports exactly 0.34.0. The installed 0.28.2 and the 0.33.4 development build are not used. | The Cua Driver skill from the 0.34.0 release (`cua-driver-rs-v0.34.0-skills.tar.gz`, sha256 `4ea2bfc2d833af791c8a6d9c5904f468bd35552728b4608e151b106ace02292f`), delivered through Claude Code's project-skill mechanism |
| B `cc-codex-cu` | Codex computer use through OpenAI's shipped `cua_repl` launcher from the `unified-computer-use` plugin of the ChatGPT/Codex app, unchanged, registered in Claude Code as MCP server `codex-cu` (section 10) | None (the Codex computer-use skill is not delivered; see confounds) |

Arm A is the only arm with a skill. This follows the request for the benchmark ("Cua Driver MCP plus the Cua Driver
skill"). The cost of reading the skill is part of arm A's measured tokens and time.

Versions pinned in `manifest.json` and checked by the preflight: Claude Code 2.1.289; macOS 26.6.1 (25G76);
ChatGPT/Codex app 26.930.51102 (`com.openai.codex`), its `unified-computer-use` plugin 26.930.51102, Codex Computer Use
service 26.929.1001365 (`com.openai.sky.CUAService`). The ChatGPT app can update itself during the run; the runner
re-reads these versions before and after every trial and flags any change.

Confounds reported, not controlled: arm A carries a long skill (about 160 KB across its files); Codex's API has no
hover or pointer-move primitive; Cua Driver exposes browser tools that Codex's surface does not (not used by these
tasks); the MCP servers differ in tool names and result formats; the tasks were written by the Cua team (section 11).

## 3. Isolation and safety

* No human is present overnight. The runner records the HID idle time at the start of each trial anyway.
* `BenchSentinel`, a small native window, is frontmost before the agent starts (except where a task states
  otherwise). During the agent phase it logs focus changes, keystrokes and clicks delivered to it, HID-level input and the
  real pointer position at 20 Hz.
* Task apps run with a throwaway HOME; no browser profile of the owner is used; nothing leaves the machine except the
  model API calls of Claude Code itself.
* Evaluators run outside the agent's reach (the agent has no shell and no file-read of the evaluator paths; the Read
  tool, if present, is scoped by the permission rules to the skill directory only; see `manifest.json`).
* Every trial is screen-recorded (H.264, plus a 720p copy).

## 4. Tasks (frozen list)

See `TASKS.md` for the exact prompts, setup, checkers, limits and validation evidence. Summary:

| ID | Task | Group | Ability | Wall (s) | Max turns | Coverage caveat |
|---|---|---|---|---|---|---|
| MB-01 | Click 12 numbered dots in order on a custom-drawn board | AF | canvas_left_click | 360 | 45 | none |
| MB-02 | Right-click 4 dots on a custom-drawn board and choose the stated menu action | AF | canvas_right_click | 360 | 45 | none |
| MB-03 | Scroll a 400-row table, find one row by Name and Qty, select it, Confirm | AF | scroll | 360 | 45 | none |
| MB-04 | Drag three tiles to their zones on a custom-drawn board | AF | canvas_drag | 360 | 45 | none |
| MB-05 | Fill and submit a native invoice form | AF | form_fill | 360 | 45 | none |
| MB-06 | Type a paragraph, bold one sentence, replace one word in a rich text editor | AF | text_editing | 360 | 45 | none |
| MB-07 | Compute in Calculator, copy the result, paste it into BenchLab and save | AF | multi_app_clipboard | 360 | 45 | none |
| MB-08 | Open Settings from the menu bar, change three settings, apply, confirm in the main window | AF | multi_window_menu | 360 | 45 | none |
| MB-09 | Reorder a native list by drag and drop | P | list_drag_drop | 360 | 45 | none |
| MB-10 | Reveal a hover-only toolbar and click one action | P | hover_menu | 360 | 45 | hover: no Codex tool, report separately |
| MB-11 | Read a native tooltip and type its code | P | tooltip | 360 | 45 | hover: no Codex tool, report separately |
| MB-12 | Fill a form in a background app while another app stays frontmost and the pointer stays put | P | background_operation | 360 | 45 | none |
| MB-01 | yes | yes | yes | yes: ['left_sequence_in_order', 'no_extra_left_clicks'] | 1.12 / 0.058 / 0.06 | 4.8 |
| MB-02 | yes | yes | yes | yes: ['target_2_action'] | 1.14 / 0.093 / 0.07 | 10.2 |
| MB-03 | yes | yes | yes | yes: ['no_wrong_confirm', 'selected_target_at_confirm', 'selection_events_consistent', 'state_matches'] | 1.24 / 0.073 / 0.07 | 8.8 |
| MB-04 | yes | yes | yes | yes: ['green_in_zone', 'placed_when_done', 'red_in_zone'] | 1.13 / 0.085 / 0.09 | 6.3 |
| MB-05 | yes | yes | yes | yes: ['category'] | 1.12 / 0.047 / 0.05 | 7.1 |
| MB-06 | yes | yes | yes | yes: ['bold_covers_sentence', 'no_bold_elsewhere'] | 1.02 / 0.046 / 0.05 | 8.9 |
| MB-07 | yes | yes | yes | yes: ['result_correct'] | 1.02 / 0.053 / 0.14 | 6.7 |
| MB-08 | yes | yes | yes | yes: ['applied_theme', 'ticket_correct'] | 1.12 / 0.043 / 0.05 | 8.3 |
| MB-09 | yes | yes | yes | yes: ['done_order_correct', 'order_correct'] | 1.12 / 0.047 / 0.05 | 8.0 |
| MB-10 | yes | yes | yes | yes: ['clicked_target', 'no_wrong_clicks'] | 1.12 / 0.048 / 0.05 | 3.8 |
| MB-11 | yes | yes | yes | yes: ['code_correct', 'tooltip_displayed_for_target'] | 1.12 / 0.046 / 0.05 | 6.9 |
| MB-12 | not run | not run | not run | not run | not run | not run |

Group AF is our reconstruction of the public description (canvas clicks, right clicks, scroll, drag, forms, text, multi-app and menus). Group P probes the disputed abilities. In group AF the tasks are ours, not the author's.

Per-run seeds are `sha256(task_id:run_index) mod 10^6` (run index from 0), identical for both arms for the same run
index, different across run indexes and tasks. Checkers read an event log or document state written by the app from real UI events and recompute the expected
values from the seed. No task is graded by an LLM.

## 5. Headline and secondary sets

* Headline (Q3): the eight AF tasks, MB-01 to MB-08, equal weight in the task-macro mean (over the AF tasks that have
  complete blocks if the quota stops the run early). This is the closest
  open equivalent of the public 8-task claim.
* Secondary: MB-09 to MB-12 (drag-and-drop in a native list, hover menu, tooltip, background operation) reported per
  task. MB-10 and MB-11 are coverage items: Codex's API has no hover primitive, so a failure there is a coverage fact.
  The all-tasks mean over all twelve is also reported.
* MB-13 (CDB-S01 from the private bench) is not part of this run. It needs a coding step and 20 to 35 tool calls (the private bench allows 1200 to 1800 s), uses a browser and an Electron app, and its evaluator secrecy and network denial cannot be enforced on one shared account. At a 360 s limit it would mostly measure the timeout. 

## 6. Procedure

* Order. The unit of work is a task block. Tasks run in priority order: MB-01 to MB-08 (the AF reconstruction) first,
  then MB-09 to MB-12 (the disputed-ability probes), then MB-13 (CDB-S01) if it is part of the run. Phase 1: for each
  task in that order, run 1, 2 and 3 back to back, and inside each run the two arms run back to back. For task i
  (0-based, in priority order) and run j (1-based) the first arm is A if (i + j) is even, otherwise B, so both arms are
  balanced over time. Phase 2, only after phase 1 is complete for every task and only while the quota rule allows:
  runs 4 and 5 as task blocks in the same order and with the same alternation. Seeds are in section 4.
* Completeness. A task block is complete when both arms have their scheduled runs with a valid outcome. A block cut
  off by a stop rule is kept in `results.jsonl`, flagged `block_incomplete`, listed in the report appendix, and never
  aggregated into a result. If the quota stops the run before all AF tasks have blocks, the headline is the
  macro mean over the AF tasks that have complete blocks, and the report says which are missing.
* Cutoff: a UTC time set at launch, written to `manifest.json` before trial 1, after which no new trial starts.
* Launch. The runner is started in a shell spawned by Terminal.app (`open -a Terminal launch_bench.command`) and
  every `claude -p` of both arms is its child with the same scrubbed environment (HOME, PATH, USER, LANG, TMPDIR,
  TERM only), so the launch context is identical for both arms.
* One attempt per trial; the agent gets no retries or hints.
* Verified infrastructure failures (MCP server failed to start, provider 5xx, auth) are marked `infra_failure`,
  retried (up to 3 times with backoff) and excluded from outcome statistics only if all retries fail; they are counted
  and reported per arm and per task. Agent failures, timeouts, refusals and out-of-turns count as failures.
* Rate limits. If the 5-hour window is exhausted the trial is not counted, the runner waits until the reset time
  in the event (about 03:30 UTC) with exponential backoff as a fallback (60 s up to 15 min), then retries the same
  trial, provided the 7-day utilization is below 0.95. The arm interleaving and block order are not changed. Every pause is logged to
  `pauses.jsonl` and listed in the report. Utilization before and after each trial is stored in every results row and in
  `heartbeat.json`, and the report states it.
* Smoke and validation trials run before this file was committed are never analysed. They are listed in the report appendix
  with their outcomes and cost.
* Any change to a task, checker, prompt or arm after the first analysed trial is a protocol deviation: it is logged in
  `AMENDMENTS.md` with its time and reason and the report lists it. Bug fixes in a checker are re-applied to all rows
  by re-running the checker on stored event logs.

## 7. Metrics

Primary: task success (checker `passed`), per task and arm over the 5 runs; headline task-macro mean over MB-01..08.

Secondary: checker score; agent-phase wall time; turns; tool calls (by tool); input, output, cache-read and cache-write
tokens; list-price equivalent cost; time to first action; MCP/tool errors; skill reads (arm A).

Disturbance: frontmost-app changes (yes/no and count), real pointer moved (yes/no, max excursion in px), keystrokes and
clicks leaked to the sentinel. For MB-12 disturbance is part of pass; elsewhere it is a separate metric.

Cost: reported as equivalent list-price cost from `total_cost_usd`; no real money is billed (subscription).

## 8. Analysis plan

`analyze.py` on `results.jsonl`, default seed. Per cell (task x arm): successes out of n (3 to 5), Wilson 95 percent interval.
Headline: paired difference of task-macro success (A minus B) with a hierarchical bootstrap (resample tasks, then runs)
95 percent interval. Per task: descriptive Fisher exact p-values without multiplicity correction. Time, tokens and
cost: mean, standard deviation and median per cell; paired by task and run where both arms have a valid trial.
Variance of success across runs is shown as the raw pass vector (3 to 5 runs per cell). If the headline interval contains 0 the verdict
is "no resolvable difference". Failures are never dropped. Excluded trials are counted per arm and task.

## 9. Stopping rules

Stop starting new trials, finish the one in flight, and report completed task blocks only, when any of these holds:
the 7-day utilization in `rate_limit_event` reaches 0.95; any `rate_limit_event` status is `rejected` for the 7-day
limit; the cutoff passes; an arm cannot start (preflight failure); more than 25 percent of the last 20 trials hit
infrastructure failures; or the STOP file exists. A 5-hour-window rejection is not a stop: the runner waits for the reset
and continues. There is no dollar cap. No other credentials or API keys are used.

## 10. Arm B availability

Arm B uses the route OpenAI ships for its own agents, started by Claude Code instead of Codex:

* Launcher. The `cua_repl` entry of `~/.codex/plugins/cache/openai-bundled/unified-computer-use/26.930.51102/.mcp.json`:
  the app's bundled `node` (`/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node`) running
  `@oai/cua-repl/bin/cua-repl.mjs`, with the plugin's own environment, unmodified. The only change is
  `CUA_REPL_ENABLED_SURFACES=computer`, which switches off the browser surface so Chrome cannot be touched. The server
  talks to OpenAI's Sky computer-use service over its own unix socket. Claude Code reserves the MCP name
  `computer-use`, so the server is registered as `codex-cu`.
* Tools the model sees: `js` and `js_reset`, a JavaScript REPL with a `cua` object (`listApps`, `getApp`, then click,
  typeText, pressKey, scroll, drag, setValue, selectText, performSecondaryAction, paste, accessibility state). One `js`
  call can batch many actions. There is no hover or pointer-move call. Arm A's tools are discrete MCP calls, so the two
  arms differ in call granularity; the report counts both tool calls and model turns.
* Approvals. The service asks the host to approve each app through an MCP elicitation (`Allow Computer Use to use
  "<App>"?`). Plain `claude -p` declines these. The runner sends the prompt through Claude Code's stream-json input and
  accepts the request only for the apps the task names (BenchLab, and Calculator for MB-07) and declines everything else.
  Both arms use this same host; arm A never raises such requests.
* Not used, and why. The typed client `SkyComputerUseClient mcp` returns `Sender process is not authenticated`
  (error -10000) when its parent process is not OpenAI-signed. The client ships a parent code requirement for OpenAI's
  team identifier, and the strings of the client and service name an untrusted-parent failure reason. We did not
  try to get around it: no spoofing, no injection, no re-signing, no parent-process tricks. The `cua_repl` route needs
  none of that.
* Terms. The plugin manifest says `"license": "Proprietary"`, and driving this interface from a non-OpenAI client is
  undocumented. Nobody has reviewed OpenAI's terms on this use or on publishing benchmark results. That review is
  required before the results are published or quoted outside Cua.
* Fragility. The launcher path contains the plugin version, which changes with ChatGPT updates; all clients share one
  Sky service (the benchmark never runs two trials at once).

Supplement S1 (optional, not part of the headline, decided here so it cannot be cherry-picked): after the Claude trials
stop, if time remains, the same tasks may be run with Codex on both sides (Codex's own built-in computer use through
`codex app-server`, versus Codex with the Cua Driver 0.34.0 MCP through `codex exec`, model `gpt-6-astra`). It uses a
different quota pool and a different harness, so it answers a different question and is reported in its own section.

## 11. Fairness, known weaknesses, and what this does not show

* The tasks were written by the Cua team, against a test app we wrote. Mitigations: the checkers are deterministic, the
  prompts are tool-neutral, the AF tasks follow the public description, hover and tooltip are labelled as coverage, and
  everything is public (tasks, apps, harness, prompts, seeds, raw logs, videos).
* One model (Sonnet 5.5), one machine, one OS build, one night. Not a claim about Opus 5.5, about other apps, or about
  Windows or Linux.
* The arms use different MCP servers with different tool schemas, and arm A also carries a skill; arm B has none.
* Run-to-run variance is real; three to five runs per cell give wide intervals per task.
* OpenAI's terms on benchmarking and on using the computer-use service outside their products have not been
  reviewed by a lawyer. Any use of OpenAI's service in this run goes through paths OpenAI ships. See section 10.

## 12. Reporting commitments

All analysed trials, including failures, timeouts and exclusions, are reported with raw per-run values and a video
for each. The report states what the evidence does not support. Results are published with the harness, prompts and
seeds, not only the table. Bugs found in Cua Driver are filed with repro steps.
