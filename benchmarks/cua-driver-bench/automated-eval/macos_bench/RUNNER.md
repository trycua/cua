# The Bench v2 runner

`run_bench.py` runs one model through Claude Code CLI (`claude -p`) with one
computer-use tool layer per arm, on one Mac or macOS VM. This file covers the
invocation, isolation, schedule, gates and output. The method is in
[`../../METHOD.md`](../../METHOD.md) and the tasks in [`TASKS.md`](TASKS.md).

## Arms

| Arm | Public setup | Tools Claude Code gets |
| --- | --- | --- |
| `cc-cua-driver-script` | Cua Driver | Cua Driver built from the pinned main commit, MCP server `cua`, `run_script` on, the skill of that commit in the working directory, and a short `run_script` addendum in the system prompt |
| `cc-codex-cu` | Codex Computer-Use (Sky) | OpenAI's `cua_repl` launcher from the ChatGPT app 26.930 plugin as MCP server `codex-cu` (`js`, `js_reset`), no skill |
| `cc-codex-cu-1007-browser` | Codex Computer-Use (Sky), app 26.1007 | The same launcher from app 26.1007 with the browser surface on |
| `cc-claude-cu-helper` | Claude Computer-Use (Desktop helper): Claude Desktop 2.31226.0 computer-use helper via a minimal adapter; an internal interface, not an Anthropic-supported procedure | Cua's adapter `tools/claude_cu_helper/cu_helper_mcp.py` as MCP server `claude-cu-helper` (`app_list_windows`, `app_screenshot` and the pinned input tools), no skill |
| `cx-cua-driver`, `cx-codex-cu`, `cx-claude-cu-helper` | Cua Driver, Codex Computer-Use (Sky) and Claude Computer-Use (Desktop helper, with the label above), driven by Codex CLI (Amendment 23) | Not Claude Code: the bundled Codex CLI as `codex app-server` (`codex_native.py`) with GPT-6.1 Sol (`gpt-6.1-sol`, default effort low), given the MCP entry and system prompt of `cc-cua-driver-script`, `cc-codex-cu` or `cc-claude-cu-helper` |
| `cc-cua-driver`, `cc-cua-driver-main`, `cc-cua-driver-main-skill` | Internal baselines | Cua Driver 0.34.0 release; main with `run_script` off; main with the skill in the system prompt |

Arms `codex-native-cu` and `codex-cua-driver` (`codex exec --json`, from
`run_pilot.py`) exist behind `--allow-codex-arms` and are not part of Bench v2.

## Files

| File | What it does |
| --- | --- |
| `run_bench.py` | Commands `preflight`, `dry-run`, `gate-check`, `run`, `status`, `finalize`. Trial lifecycle, retries, pauses, heartbeat |
| `bench_v2.py` | Bench v2: task list and categories, the GUI-only guard, the background condition and its scoring |
| `bench_core.py` | Schedule and blocks, quota gate, backoff, cutoff, ledger, completeness |
| `claude_arms.py` | The `claude -p` invocation, scrubbed environment, per-arm MCP config and working directory, the private Cua Driver daemons |
| `claude_driver.py` | Runs one trial through the stream-json host protocol and answers Codex's per-app approval requests |
| `claude_events.py`, `codex_events.py` | Parse the stream: tool calls by class, tokens, cost, quota events, failure classification |
| `cdb_adapter.py` | Runs a pack task from its launch descriptor (reset, apps, evaluator) |
| `recorder.py` | Screen recording of every trial and background compression |
| `analyze.py`, `analyze_bench.py`, `tools/analyze_v2.py` | Analysis; `tools/analyze_v2.py` is the Bench v2 report |
| `swift/` | BenchLab (the test app) and BenchSentinel (the witness and simulated user) |
| `pins.json` | Version pins the preflight enforces |

## The claude invocation

Every arm gets this command. Only the `--mcp-config` file, the server name in
`--allowedTools` and the arm's system-prompt addendum differ.

```text
/opt/homebrew/bin/claude -p --model claude-sonnet-5-5 --system-prompt "<SYSTEM_PROMPT>" \
  --strict-mcp-config --mcp-config <run dir>/mcp-<arm>.json \
  --input-format stream-json --output-format stream-json --verbose --no-session-persistence \
  --tools Skill,Read,ToolSearch --setting-sources project --permission-mode dontAsk \
  --max-turns 45 --max-budget-usd 6 --allowedTools mcp__<server>
```

* The system prompt replaces Claude Code's default (`claude_arms.SYSTEM_PROMPT`;
  its hash is in every row).
* Environment: an allowlist (`HOME PATH USER LANG TMPDIR TERM`) plus
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` and
  `DISABLE_AUTOUPDATER=1`. In a VM, `CDB_CLAUDE_TOKEN_FILE` names a 0600 file
  with a `claude setup-token` token, passed to `claude` by file descriptor.
* Built-in tools are `Skill`, `Read` and `ToolSearch`; `Read` is allowed only in
  the skill directory. All MCP tools are deferred in every arm, so the model
  loads a tool schema with ToolSearch before first use.
* `--setting-sources project` drops user settings, skills, plugins and hooks.
  The working directory is `/tmp/cdb-bench-cwd/work`, outside every git
  checkout, rebuilt for each trial. Only Cua Driver arms get
  `.claude/skills/cua-driver` there.
* The prompt is sent as a stream-json user message because the Codex service
  asks the host to approve each app through MCP elicitation. The host
  (`claude_driver.py`, the same for every arm) accepts only the apps the task
  names and declines everything else.
* The user message is the task brief, the background paragraph in that
  condition, and a line with the time and turn limits.

## Private Cua Driver installs

The harness never uses an installed Cua Driver. Each Cua Driver arm has its own
app copy, bundle id, daemon socket, `HOME` and state, and its MCP config calls
the binary by absolute path. Telemetry is off. A second daemon records the
screen and places windows; it is not offered to the agent. The preflight fails
unless each binary, skill tree and running daemon report the pinned version
and sha256.

* The 0.34.0 baseline: `tools/install_cua_driver.py` downloads the release
  assets, checks them against `pins.json` and the release `SHA256SUMS`, and
  unpacks a private copy under `$CDB_BENCH_WORK`.
* The main build: `tools/vm_main_build/` builds Cua Driver at the pinned commit
  inside the VM, assembles ad-hoc signed apps with their own bundle ids
  (`com.trycua.driver.benchmain`, `com.trycua.driver.benchscript`) and grants
  them Accessibility and Screen Recording.

## Schedule

The unit of completeness is a task block (one task, every arm, one run).

* Phase 1: for each task in schedule order, runs 1 to 3, each run one trial per
  arm back to back. Phase 2, only when phase 1 is complete: runs 4 and 5.
* Arm order inside a run alternates with the task and run index.
* Every arm in a run shares the run's seed `sha256("<task>:<run_index>") mod 10^6`.
* Only complete blocks are analysed. Rows of an interrupted block carry
  `block_incomplete: true`.
* `--condition foreground|background` sets the condition for the whole run.
  Trial ids are the same in both, so runs pair by task, run and seed.

## Gates, retries and stops

* Quota: every `rate_limit_event` updates the five-hour and seven-day use. No new
  trial starts at seven-day 0.95 or above, or after a seven-day rejection
  (`STOPPED_QUOTA`). A rejected five-hour window waits for its reset.
* A trial cut short by a rate limit is not counted: it is retried with the same
  trial id after a backoff. API 5xx or overload: up to 3 retries, then the trial
  is excluded. MCP start failure or harness error: one retry, then excluded.
  Everything else (timeout, max turns, BLOCKED, wrong state, GUI-only
  violation) is a failed trial.
* `STOPPED_TIME` after `--cutoff-utc`, `STOPPED_USER` when `STOP` exists in the
  run folder or on SIGTERM, `STOPPED_DISK` below 80 GB. `--max-budget-usd 6` per
  trial is a runaway guard.
* `$CDB_BENCH_WORK/HOLD` blocks the launcher and `run`.
* Before each trial the runner kills leftover BenchLab and BenchSentinel
  processes by exact name, and with `CDB_BENCH_DISPOSABLE=1` the task apps too,
  so nothing else may use those apps during a run.

## Per-trial output

`runs/<run>/trials/<trial id>/a<attempt>/` holds the stripped stream
(`claude-stream.tsv`), `artifacts/` (prompt, argv, evaluator result, sentinel
log, HID idle log, elicitations), `video/` and `trial.json`. `results.jsonl` has
one row per attempt; `final: false` rows are retried attempts. Rows carry the
arm, task, category, condition, run, seed, versions and hashes, status, pass and
checks, turns, tool calls by class, tokens, cost, the background measures, the
GUI-only evidence and the interruption diagnostic.

## Commands

```bash
python3 run_bench.py dry-run --bench-v2 --phase1-runs 3 --phase2-runs 2    # schedule and commands, no side effects
python3 run_bench.py preflight --offline --bench-v2 --build-dir WORK/build  # static checks only
python3 run_bench.py gate-check                                             # HOLD file and quota, no model call
python3 run_bench.py preflight --bench-v2 --build-dir WORK/build            # full: daemons, MCP, recording, 3 Haiku calls
python3 run_bench.py run --bench-v2 --condition background --run-id RUN ... # see ../../README.md
touch WORK/runs/RUN/STOP                                                    # end after the current trial
python3 tools/analyze_v2.py WORK/runs/RUN --md report.md
```

Tests: `python3 -m pytest tests probes/tests swift/tests` (no GUI, no model).
`--claude-bin PATH` substitutes a stand-in `claude` binary for runner tests.
