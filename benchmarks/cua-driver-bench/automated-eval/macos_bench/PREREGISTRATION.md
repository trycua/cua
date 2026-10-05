# Pre-registration: Claude Code with Cua Driver vs Claude Code with Codex computer use

Status: frozen when this file is committed, before any trial that enters the analysis.
Date: 2026-10-05 (UTC evening). Scope: head-to-head, macOS host, one Mac. Owner: Cua Driver Bench maintainers.
Supersedes the pilot pre-registration (private trycua/cua-driver-bench PR #59: Codex CLI with gpt-6-astra, Cua Driver arm only).

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
