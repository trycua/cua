# Bench v2 method and pre-registration summary

This is the public summary of how Cua Driver Bench v2 is run and analysed. It
restates the rules of the pre-registration that applies to Bench v2. The
pre-registration itself, with every amendment, lives in the git history listed
under [Registration record](#registration-record); this summary adds no rule.

## Registration record

The protocol was committed before the first trial, and every change since then
is a dated amendment committed before the trials it covers. The full text is
`benchmarks/cua-driver-bench/automated-eval/macos_bench/PREREGISTRATION.md` at
these commits of `trycua/cua` (branch `bench/macos-head-to-head`). Times are the
commit times in UTC; GitHub's push history for that branch is the independent
record of when each one became public.

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

Smoke runs (Amendments 14 to 17) validate the harness and are never analysed.
Amendments 1 to 13 cover Bench v1, which used different tasks and tools; Bench
v2 results are never pooled with them.

**Coming next, each registered before its first trial:** a larger Bench v2 task
set, the final full run with the Amendment 20 setup, and a run on a sealed
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

## Setups

| Setup | Arm id | What changes |
| --- | --- | --- |
| Cua Driver | `cc-cua-driver-script` | Cua Driver built from a pinned main commit, with its skill and `run_script` on (the default from Cua Driver 0.35) |
| Codex Computer-Use (Sky) | `cc-codex-cu` | OpenAI's `cua_repl` launcher from the ChatGPT app 26.930 plugin, used unmodified except that the browser surface is off, through an MCP bridge. Claude chooses the actions. No skill ships with it. |
| Codex Computer-Use (Sky), driven by Codex CLI + `gpt-6-astra` | registered (Amendment 20), not yet run | The same `cua_repl` tool layer, driven by OpenAI's own agent (the Codex CLI bundled with the ChatGPT app, run as `codex app-server`) and model (`gpt-6-astra`), with the same prompts, approvals, budget, GUI-only guard and checkers. It joins the next full run. Its harness code will be added here once its smoke checks pass. |

The registered runs also included a third setup that is not part of this
public release. Internal baselines (other Cua Driver builds) may be run; they
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

**Background completion (primary metric of the background condition):** the
trial passed, and the user window held focus and frontmost status the whole
time:

* no focus steal (the frontmost app never changed away from it);
* no loss of key status;
* no window of another app raised over it;
* no keystroke, click or scroll landing in it;
* its text was never disrupted and is intact at the end.

A window owned by the computer-use tool itself (an agent cursor, a status
banner) is logged but not counted as a raise.

**Secondary:** success regardless of focus; the number of focus steals; key
loss; windows raised; simulated user keystrokes swallowed (ticks when the user
window was not active and key); pointer movement over 10 px (recorded, not part
of background completion); for the IR tasks, whether the interruption was
shown, exercised, handled and the task completed; turns, equivalent cost and
wall time per trial.

## Schedule and sample size

For each condition: every task, every setup, 5 runs (runs 1 to 3 for every
task first, then runs 4 and 5), with setups interleaved inside each task and
run. Every setup in a run uses the same seed. Only complete task blocks are
analysed. Rate-limit interruptions are retried with the same trial id;
infrastructure failures are excluded after bounded retries and reported.

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
  that. Comparing it with Cua Driver changes the tool layer, the agent and the
  model at once, and reports will say so.
* **Simulated interruptions.** The IR prompts and overlays are drawn by
  BenchLab, not by macOS or Chrome.
* **Small samples.** 5 runs per cell; a 5/5 has a Wilson 95% lower bound of
  0.57.
* **One model, one OS build.** Not a claim about other models, apps, Windows or
  Linux.
