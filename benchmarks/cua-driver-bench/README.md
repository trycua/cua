# Cua Driver Bench

Cua Driver Bench measures how well a computer-use tool layer lets an agent get
real desktop work done, and whether it can do that work in the background
without taking the user's focus, keyboard or pointer. The agent, the model, the
prompts and the limits are the same for every setup; only the computer-use tool
changes.

This directory holds the harness, the public task set and the method of
**Bench v2**. Bench v2 is **macOS-only for now**: every task, app and
measurement assumes a macOS 26 desktop.

| Path | What it is |
| --- | --- |
| [`METHOD.md`](METHOD.md) | Pre-registration summary: design, setups, conditions, metrics, analysis, and the dated amendment commits |
| [`automated-eval/macos_bench/`](automated-eval/macos_bench) | The harness: runner, BenchLab and BenchSentinel apps, the BenchLab tasks, analysis |
| [`automated-eval/macos_bench/TASKS.md`](automated-eval/macos_bench/TASKS.md) | The ten Bench v2 tasks: prompts, setup, checkers, limits |
| [`automated-eval/macos_bench/RUNNER.md`](automated-eval/macos_bench/RUNNER.md) | How the runner works: the `claude -p` invocation, isolation, schedule, output |
| [`taskpack/`](taskpack) | The three multi-app tasks (Electron apps, fixtures, evaluators) that CDB-G02 to CDB-G04 run |

No results are published here yet. The final Bench v2 run and the held-out run
will be published together with their raw rows.

## The Bench v2 task set

Ten tasks in three categories. Each task has a `PROVENANCE.md` next to it.

| Task | Category | What the agent does |
| --- | --- | --- |
| CDB-G02 | Multi-app workflows | Replan a schedule across mail, chat, a calendar and a LibreOffice Calc workbook after a later message overrides an earlier one |
| CDB-G03 | Multi-app workflows | Review payables across a mail app, a Calc workbook and a GnuCash book, and record discrepancies and duplicates |
| CDB-G04 | Multi-app workflows | Triage duplicate support cases with a case tracker, a manual, a local archive site in Chrome and an access log |
| MB-09 | Precision interactions | Reorder a native list by drag and drop |
| MB-10 | Precision interactions | Reveal a hover-only toolbar and click one action |
| MB-11 | Precision interactions | Read a native tooltip and type the code it shows |
| IR-01 | Interruptions & recovery | Finish a two-step form; a permission-style prompt appears on the way |
| IR-02 | Interruptions & recovery | Apply settings while a notification-style banner covers the button |
| IR-03 | Interruptions & recovery | Edit two notes and keep both changes through a save prompt |
| IR-04 | Interruptions & recovery | Sign up on a web page while a cookie-consent overlay appears |

The CDB-G tasks are the GUI-only variants of pack tasks `cdb-s02` to `cdb-s04`:
the same files, with no coding tools offered. The MB and IR tasks run in
BenchLab, a small AppKit test app built from source in this directory. A fourth
category, Web apps & forms, has no Bench v2 task yet.

## Method in brief

The full method, with the reasons for each rule, is in [`METHOD.md`](METHOD.md).

* **Pre-registration.** The protocol was committed before the first trial, and
  every change since is a dated amendment committed before the trials it covers.
  `METHOD.md` lists the commits.
* **One agent, different tools.** Every setup runs Claude Code
  (`claude -p`, Claude Sonnet 5.5) with the same system prompt, the same
  built-in tools and the same limits: 360 s and 45 turns per trial.
* **Two conditions.** In the *foreground* condition a small witness window is
  in front at the start and the agent may take focus. In the *background*
  condition a "User activity" window stays in front and types one character a
  second, as a user would; the target app is behind it, partly covered, and the
  brief asks the agent to leave that window in front and in focus.
* **GUI-only guard.** Agents get no shell or file tools. A live guard ends a
  trial, and fails it, if the agent reaches a shell through the GUI (Terminal,
  Script Editor, Automator, Shortcuts) or sends code that runs outside the GUI
  through a computer-use tool.
* **Checkers, not judges.** Each task's checker reads the app's final state and
  event log. No model grades anything.
* **Runs.** 5 runs per task, setup and condition (3 in phase 1, then 2),
  interleaved across setups, with a seed per task and run that every setup
  shares.

## Metrics

| Metric | Definition |
| --- | --- |
| Success | The checker passed every required check. A GUI-only violation is a fail. |
| Background completion | Background condition only, the primary metric: the trial passed **and** the user window held focus the whole time: no focus steal, no loss of key status, no window raised over it, no keystroke, click or scroll landing in it, and its text intact. |
| Focus steals | Times the frontmost app changed away from the witness or user window |
| Key loss | Times the witness or user window lost key status |
| Windows raised | Times a window of another app appeared above the witness or user window. Windows owned by the computer-use tool itself (an agent cursor, a status banner) are logged but not counted. |
| User keystrokes swallowed | Simulated user keystrokes that never reached the user window, because it was not active and key |
| Pointer moved | The real pointer moved more than 10 px. Recorded, not part of background completion. |
| Interruption handled | IR tasks: the interruption was shown, appeared before the final action, and was answered safely |
| Cost, turns, wall time | Claude Code's equivalent cost, model turns and agent wall time per trial |

Success and background completion are reported as k/n with a Wilson 95%
interval, overall and per category. With n = 5 per task and setup, per-task and
per-category differences are directional only.

## Setups

| Setup | Status | What the agent drives |
| --- | --- | --- |
| **Cua Driver** | Run | Cua Driver's MCP server, with its skill and `run_script` on (Cua Driver 0.35 defaults), built from a pinned main commit (arm `cc-cua-driver-script`) |
| **Codex Computer-Use (Sky)**, driven by Claude Code | Run | OpenAI's `cua_repl` launcher from the ChatGPT app 26.930 plugin, talking to OpenAI's Sky computer-use service, registered in Claude Code as MCP server `codex-cu` (arm `cc-codex-cu`). Claude chooses the actions through this MCP bridge; OpenAI's model is not involved. |
| **Codex Computer-Use (Sky)**, driven by Codex CLI + `gpt-6-astra` | Planned (pre-registered in Amendment 20, not yet run) | The same Codex tool layer driven by OpenAI's own agent and model: the Codex CLI bundled with the ChatGPT app, run as `codex app-server`, with `gpt-6-astra`. Same prompts, approvals, limits, guard and checkers. Its harness code is `automated-eval/macos_bench/codex_native.py` (arm `cx-codex-cu`); its smoke checks pass, and it runs in the next full Bench v2 run. No numbers yet. |

The harness also keeps internal baseline arms (the Cua Driver 0.34.0 release,
main with `run_script` off, and the skill in the system prompt) that are not
public setups.

## Reproduce

You need an Apple silicon Mac that is not your daily machine, or better, a
macOS VM on one. The registered runs used one [Lume](../../libs/lume) macOS VM.

### 1. Prepare the macOS VM

| Item | Registered runs |
| --- | --- |
| macOS | 26.5.2 (25F84), Apple silicon |
| VM | 6 CPU, 12 GB, display 1920 x 1080 (System Settings > Displays, not the scaled 960 x 540 mode). The task windows assume this size. |
| Claude Code | 2.1.289 at `/opt/homebrew/bin/claude` (`npm i -g --prefix /opt/homebrew @anthropic-ai/claude-code@2.1.289`) |
| Python | 3.12 at `/opt/homebrew/bin/python3` |
| Tools | ffmpeg (screen recordings), Node 22 or later, Google Chrome, LibreOffice and GnuCash (CDB-G02 to CDB-G04), Xcode Command Line Tools (`xcrun swiftc`) |
| Login | `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`, in a 0600 file named by `CDB_CLAUDE_TOKEN_FILE`; the runner passes it to `claude` by file descriptor |

Keep the desktop clear: windows that cover BenchLab break real-input clicks.

### 2. Get the harness and build the apps

```bash
export CDB_BENCH_WORK=~/bench-work
mkdir -p "$CDB_BENCH_WORK/src"
cp -R benchmarks/cua-driver-bench/automated-eval/macos_bench "$CDB_BENCH_WORK/src/"
cp -R benchmarks/cua-driver-bench/taskpack "$CDB_BENCH_WORK/taskpack"
cd "$CDB_BENCH_WORK/src/macos_bench"
swift/build.sh "$CDB_BENCH_WORK/build"     # BenchLab.app and BenchSentinel.app, ad-hoc signed
```

Install the task apps' Electron dependency, pinned by each lockfile:

```bash
for app in "$CDB_BENCH_WORK"/taskpack/tasks/shared/*/apps/*; do (cd "$app" && npm ci); done
```

### 3. Install each setup

* **Cua Driver.** Build Cua Driver from the pinned main commit inside the VM
  with the scripts in [`tools/vm_main_build/`](automated-eval/macos_bench/tools/vm_main_build)
  (`build_main.sh`, `assemble_main.sh`, `assemble_script.sh`, then
  `tcc_grant.sh` for each app; they need SIP off in the VM). Record the binary,
  skill and script hashes in `pins.json` `cua_main`. The baseline release is
  installed with `python3 tools/install_cua_driver.py`, which checks the
  release assets against `pins.json` and never touches an installed driver.
  Grant the private apps Accessibility and Screen Recording.
* **Codex Computer-Use (Sky).** Install the ChatGPT app at the version in
  `pins.json` and copy `~/.codex/computer-use/Codex Computer Use.app` and the
  `unified-computer-use` plugin cache into the VM. Do not launch the ChatGPT app
  in the VM: it updates itself and rewrites the plugin cache. Give "ChatGPT
  Computer Use" Accessibility and Screen Recording. The launcher writes the MCP
  config from the plugin's own file (`tools/make_codex_cu_mcp.py`). The plugin
  is OpenAI's and is not redistributed here; driving it from a client other
  than OpenAI's is undocumented.
* **Codex CLI + `gpt-6-astra`** (arm `cx-codex-cu`). Nothing extra to install: it uses the Codex CLI bundled in the same ChatGPT app (`Contents/Resources/codex-cli/CodexCLI.app`) and the same `cua_repl` MCP config as the Claude-driven Codex setup. Point `CDB_CODEX_GATEWAY` at an OpenAI-compatible Responses endpoint that serves `gpt-6-astra` (default `http://127.0.0.1:4000/v1`). The pinned CLI build is in `pins.json` (`codex_native`).

### 4. Check, then run

```bash
cd "$CDB_BENCH_WORK/src/macos_bench"
export CDB_BENCH_DISPOSABLE=1      # the runner may kill Electron, Chrome, LibreOffice and GnuCash by name
python3 run_bench.py preflight --offline --bench-v2 --build-dir "$CDB_BENCH_WORK/build"
python3 run_bench.py preflight --bench-v2 --build-dir "$CDB_BENCH_WORK/build" \
  --arms cc-cua-driver-script cc-codex-cu
```

The preflight checks every pin (driver binary and skill hashes, Claude Code,
macOS and Codex versions, and the tree digest of each pack task), starts each
MCP server and makes three small Haiku calls. Then start a run from a
Terminal.app shell, one condition at a time:

```bash
python3 run_bench.py run --bench-v2 --condition background --park-pointer \
  --arms cc-cua-driver-script cc-codex-cu \
  --phase1-runs 3 --phase2-runs 2 --run-id v2-background --build-dir "$CDB_BENCH_WORK/build"
python3 run_bench.py run --bench-v2 --condition foreground --park-pointer \
  --arms cc-cua-driver-script cc-codex-cu \
  --phase1-runs 3 --phase2-runs 2 --run-id v2-foreground --build-dir "$CDB_BENCH_WORK/build"
```

`launch/launch_bench.command` wraps the same command with the quota gate and a
log, and `launch/watch_bench.py` shows live status. A run resumes when started
again with the same `--run-id`. See [`RUNNER.md`](automated-eval/macos_bench/RUNNER.md)
for the gates, retries and stop rules.

**Evaluator isolation (optional).** By default the task evaluators run as the
bench user. The registered runs ran them as a separate OS user that the agent's
user cannot read (`CDB_EVAL_SUDO=1`, through `/usr/local/libexec/cdb-eval-run`;
see `cdb_adapter.py` for the call). Every tool input is also scanned for
evaluator and oracle paths, and the row records any peek.

### 5. Analyse

```bash
python3 tools/analyze_v2.py "$CDB_BENCH_WORK/runs/v2-background" "$CDB_BENCH_WORK/runs/v2-foreground" \
  --md report.md --json report.json
```

The report gives, per setup, overall and per category: success and background
completion with Wilson intervals, focus steals, key loss, windows raised,
swallowed keystrokes, GUI-only violations with evidence, and the interruption
rates. Keep raw results, streams and recordings out of git.

### Tests

```bash
cd automated-eval/macos_bench
python3 -m pytest tests probes/tests swift/tests          # harness, checkers, sentinel (no GUI, no model)
cd ../../taskpack/tasks/shared/cdb-s02 && python3 -m unittest discover -s tests
```

## Adding tasks

New tasks follow the same layout: a BenchLab task gets `probes/<ID>/` (with
`task.json`, `brief.md`, `render_brief.py`, `evaluate.py` and `PROVENANCE.md`)
and, if needed, a BenchLab mode; a multi-app task goes into `taskpack/tasks/`
with a selector in `probes/<ID>/task.json`, its tree digest in `pins.json`, and a
`PROVENANCE.md`. A task joins the Bench v2 list (`bench_v2.V2_TASKS`) only
through a dated amendment committed before its first trial. See
[`taskpack/README.md`](taskpack/README.md).

## Licence

MIT, under the repository [LICENSE](../../LICENSE.md). The task pack was first
written in Cua's private benchmark repository and is relicensed under MIT here;
see [`taskpack/README.md`](taskpack/README.md) and its third-party notices.
