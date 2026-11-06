# Prompt tests: plain-English goals, hard checks

Status: v1 (CUA-3570). Approved direction: Francesco, 5 Nov 2026.

## What

A prompt test is a goal written the way a user would say it ("add a second
computer, then remove it"). An agent carries it out on a real machine by
driving the real Cua Spaces app through Cua Driver. Afterwards, hard checks
decide pass or fail: the relay API, `cua ... --json`, the app's accessibility
tree, the window titles that appeared, and files. The model never decides the
verdict.

They exist because the bugs found on Windows and Linux on 5 Nov (a ghost row
after Remove, a PC that stayed offline after setup, staging taking over the
production host, a firewall prompt, an offline computer still shown online)
all sat between components. Each passed its unit tests. Only someone using
the app on a real machine saw them.

## The chosen pattern

| Piece | Where | What |
|---|---|---|
| Spec | `tests/agentic/<area>/<name>.yaml` | `name`, `os`, `setup` (build, canary account, hosts), `prompt`, `budget` (steps, minutes), `expect` (hard checks). Validated by `bin/agentic-e2e validate`; unknown keys are errors |
| Runner | `bin/agentic-e2e`, `tests/agentic/runner/` | Installs the build, starts Cua Driver, sets up the hosts, signs the canary in, runs the agent, runs the checks, writes the report, tears down |
| Agent | Claude Code headless, version pinned | The model and the harness version are pinned in `tests/agentic/config.yaml`. Its only tools are Cua Driver's, through a pass-through MCP server that logs every call, enforces the step budget and hides recording and config tools |
| Checks | `runner/checks.py` | `relay` and `cli` take jq predicates. `ui` asserts on the tree (`contains`, `absent`, `row`), held for `hold_s` or reached within `within_s`, optionally timed from an event such as `power_off:2`. `no_dialogs` watches every window title from install to the last check. `file` |
| Trajectories | `tests/agentic/trajectories/<area>/<name>/<os>/` | Cua Driver's own turn files (`action.json`) plus `portable.json` |
| Artifacts | the CI run's `agentic-e2e-<os>` artifact | `report.md`, screenshots, the video, `calls.jsonl` and the agent transcript, for 30 days |
| Workflow | `.github/workflows/e2e-agentic.yml` | Nightly on GitHub's Windows, Linux and macOS runners; `workflow_call` for the release gate; specs and runner validated on PRs |

**Hosts.** A host is a Cua home on the test machine. The app host is the
computer the person uses. A `computer` host stands in for a second computer.
It runs as a plain process, so `cua-e2e-power off` in its terminal is a real
power loss as far as the relay can tell. One machine can therefore test
"add a second computer" with no second VM. Every host gets a run-unique name
(`e2e-<run>-h2`), and teardown takes every computer the run added off the
canary account. It also removes computers that crashed runs left behind
(offline for more than 3 hours).

**Record and replay.** A passing live run with `--record` saves the turns
that Cua Driver's `start_recording` wrote. Cua Driver's `replay_trajectory`
re-sends the recorded pid, window id and element tokens verbatim, so it
cannot work against a new app process. Replay reads the same turn files and
maps those three onto the live app first: pid by app name, window by title,
token by finding the row with the same text. If the replay breaks (a turn
cannot be mapped, a tool errors, or a check fails), the runner tears down,
sets up again and runs the agent live. If the live run passes, it writes an
updated trajectory to commit in a PR. If it fails, the test is red. A
trajectory that types text read off the screen (a pairing code) is marked
not replayable, so that test always runs live. The follow-up is to move
pid, window and token mapping into `replay_trajectory` itself.

**Tiers.** Nightly: everything live, after the Spaces nightly build
publishes. Release: `workflow_call`, everything live. It joins the release
gate once the secrets exist and the nightly has been green for a week. PR
(v2): replay only the tests of the areas the PR touches.
`bin/agentic-e2e select --changed` already maps changed paths to areas
(`config.yaml`).

## Alternatives

- **Playwright (or any scripted UI test) only.** Deterministic and cheap, but
  every flow is code that breaks when the UI changes, and the tests check what
  the author thought of. The 5 Nov bugs needed someone to notice a
  firewall prompt or a row that came back. Our replay is the scripted part;
  the agent covers what a script would miss.
- **The agent judges pass or fail.** One prompt, no checks to write. But an
  agent's verdict is a claim, not evidence: it can talk itself into "done",
  and the same run can be judged differently twice. Rejected. A model review
  is optional, goes in the report, and never sets the verdict.
- **Record-only (a person records once, CI replays).** Cheap per run, but the
  recording breaks with every UI change and needs a person to fix it. Here a
  broken replay falls back to the agent, which re-records it.

## Model, secrets, cost

- **Model:** `claude-sonnet-5-5` (the latest Sonnet), effort medium, pinned
  by exact id. The harness is Claude Code headless, also pinned (npm
  version), with no built-in tools, no settings or memory files, and a
  short system prompt.
- **Secrets:** `ANTHROPIC_API_KEY`, `CUA_E2E_CANARY_USERNAME`,
  `CUA_E2E_CANARY_PASSWORD` (the staging canary account). The runner signs
  in with the device code flow, as a person would, and refuses any realm
  but staging. Canary variables are stripped from the agent's environment.
  Nothing secret is logged. Only the staging environment is ever touched.
- **Cost:** about $0.17 to write the tool list into the cache once per run,
  then cents per step. Measured live runs are in the PR. The nightly
  budget is (tests × OS lanes) × that. Runner minutes: one GitHub-hosted job
  per OS.

## Flakiness

- Only hard checks decide, and they poll (`within_s`) instead of sleeping.
  `hold_s` turns "no ghost row" into "absent for 45 s", not "absent once".
- Budgets cap steps (enforced in the MCP proxy) and minutes. A spent budget
  is a failure with its own label.
- Every run is isolated: fresh Cua homes, a fresh app profile, run-unique
  computer names, and teardown that always runs. Parallel OS lanes share
  the canary account without seeing each other's computers.
- The model, harness, Cua Driver release and build are pinned per run and
  printed in the report, so a red night can be traced to what changed.
- Replay is deterministic; the agent runs only when replay breaks.

## Not in v1

- macOS lane: the workflow plans it, but no test lists macOS yet. Hosted
  macOS needs the Cua Driver TCC seeding that `e2e-rust-macos.yml` uses, and
  host terminals.
- PR replay (v2), and the release gate wiring (after the secrets and a green
  week).
- Pid, window and token mapping inside `replay_trajectory`.
