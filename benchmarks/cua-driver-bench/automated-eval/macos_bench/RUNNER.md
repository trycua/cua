# Claude Code benchmark runner

This runner compares one model driving Claude Code CLI (`claude -p`) through two computer-use tool layers on the
same Mac. The probes, evaluators and BenchSentinel come from the pilot (`README.md`, `PREREGISTRATION.md`); this
file covers what is new: the Claude Code harness, the schedule, the quota rules and the recorder.

| Arm | Tools Claude Code gets | Skill |
| --- | --- | --- |
| `cc-cua-driver` | Cua Driver 0.34.0 MCP server `cua` (private app, private daemon) | The Cua Driver skill of the 0.34.0 release |
| `cc-codex-cu` | OpenAI's Codex computer-use `cua_repl` launcher as MCP server `codex-cu` (`js`, `js_reset`) | none |

Fallback arms `codex-native-cu` and `codex-cua-driver` (`codex exec --json`, from the pilot) exist behind
`--allow-codex-arms`. They are not run at scale.

## Files

| File | What it does |
| --- | --- |
| `run_bench.py` | Commands `preflight`, `dry-run`, `run`, `status`, `finalize`. Trial lifecycle, retries, pauses, heartbeat |
| `bench_core.py` | Pure logic: schedule and blocks, quota gate, backoff, cutoff, ledger, completeness |
| `claude_arms.py` | The one `claude -p` invocation, scrubbed environment, per-arm MCP config and working directory, private Cua daemon |
| `claude_driver.py` | Runs one trial through the stream-json host protocol and answers the Codex app-approval requests |
| `claude_events.py` | Parses the stream: tool calls, tokens, cost, quota events, failure classification |
| `recorder.py` | Screen recording of every trial and background compression |
| `analyze_bench.py` | Runs the pre-registered `analyze.py` on complete blocks and adds cost, token and quota tables |
| `pins.json` | Version pins that preflight enforces |
| `tests/test_bench.py`, `tests/test_runner_flow.py` | Unit and flow tests (no GUI, no model) |
| `../../launch_bench.command`, `../../term_run.sh`, `../../watch_bench.py` | Terminal launch, one-off Terminal run, live status |

## The claude invocation

Both arms get this command. Only the `--mcp-config` file and the server name in `--allowedTools` differ.

```text
/opt/homebrew/bin/claude -p --model claude-sonnet-5-5 --system-prompt "<SYSTEM_PROMPT>" \
  --strict-mcp-config --mcp-config <run dir>/mcp-<arm>.json \
  --input-format stream-json --output-format stream-json --verbose --no-session-persistence \
  --tools Skill,Read,ToolSearch --setting-sources project --permission-mode dontAsk \
  --max-turns <task max_turns, 30> --max-budget-usd 6 --allowedTools mcp__<server>
```

* The system prompt replaces Claude Code's default (text in `claude_arms.SYSTEM_PROMPT`, hash in the manifest).
* Environment: an allowlist (`HOME PATH USER LANG TMPDIR TERM`) plus `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`,
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` and `DISABLE_AUTOUPDATER=1`. Variables of a parent Claude Code session cannot
  leak. `HOME` is real so the existing OAuth login keeps working; the runner never reads credentials.
* Built-in tools are `Skill`, `Read` and `ToolSearch`. `ToolSearch` is there so Claude Code's own tool deferral
  applies in both arms. All MCP tools are deferred in both arms: the model loads a tool schema with ToolSearch
  before first use. `--tool-search off` removes it (arm A's first-call prompt then grows from about 5.5k to
  about 36k tokens).
* `--setting-sources project` drops user settings, user skills, plugins and hooks. The working directory is
  `/tmp/cdb-bench-cwd/work`, outside every git checkout, rebuilt for each trial. Only arm A gets
  `.claude/skills/cua-driver` there, so the skill reaches the model through Claude Code's project-skill mechanism.
  Claude Code's own bundled skills (deep-research, design, and so on) remain in both arms.
* `--permission-mode dontAsk` with `--allowedTools mcp__<server>` lets the arm's MCP tools run unattended and denies
  everything else, including `Read` outside the working directory.
* The prompt is sent as a stream-json user message because the Codex computer-use server asks the host to approve
  each app through MCP elicitation, and only stream-json input can answer. Both arms use the same host
  (`claude_driver.py`). It accepts `Allow Computer Use to use "<App>"?` only for BenchLab (plus the task's
  `needs_apps`, Calculator for MB-07) and declines everything else.
* The user message is the task brief plus a line with the time and turn limits (`--no-tell-budget` removes it).

## Cua Driver 0.34.0 private install

Release `cua-driver-rs-v0.34.0` (github.com/trycua/cua, commit `b0968e1b12834e485dda68789541a3cc57664a9f`).

* Asset `cua-driver-rs-0.34.0-darwin-universal.tar.gz`, sha256
  `2d0ade531c07b4d16e8078844fe1b63a0dfa0ee19677c9dcc079b4d3460ab387`, equal to `SHA256SUMS` and `checksums.txt`.
  The sigstore bundle is the legacy cosign format: the signature verifies with the certificate's key (openssl),
  the certificate names `cd-rust-cua-driver.yml@refs/tags/cua-driver-rs-v0.34.0` in `trycua/cua`, and the Rekor
  entry carries the same sha256. The Fulcio chain was not verified offline.
* Private copy: `WORK/cua-0.34.0/CuaDriver-0.34.0.app`, binary sha256
  `54e5d48128fb62323366c49b6be480ccef83cdee23bd7fc1f19431347a5df86b`, `--version` prints `cua-driver 0.34.0`.
  The user's installed driver (0.28.2) and `cua-driver-local` are never used: the arm A MCP config calls the
  private binary by absolute path.
* The runner starts two private daemons with their own sockets and `HOME` (telemetry off, approvals bypassed as
  in the pilot): the agent daemon `/tmp/cdb-bench-cua-0340.sock` for arm A, and a recorder daemon
  `/tmp/cdb-bench-cua-rec.sock` (no overlay). Preflight fails unless the binary reports exactly 0.34.0, its
  sha256 matches `pins.json` and the running daemon's `health_report` reports the same version and sha256.
* Accessibility and Screen Recording belong to the bundle id `com.trycua.driver`, so the private copy uses the
  grant that already exists. No password or prompt was needed, also when started from Terminal.app.
* The skill is the 0.34.0 skills tarball (sha256 `4ea2bfc2...2292f`), copied into the arm A working directory.

## Schedule

Task order is fixed: MB-01 to MB-12, then anything else (CDB-S01). The unit of completeness is a task block.

* Phase 1: for each task, runs 1 to 3, each run a pair of back-to-back trials, one per arm.
* Phase 2, only when phase 1 is complete for every task and the quota allows: for each task, runs 4 and 5.
* First arm in a pair: `cc-cua-driver` when `(task_index + run_index)` is even (both 0-based), else `cc-codex-cu`.
* Both arms share the run's seed `sha256("<task>:<run_index>") mod 10^6`.
* Only complete blocks are analysed. Rows of an interrupted block stay in `results.jsonl` and carry
  `block_incomplete: true`.

## Quota, retries, stops

* Every `rate_limit_event` updates the five-hour and seven-day utilisation. They go into `heartbeat.json` and into
  every row (`quota_five_hour_before/after`, `quota_seven_day_before/after`, `quota_status`).
* No new trial starts when seven-day utilisation is 0.95 or higher, or a quota event says `rejected` for the
  seven-day window (`STOPPED_QUOTA`). A rejected five-hour window waits for its reset time.
* A trial cut short by a rate limit is not counted: the row is `infra_failure: rate_limit`, the runner waits
  (exponential from 60 s to 15 min, or until the reset time the message carries, at most 3 h), then retries the
  same trial id. Each pause is written to `pauses.jsonl` and the heartbeat.
* API 5xx or overload: backoff and up to 3 retries, then the trial is excluded and the run moves on. MCP start
  failure, harness crash or harness exception: one retry, then excluded. Authentication failure: one retry, then
  `STOPPED_AUTH`. Everything else (timeout, max turns, BLOCKED, wrong state) is a failed trial.
* `STOPPED_TIME` after `--cutoff-utc` (phase-2 blocks also need to fit before it), `STOPPED_USER` when `STOP`
  exists or the runner gets SIGTERM/SIGHUP, `STOPPED_DISK` below 80 GB, `STOPPED_BUDGET` only with the optional
  `--budget-cap-usd`. There is no dollar cap by default; `--max-budget-usd 6` per trial is a runaway guard.
* Cost is `total_cost_usd` from the result event (equivalent cost). It goes into the row and `ledger/spend.jsonl`.

## Per-trial output

`runs/<run>/trials/<trial id>/a<attempt>/` holds `claude-stream.tsv` (epoch-ms stamped, screenshots stripped),
`claude.stderr`, `artifacts/` (prompt, argv, evaluator result, sentinel log, HID idle log, elicitations), `video/`
(`video.mp4` at 15 fps crf 28 and `video-720p.mp4`, compressed in the background, raw deleted after a duration
check) and `trial.json`. `results.jsonl` has one row per attempt; `final: false` rows are retried attempts.

Row fields include: arm, task, phase, block, run, seed, order and first-arm flags, versions (claude, cua driver
version and sha256, daemon version, skills hash, macOS, repo commit, source hash), status, pass/score/checks and
the evaluator's `diagnostics`, wall and overhead seconds, turns, tool calls by name and class, failed calls, action
latency, tokens (input, output, cache read, cache write), cost, baseline prompt tokens, ToolSearch calls, deferral
inference, elicitation answers, disturbance (frontmost changes, pointer moved and maximum excursion, leaked input,
HID events) and quota fields.

## Pins

`pins.json` holds every pinned value and preflight fails if one differs from what is installed: Cua Driver binary
sha256, release tarball sha256, skills tarball sha256 and skills tree sha256, Cua Driver version string, Claude
Code version, macOS version and build, ChatGPT app version, the unified-computer-use plugin version, the
`@oai/cua-repl` package version and the Codex Computer Use service version. The daemon's own `health_report`
(version, exe sha256, git sha) must match too. `manifest.json` records the expected and observed values plus
the hash of the Codex MCP config and of the BenchLab and BenchSentinel executables; every trial row repeats the
Cua Driver version, binary sha256, daemon version, skills hash, claude version and macOS version.

## Gates and launching

Two things stop a launch before any model call: `WORK/HOLD` (any file there blocks the launcher and `run`) and
the latest known seven-day utilisation. The latest value comes from `ledger/quota_latest.json`, written by the
runner whenever it sees a quota event, and from ledger entries that carry `seven_day_after`. At 0.95 or above the
launcher prints `RUN GATED` and exits 3 (HOLD exits 4). `run` also refuses to start without any quota reading
(`--allow-unknown-quota` overrides), so the first thing a real run does is one small Haiku call that reads it.

## Commands

```bash
cd WORK/src/macos_bench
python3 run_bench.py dry-run --phase1-runs 3 --phase2-runs 2        # schedule and commands, no side effects
python3 run_bench.py preflight --offline --build-dir WORK/build     # static checks, no daemon, GUI or model call
python3 run_bench.py gate-check                                      # HOLD file and latest quota, no model call
python3 run_bench.py preflight --build-dir WORK/build               # full: daemons, MCP, recording, 3 small Haiku calls
# the real run, from Terminal.app and detached from T3 Code (rm WORK/HOLD first):
cp WORK/launch.env.example WORK/launch.env                           # set RUN_ID and CUTOFF_UTC
open -a Terminal WORK/launch_bench.command
python3 WORK/watch_bench.py main -f                                  # live status
touch WORK/runs/main/STOP                                            # end after the current trial
python3 analyze_bench.py WORK/runs/main/results.jsonl --out-md report.md --out-json report.json
```

Tests: `python3 -m unittest discover -s tests -p "test_[br]*.py"` (Claude runner) and
`python3 -m unittest discover -s tests` (everything; one pilot test needs a Codex plugin file that this host no
longer has). `--claude-bin PATH` substitutes a stand-in claude binary for runner tests; a stand-in run never
writes the quota file and may bypass the gates.

Resume: start the same `RUN_ID` again; final trial ids are skipped. The runner writes `DONE` or `STOPPED_<reason>`
and `done.json` when it ends. Before each trial it kills leftover `BenchLab` and `BenchSentinel` processes by exact
name, so nothing else may use those apps while a run is in progress. The desktop must be clear: windows covering
BenchLab break real-input clicks.

## Set up from a clean checkout

`CDB_BENCH_WORK` (default `~/.cache/cua-bench-h2h`) holds everything the harness creates: the private Cua Driver install,
the Codex MCP config, the spend ledger, the `HOLD` file and the run folders.

1. Build the apps: `swift/build.sh "$CDB_BENCH_WORK/build"` (BenchLab and BenchSentinel, ad-hoc signed).
2. Install the pinned driver: `python3 tools/install_cua_driver.py`. It downloads the 0.34.0 release assets, checks
   them against `pins.json` and the release `SHA256SUMS`, and unpacks a private copy. It never touches an installed driver.
3. Grant the private Cua Driver app Accessibility and Screen Recording in System Settings, once.
4. For arm B, have the ChatGPT app installed at the version in `pins.json`. The launcher writes the MCP config from
   OpenAI's shipped plugin file (`tools/make_codex_cu_mcp.py`) and the preflight fails if the versions differ.
5. Remove `$CDB_BENCH_WORK/HOLD` when you want the run to start. Copy `launch/launch.env.example` to
   `$CDB_BENCH_WORK/launch.env` and set `CUTOFF_UTC`. Then `open -a Terminal launch/launch_bench.command`, and watch with
   `python3 launch/watch_bench.py main -f`.

## Running in a VM (Amendment 1, 6 Oct 2026)

The trials run in a Lume macOS VM on a separate Mac Studio, not on the owner's desktop. What the VM holds and how it is set up (nothing here is task content; the CDB task pack is private and is not in this repository):

| Item | How |
| --- | --- |
| VM | `lume clone <cached cua base image> cdb-h2h`; `lume set cdb-h2h --cpu 6 --memory 12GB --display 1920x1080`; `lume run cdb-h2h --display none --detach --shared-dir <staging dir>`. System Settings, Displays: 1920 x 1080 (not the default 960 x 540 scaled mode). |
| Tools | Claude Code 2.1.289 (`npm i -g --prefix /opt/homebrew`), Python 3.12 (uv) linked at `/opt/homebrew/bin/python3`, ffmpeg (Homebrew), Node and Chrome from the base image, LibreOffice and GnuCash copied from the Mac. `/opt/homebrew/bin` is where the runner expects `claude`, `ffmpeg` and `python3`. |
| Harness | `$CDB_BENCH_WORK` is `~/bench-work`: `src/macos_bench` (this directory), `build/` (BenchLab and BenchSentinel, built in the VM with `swift/build.sh`), `cua-0.34.0/` (`tools/install_cua_driver.py`). |
| Cua Driver permissions | The private copy needs Accessibility and Screen Recording for bundle id `com.trycua.driver` (System Settings, Privacy and Security; add `/Applications/CuaDriver.app`, a copy of the private app). Terminal.app needs Screen Recording. |
| Arm B | The ChatGPT app and `~/.codex/computer-use/Codex Computer Use.app` copied as signed bundles (`ditto`), and the plugin cache file `~/.codex/plugins/cache/openai-bundled/unified-computer-use/<version>/.mcp.json` copied with the home directory rewritten. **Do not launch the ChatGPT app in the VM**: it updates itself on start and rewrites the plugin cache. The service needs Accessibility and Screenshots for "ChatGPT Computer Use" (its own "Enable ChatGPT Computer Use" window; Screenshots is granted by dragging the app into Screen Recording). No ChatGPT sign-in was needed for the computer surface. |
| Claude Code | `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`, kept in a 0600 file named by `CDB_CLAUDE_TOKEN_FILE`; the runner passes it to `claude` by file descriptor. |
| CDB task pack | Agent-visible subset (apps with `node_modules`, fixture, reset scripts, launch descriptor, brief) in `$CDB_TASKPACK` (`~/cdb-runtime`); the full pack in the home of OS user `cdbeval` (0700); `/usr/local/libexec/cdb-eval-run` runs the evaluator as that user (`sudoers`: `lume ALL=(cdbeval) NOPASSWD: /usr/local/libexec/cdb-eval-run`). `CDB_EVAL_SUDO=1` selects this path. |

`launch.env` for the VM sets `CDB_BENCH_WORK`, `CDB_BENCH_DISPOSABLE=1` (the runner may kill Electron, Chrome, LibreOffice and GnuCash by name), `CDB_TASKPACK`, `CDB_EVAL_SUDO=1`, `CDB_CLAUDE_TOKEN_FILE`, `RUN_ID` and `CUTOFF_UTC`. The runner is started with `open -a Terminal ~/bench-work/launch_bench.command` inside the VM, like on a Mac. Results are copied out through the shared directory after the run. When the study ends: stop and delete the VM, delete `~/.cdb-secrets`, and revoke the setup token.

## arc-driver arm (Amendment 9, CUA-1241)

`cc-arc-driver` runs arc-driver, the MCP server of the third-party package arc-cua 0.1.1 (MIT), as MCP server `arc`. It has no skill, and it starts Chrome and Electron with `--force-renderer-accessibility`. It is only ever installed in a VM clone made for it (`cdb-arc`), never on a Mac anyone uses:

1. Clone the stopped `cdb-h2h`, start the clone, and delete any `~/.cdb-secrets/claude-token` it carries.
2. Run `zsh tools/arc_driver/install_arc_driver.sh` in the VM. It creates a hash-pinned Python 3.12 venv, builds and ad-hoc signs `ArcDriverBench.app` (`com.trycua.bench.arcdriver`), and grants that bundle id Accessibility and Screen Recording.
3. Copy the printed values into `pins.json` `arc_driver`.
4. Run with `--arms cc-arc-driver`. The preflight starts the server through the launcher, checks its 16 tools, and checks that its `status` tool reports accessibility, screen recording and background input.
5. Analyse with `tools/analyze_arc.py <arc run> <v038 run> [--no-arm-b]`.
