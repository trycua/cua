# Bench v2 method and pre-registration summary

This is the public summary of how Cua Driver Bench v2 is run and analysed. It
restates the rules of the pre-registration that applies to Bench v2. The
pre-registration itself, with every amendment, lives in the git history listed
under [Registration record](#registration-record); this summary adds no rule.

## Registration record

The protocol was committed before the first trial, and every change since then
is a dated amendment committed before the trials it covers. The full text is
`PREREGISTRATION.md` at the commits below, on the harness's working branch
`bench/macos-head-to-head`. That branch now lives in a private Cua repository;
each registration commit is tagged there (`cdb-prereg/*`). The commits and the
pre-registration text are verifiable on request: ask Cua for the commit objects,
and check the hashes and commit times below against them. Times are the commit
times in UTC. Until 11 Oct 2026 the branch was also public on `trycua/cua`, so
GitHub's push history there is an independent record for the commits listed up
to that date.

| Commit | Time (UTC) | What it registered |
| --- | --- | --- |
| `231b8dc9d42528ac52c1ee8a93b6301c7c0465b0` | 2026-10-05 23:35 | The original pre-registration (Bench v1), before any trial |
| `d1bdc834019519960f90be3be909835b7af55797` | 2026-10-10 13:11 | Amendment 14: Bench v2 (GUI-only tasks, categories, background score, interruption tasks), before any v2 trial |
| `f10a352f064e02351923cdfbe5fffbe70862d964` | 2026-10-10 13:54 | Amendment 15: the background condition and background completion, before any scored v2 run |
| `6ebd5ce87d7c3c7db0d4e9acb00694fbbea95e36` | 2026-10-10 18:21 | Amendment 16: smoke checks of a Cua Driver key-focus fix |
| `55efe719db3e8ce4bcf9ca4ee4647ba579357fe0` | 2026-10-10 21:14 | A16.3: the first full Bench v2 run |
| `f554cb9f0086c5dd5cca34d75d2101026ab568b5` | 2026-10-10 22:27 | A16.3 stopped after 35 trials and marked superseded |
| `6374da3b485e885ef6a02fc7424c1081b2b0fb0a` | 2026-10-10 23:19 | Amendment 17: smoke checks of a Cua Driver window-order fix, and the rerun |
| `b74564efd4fc657567be6acf011dabbdbf72f43c` | 2026-10-11 02:45 | A17.3: the Bench v2 rerun, both conditions, registered before its first trial |
| `69229a31c55f6693cbab20ac48287d2b17d42bac` | 2026-10-11 09:10 | A17.3 outcome |
| `f7f65e4a281f685817ce71e29d1d9fd704684b27` | 2026-10-11 11:51 | Amendment 20: Codex Computer-Use (Sky) driven by the Codex CLI and `gpt-6-astra`, before any trial of that setup |
| `91a758aa914a5a4a671eb338c173fecc81cbc760` | 2026-10-11 14:54 | Amendment 22: background completion means the user was undisturbed (multi-player rule), the headline from the next full run |
| `c77413bbbd45eca33a34f722c197d35a51860900` | 2026-10-11 15:08 | Amendment 23: Codex CLI as a second harness (Cua Driver under Codex CLI), and the Claude helper setup made public |
| `47fd21ff82d2ebfc732442872508673b46a828d6` | 2026-10-11 15:12 | A23.1: the Codex CLI model is GPT-6.1 Sol if accessible, else `gpt-6-astra` |
| `ad49a3c1a8e373834fbe78ce53c82aeb4dd504be` | 2026-10-11 15:18 | A23: the Claude helper also runs under Codex CLI (`cx-claude-cu-helper`), the full 2 x 3 matrix |
| `a1036e6aac3f17ce15256e88cae7f1410e5eddd6` | 2026-10-11 15:22 | A23.1: GPT-6.1 Sol (`gpt-6.1-sol`, effort low) for the Codex CLI setups |
| The `trycua/cua` commit that adds the shared-desktop amendment to this file | 2026-11-06 | Shared-desktop amendment: PA-01 and UM-01 with their scoring rules, before the first scored trial of either task (in this file, [below](#shared-desktop-tasks-amendment-of-6-nov-2026)) |
| The `trycua/cua` commit that adds the hard-variants amendment to this file | 2026-11-06 | Hard-variants amendment: PA-02 and UM-02 with their scoring rules, before the first scored trial of either task (in this file, [below](#shared-desktop-hard-variants-amendment-of-6-nov-2026)) |

Smoke runs (Amendments 14 to 17) validate the harness and are never analysed.
Amendments 1 to 13 cover Bench v1, which used different tasks and tools; Bench
v2 results are never pooled with them.

**Coming next, each registered before its first trial:** a larger Bench v2 task
set, the final full run with the Amendment 20 and 23 setups, and a run on a sealed
held-out task set.

**Why a summary rather than the full text here.** The full pre-registration
records operational details of Cua's own test infrastructure and setups that
are not part of this public release. Rather than ship a redacted copy, which
would be a different document from the one registered, this summary restates
the Bench v2 rules and points to the unedited, dated originals.

## Question

With the same agent, model, prompts and limits, how often does each computer-use
tool layer get a desktop task done, and can it do the work in the background
while the user keeps using the front window?

## Platform

macOS only for now. The registered runs used one Lume macOS 26.5.2 VM (Apple
silicon, 6 CPU, 12 GB, 1920 x 1080) on a separate Mac Studio, never on anyone's
working desktop. Apps and workspaces are reset before every trial; the VM is
not restored between trials. Trials run one at a time.

## Agent and limits

Every setup runs Claude Code 2.1.289 (`claude -p`) with Claude Sonnet 5.5 at
default effort, the same system prompt, `--strict-mcp-config`, ToolSearch on,
and only the built-in tools `Skill`, `Read` (restricted to the skill directory)
and `ToolSearch`, plus the setup's own computer-use MCP server. Each trial gets
360 seconds and 45 turns. See [`automated-eval/macos_bench/RUNNER.md`](automated-eval/macos_bench/RUNNER.md).
The Codex CLI setups of Amendment 23 run the Codex CLI bundled with the ChatGPT
app as `codex app-server` instead, with the same prompts, limits, GUI-only guard
and checkers.

## Setups

| Setup | Arm id | What changes |
| --- | --- | --- |
| Cua Driver | `cc-cua-driver-script` | Cua Driver built from a pinned main commit, with its skill and `run_script` on (the default from Cua Driver 0.35) |
| Codex Computer-Use (Sky) | `cc-codex-cu` | OpenAI's `cua_repl` launcher from the ChatGPT app 26.930 plugin, used unmodified except that the browser surface is off, through an MCP bridge. Claude chooses the actions. No skill ships with it. |
| Claude Computer-Use (Desktop helper): Claude Desktop 2.31226.0 computer-use helper via a minimal adapter; an internal interface, not an Anthropic-supported procedure | `cc-claude-cu-helper` | Claude Desktop 2.31226.0's `app-cu-helper`, used unmodified, behind Cua's minimal MCP adapter, which adds a window list and window screenshots. |
| Cua Driver, driven by Codex CLI | `cx-cua-driver` (Amendment 23, not yet run) | The `cc-cua-driver-script` build, daemon, MCP entry and system prompt, driven by the Codex CLI bundled with the ChatGPT app (`codex app-server`), without skill files. |
| Codex Computer-Use (Sky), driven by Codex CLI | `cx-codex-cu` (Amendments 20 and 23, not yet run) | The same `cua_repl` tool layer, driven by OpenAI's own agent (the Codex CLI bundled with the ChatGPT app, run as `codex app-server`) and model, with the same prompts, approvals, budget, GUI-only guard and checkers. |
| Claude Computer-Use (Desktop helper), driven by Codex CLI (same label and caveat) | `cx-claude-cu-helper` (Amendment 23, not yet run) | The `cc-claude-cu-helper` helper, adapter and MCP entry, driven by the Codex CLI. |

The Codex CLI setups (Amendment 23) use GPT-6.1 Sol (`gpt-6.1-sol`, default effort low). The
model is recorded in every row and named in the label; no numbers exist yet for
these setups. Internal baselines (other Cua Driver builds) may be run; they
are reported as baselines, never as headline setups.

## Tasks and categories

Ten tasks, listed in [`automated-eval/macos_bench/TASKS.md`](automated-eval/macos_bench/TASKS.md):
three multi-app workflows (CDB-G02 to CDB-G04), three precision interactions
(MB-09 to MB-11) and four interruptions (IR-01 to IR-04). Every task has one
category. A fourth category, Web apps & forms, has no task yet and is reported
as "no tasks".

The interruption tasks show a permission-style prompt, a notification-style
banner, an unsaved-changes dialog or a cookie-consent overlay mid-task. Every
brief ends with the same sentence asking the agent to handle anything that comes
up as a careful person would. A trial passes only if the task is done **and**
the interruption is handled safely.

### Shared-desktop tasks (amendment of 6 Nov 2026)

**Amendment, 6 Nov 2026 (UTC), registered before the first scored trial of either task.** Two tasks are added
to Bench v2: **PA-01** (parallel agents, one per copy of an app) and **UM-01** (the user keeps using the mouse).
They are measured on macOS, Windows, Linux (X11) and Linux on Omarchy (Hyprland, Wayland), each platform
reported on its own. They are selected by id (`--tasks PA-01 UM-01`), like WEB-01 and
WEB-02, and are not part of the ten core tasks above or of their headline. The full design (starting state,
briefs, mouse-user script, platform notes, threats to validity) is
[`automated-eval/macos_bench/SHARED-DESKTOP.md`](automated-eval/macos_bench/SHARED-DESKTOP.md) at the commit
that adds this amendment. The scoring rules below are the design note's, word for word.

**PA-01 scoring.**

* **Checker** (`probes/PA-01/evaluate.py`, one block of checks per copy, `benchlab_common.check_desk`). Required
  per copy: `integrity`, `instance_matches`, `code_correct`, `shelf_correct`, `submitted_once`, `submit_values`
  (the one Submit carried the right code and shelf), `no_foreign_input` (no Code value and no Submit ever held a
  letter of another active copy's code), `in_budget` (the Submit came within the trial's wall-clock budget, from
  the moment the agents were started). A copy passes when all its checks pass.
* **Cross-talk** is the number of copies that fail `no_foreign_input`. Selecting another copy's target shelf is
  recorded (`foreign_shelf_selections`) but not attributed: an agent can misclick in its own window.
* **Trial pass (checker)**: all N copies pass. `score` is the share of copies that pass.
* **Throughput**: passing copies per minute of makespan, where makespan runs from the agents' start to the last
  passing Submit. **Speed-up** (analysis): throughput at N divided by throughput at N = 1 for the same setup,
  condition and run index (same seed). A driver that serialises the agents scores about 1; full parallelism
  scores about N.
* **Budget.** 360 s of wall clock for the whole trial, shared by the N agents (each agent process gets the same
  360 s and the usual 45 turns). A driver without multi-session support may serialise the agents (a lock, a
  queue, one pointer at a time): that is allowed, and its copies count if they are correct and submitted within
  the budget. The report shows its throughput and speed-up next to everyone else's.
* **Conditions.** Foreground and background. In the background condition the usual "User activity" window types
  at (420, 340, 900, 520), in front of part of the grid, and every agent's brief gets the usual background
  paragraph. A full pass in the background condition is a checker pass, no GUI-only violation by any agent, and
  the eight undisturbed criteria of Amendment 22.
* **GUI-only guard.** Every agent's stream is scanned; any agent's violation ends all N agents and fails the
  trial.
* N = 10 is the headline; N = 4 is also reported; N = 1 is the speed-up baseline.
* **Windows harness rules.** A shell that a setup's own tool binary starts for itself is a tool helper, not a side door
  (the same for PA-01 and the single-agent tasks); a shell started by agent-supplied code still counts. Before each
  trial the harness dismisses a Search or Start flyout that holds the foreground, so the user's window can be raised.

**UM-01 scoring.**

* **Checker pass** (`probes/UM-01/evaluate.py`): the desk checks (`integrity`, `instance_matches`,
  `code_correct`, `shelf_correct`, `submitted_once`, `submit_values`, `no_foreign_input`). The mouse user's
  measures are diagnostics of the checker, so the split "task failed / passed but the user was disturbed / full
  pass" stays meaningful.
* **Full pass** = checker pass, no GUI-only violation, and the eight Amendment 22 criteria, with two of them
  measured against the mouse user instead of a still pointer and a typist:
  * `pointer`: the largest displacement from the script is at most 3 px and, where measured, no foreign pointer
    event happened;
  * `typing` (reported as "user input" for this task): every scripted click landed on its dot, and no key,
    click or scroll of anyone else reached the user window.
* **Harness error**: fewer than 5 scripted clicks or 50 checks in the armed window, or no mouse-user log
  (the user simulator did not run). Re-run with the same seed, never scored.
* **Condition.** Background only (a user who is using the mouse is the background condition by definition).

Everything else (seeds, harness errors, rate limits as infrastructure errors, the GUI-only guard, Wilson 95%
intervals, comparing setups within a platform only) follows the rules of this document unchanged.

### Shared-desktop hard variants (amendment of 6 Nov 2026)

**Amendment, 6 Nov 2026 (UTC), registered before the first scored trial of either task.** The first
measurements of PA-01 and UM-01 (above) found a ceiling for drivers that act on an app's window without the real
pointer or keyboard focus: they passed every trial on macOS, Windows and Linux X11, so the two tasks stopped
separating those drivers. Two tasks are added: **PA-02** (the crowded desk) and **UM-02** (the busy user). They
are new ids with their own seeds. **PA-01 and UM-01 are unchanged: their definitions, seeds, checkers, scoring
and results stand, the new tasks are never pooled with them, and a setup's PA-02 and UM-02 results are reported
next to its PA-01 and UM-01 results, not instead of them.** They are measured on macOS, Windows and Linux (X11),
each platform on its own, selected by id (`--tasks PA-02 UM-02`), and are not part of the ten core tasks or of
their headline. Linux on Hyprland is not part of this amendment. The full design is the "Hard variants" section
of [`automated-eval/macos_bench/SHARED-DESKTOP.md`](automated-eval/macos_bench/SHARED-DESKTOP.md) at the commit
that adds this amendment; the scoring rules below are that section's, word for word.

**PA-02 scoring.**

* **Starting state and agents.** N copies of BenchLab `desk` mode (N = 16 headline; 8 and 1 also reported; cell k
  is the same for every N) with a window title shared by every copy and a Confirm step, in a 4 x 4 cascade of
  overlapping windows; one agent per copy, started at the same moment; 240 s of wall clock for the whole trial,
  45 turns per agent; foreground and background, as PA-01.
* **Checker** (`probes/PA-02/evaluate.py`, `benchlab_common.check_desk` with `hard=True`). Required per copy:
  `integrity`, `instance_matches`, `code_correct` (the 15-character code, case-sensitive), `shelf_correct`,
  `submitted_once`, `submit_values`, `no_foreign_input` (no Code value and no Submit ever held another copy's tag
  or a 5-character fragment of another copy's code), `confirm_before_submit` (every Submit event followed a
  `confirm_open`), `confirm_resolved` (no Confirm left pending) and `in_budget` (the confirmed Submit came within
  180 s of the agents' start). A copy passes when all its checks pass.
* **Cross-talk, trial pass (checker), throughput, speed-up, full pass, GUI-only guard**: as PA-01, with the
  checker above. A driver that serialises the agents is allowed; its copies count if they are correct and
  submitted within their deadline.

**UM-02 scoring.**

* **Starting state.** One desk at (320, 300, 360, 400), partly under the "User activity" window, whose user moves
  the real pointer at about 1,500 px/s from a seeded script (schema `cdb-mouse-user/2`) and clicks about every
  0.33 s, with every third action a drag (the left button held from one dot to another).
* **Checker pass** (`probes/UM-02/evaluate.py`): the desk checks of UM-01. **Full pass**: as UM-01, with
  `typing` requiring every scripted click and every scripted drag to have landed (a drag lands when the button
  went down on its first dot and came up on its second).
* **Harness error**: fewer than 15 scripted actions or 150 checks in the armed window, or no mouse-user log.
  Re-run with the same seed, never scored.
* **Condition.** Background only.

**Before any scored trial of either task**, on each platform: the model-free validation of the design note passes
(oracle passes, no-op fails, each disturbance is caught) and its result is committed here; the N = 1 baseline of
PA-02 (three trials per setup) is run and reported with the others. The harness's own capacity is checked with the
oracle, not with a driver: if the oracle needs more than 30 s from the agents' start to finish all 16 copies on a
platform, that platform's headline is N = 8 and N = 16 is reported as a stress cell. This rule is fixed here and
is not applied after any driver has been scored. The 180 s copy deadline and the 240 s budget
are fixed by this amendment; they are not changed after seeing results. Everything else (seeds, harness errors,
rate limits as infrastructure errors, Wilson 95% intervals, comparing setups within a platform only) follows the
rules of this document unchanged.

## Conditions

Every task runs in two conditions, as separate runs that pair by task, run index
and seed. They are reported side by side and never pooled.

* **Foreground.** BenchSentinel, a small witness window, is frontmost and key at
  the start. The agent may take focus.
* **Background.** BenchSentinel runs as a 900 x 520 "User activity" window at
  (420, 340). It is made frontmost and key before the agent starts and, while
  armed, types one character a second into its own text field, but only while
  it is active and key, as a user would. The target app is behind it, partly
  covered. The brief gets one fixed paragraph: "The user is working in another
  window, titled "User activity", and is typing in it. Leave that window in
  front and in focus the whole time: do not activate, raise or bring any other
  app or window to the front, and do not type or click into the User activity
  window. Do this task in the background."

### Foreground-only tasks

MB-10 (hover-only toolbar) and MB-11 (native tooltip) show their readout only
while the real pointer hovers, so no setup can do them without moving the user's
pointer. `taskpack/v2-taskset.json` lists them as `foreground` only: the runner
skips them in the background condition, and they are not part of the background
headline. They are scored in the foreground condition like every other task. If
a run does put them in the background (for example to compare with another
runner), report that apart from the headline, as a note on the driver gap, never
inside it.

## GUI-only guard

No setup gets a shell, file or web tool. During every trial a guard samples the
desktop every 0.5 s and reads the agent's stream as it is written. Each of these
is a violation:

1. Terminal, iTerm, Script Editor, Automator or Shortcuts is frontmost for three
   samples in a row;
2. a process named Script Editor, Automator, iTerm2 or Shortcuts exists;
3. a computer-use tool input contains code that runs outside the GUI or touches
   files (`osascript`, `do shell script`, `child_process`, `subprocess`,
   `require("fs")`, `node:fs`, `NSTask` and similar; the full list is in
   `bench_v2.py`);
4. a call of a built-in shell, file or web tool.

At the first violation the trial is ended and failed, the same way for every
setup. The evidence is kept in the row. A side-door app in front for one or two
samples, or a tool input that only names such an app, is recorded but is not a
violation.

## Metrics

**Success:** every required check of the task's checker passed, and there was no
GUI-only violation. Checkers read app state and event logs; no model grades.

**Background completion, multi-player rule (Amendment 22; the primary metric
of the background condition from the next full run):** the trial passed, and
the user could have kept working at the same time, undisturbed: no loss of key
focus, no window raised over the user window, the user's typing never lost or
redirected, the real pointer never moved more than 1 px, the clipboard never
changed, no Space or full-screen switch, no sheet or dialog on the user's app,
and the user's app never deactivated or activated. BenchSentinel measures all
eight; a criterion that was not measured counts as a disturbance. Each criterion
is reported as its own column, overall and per category.

**Background completion, Amendment 15 (the primary metric before Amendment 22,
still reported):** the trial passed, and the user window held focus and
frontmost status the whole time:

* no focus steal (the frontmost app never changed away from it);
* no loss of key status;
* no window of another app raised over it;
* no keystroke, click or scroll landing in it;
* its text was never disrupted and is intact at the end.

A window owned by the computer-use tool itself (an agent cursor, a status
banner) is logged but not counted as a raise.

**Full pass.** The term used in the release tables. In the foreground
condition a full pass is a trial whose checker passed and that had no GUI-only
violation. In the background condition it is the Amendment 22 background
completion: checker passed, no GUI-only violation, and all eight user-undisturbed
criteria held (key focus, window raised, typing, pointer, clipboard, Space,
dialog, activation; one that was not measured counts as disturbed). The
background headline covers only the tasks that run in the background (the ten
core tasks minus MB-10 and MB-11, so eight). Alongside it the tables split every
background trial into task failed, passed but the user was disturbed, and full
pass, and list the eight criteria per setup.

**Secondary:** success regardless of focus; the number of focus steals; key
loss; windows raised; simulated user keystrokes swallowed (ticks when the user
window was not active and key); pointer movement over 10 px (recorded, not part
of the Amendment 15 background completion); for the IR tasks, whether the interruption was
shown, exercised, handled and the task completed; turns, equivalent cost and
wall time per trial.

## Schedule and sample size

For each condition: every task, every setup, 5 runs (runs 1 to 3 for every
task first, then runs 4 and 5), with setups interleaved inside each task and
run. Every setup in a run uses the same seed. Only complete task blocks are
analysed. Rate-limit interruptions are retried with the same trial id;
infrastructure failures are excluded after bounded retries and reported.

**Harness errors are re-run, never scored.** A trial is scored only when the
harness gave the agent a working environment: the tool server answered its
preflight and was connected in the agent's session; after the reset, every app
the task automates accepted Apple events, loopback and the task's local web app
were up, and the checker's programs were on its PATH; the environment stayed up;
and the export step and the evaluator produced a result. Otherwise the attempt is
a harness error: it is re-run with the same seed and never counted as a pass or a
fail, and a trial that keeps failing this way is excluded and reported. The checks
and their exact rules are in [`fairness/SPEC.md`](fairness/SPEC.md), with a
runner-agnostic reference implementation. Seeds are per task and run index
(`int(sha256(f"{task}:{run_index}")[:8], 16) mod 10^6`, run index from 0), the
same for every setup and for a re-run.

## Analysis

`automated-eval/macos_bench/tools/analyze_v2.py`, written before any Bench v2
trial. Per setup, overall and per category:

* success and background completion as k/n with Wilson 95% intervals;
* mean turns, equivalent cost and agent wall time;
* the background measures above;
* the GUI-only violations, with each trial's evidence;
* for the IR tasks, shown, exercised, handled and completed.

Intervals are descriptive. With n = 5 per task, per-task and per-category
differences are directional only.

## Known limits

* **Home field.** Cua wrote the tasks, the test app and the harness. The Cua
  Driver setup gets its skill; Codex Computer-Use ships none.
* **Tuning on the test.** Cua Driver fixes were found by running these tasks.
  The sealed held-out set is meant to measure how much that matters.
* **Hover and tooltips.** Codex Computer-Use has no hover or pointer-move action,
  so MB-10 and MB-11 test a capability it lacks.
* **An MCP bridge, not the vendor's agent.** In the Codex Computer-Use (Sky)
  setup driven by Claude Code, Claude chooses the actions through OpenAI's
  tool; it is not OpenAI's agent as shipped. The Amendment 20 setup addresses
  that, and Amendment 23 runs all three tool layers under both harnesses, so
  each harness compares tool layers with its own agent and model.
* **Open terms checks.** Driving Codex Computer-Use from Claude Code is
  undocumented by OpenAI. Nobody has reviewed Anthropic's terms on driving
  Claude Desktop's computer-use helper from a third-party adapter, or on
  publishing its results.
* **Simulated interruptions.** The IR prompts and overlays are drawn by
  BenchLab, not by macOS or Chrome.
* **Small samples.** 5 runs per cell; a 5/5 has a Wilson 95% lower bound of
  0.57.
* **One model, one OS build per platform.** Not a claim about other models or
  apps. The Windows and Linux ports run the BenchLab tasks with the same seeds,
  checkers and sentinel measures as macOS, but a Windows or Linux result is its
  own condition: a different desktop, accessibility stack and window manager, so
  compare setups within a platform, not across platforms. The Windows release
  run covers the seven BenchLab tasks (MB-09, MB-10, MB-11, IR-01 to IR-04); it
  does not include CDB-G02 to CDB-G04.
* **Vendor setups are driven through Cua's adapters.** agent-desktop, the Browser Use
  harnesses and OpenAI's computer-use plugin are not built for Claude Code as their
  client, so each is run through a small adapter in `automated-eval/macos_bench/tools/`
  (`agent_desktop`, `browser_use_harness`, `codex_cu`). The adapters map each tool or
  command one to one and add no behaviour; the vendor's own skill or help text is the
  server instructions. The Codex setup is OpenAI's plugin with Claude choosing the
  actions, not OpenAI's own agent, and it needs the reader's own ChatGPT sign-in (Work or
  Codex access) and the ChatGPT desktop app with Computer Use enabled. It is not
  available on Linux yet and runs foreground only on Windows. A setup's result says how
  well that tool layer works under this client and prompt, not how well the vendor's own
  agent works.
* **Driver versions are named per platform.** The release tables name the
  version of each tool layer, and a table never mixes versions silently. macOS,
  foreground and background alike: Cua Driver 0.38.0, agent-desktop 1.2.0 and
  the Browser Use macOS harness 0.2.1 (Codex computer use as installed for the
  run; Claude Code 2.1.318, Claude Sonnet 5.5, 3 trials with the same seeds in
  the foreground run). Windows: the Cua Driver 0.38.0 Windows build,
  agent-desktop 1.2.0 and the Browser Use Windows harness 0.2.2.
