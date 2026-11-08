---
name: formally-verify-cua-driver
description: Model-check the Cua Driver's state machines with TLA+ (element tokens, session lifecycle, permission admission, macOS foreground restore, recording, Windows typing and serve drain, invoke_menu cancel) after a change to the code they cover. Use when a diff touches a file in the coverage table, when a race or ordering bug is suspected there, or to add a new machine.
---

# Formally verify Cua Driver

Tests check the interleavings someone wrote down. These TLA+ models check every interleaving of a few actors, inside small bounds, for the driver's state machines: the places where a bug does the most harm and tests reach it least. Each model is a few dozen lines that mirror one piece of code, step for step. `verify-cua-driver` checks behaviour on a real desktop. This skill checks orderings no fixture can force. Use both.

## Does the change touch a model?

Match the diff against the **Code** column below. If a covered function changed, rerun its rows and reread the model actions that cite it. If you add a lock, a flag, a background worker or a new end state to one of these machines, the model needs the same step.

Paths are relative to `libs/cua-driver/rust/crates/`. Each spec lives in `<crate>/models/` beside the code it describes, and its header comment lists each config and what it shows.

| Machine | Spec | Code it mirrors | Rules (invariants) | Harm when a rule breaks |
| --- | --- | --- | --- | --- |
| Element token | `cua-driver-core/models/token.tla` | `cua-driver-core/src/snapshot_store.rs` `publish_snapshot`, `row_snapshot`, `remember_retired`, `resolve`, `token_owner`, `set_zoom`; `element_token.rs` `fill_pid_from_token`, `LRU_CAP_PER_PID` | R1 acts only on a current snapshot. R2 one current snapshot per window. R3 acts on the snapshot the caller named. R4 a refusal names the current snapshots. R5 a read reports what it retired. R6 a zoom binds only to a current snapshot | a click or a typed value lands on a different element than the agent read, and is reported as a success |
| Session lifecycle | `cua-driver-core/models/session.tla` | `cua-driver-core/src/session.rs` `begin_session_dispatch_inner`, `SessionDispatchGuard::drop`, `end_session_with_reason`, `finish_session_end`, `mark_session_ended`, `evict_idle_matching`, `retry_session_cleanup`, `activate_or_revive_session_for_owner` | SE1 never live and ended at once. SE2 the idle sweep never ends a session used after it was picked. SE3 a live session has no unfinished cleanup. SE4 a deferred end exists only behind a call. SE5 cleanup never starts while an admitted call runs | an agent's session ends under it, or a recording, cursor or grant is torn down mid-action |
| Permission admission | `cua-driver-sdk/models/admission.tla` | `cua-driver-sdk/src/runtime.rs` `Admission`, `set_awaiting_permissions`, `invoke_with_context_and_evidence`, `RuntimeSession::invoke`, `shutdown`; `cua-driver/src/serve.rs` `settle_permission_gate`; `cua-driver/src/main.rs` serve startup | PG1 no tool runs while the gate is pending. PG2 a timed-out gate opens once grants arrive. PG3 a closed runtime never opens again | the daemon runs a desktop tool without Accessibility or Screen Recording, and macOS caches the negative TCC answer |
| macOS foreground restore | `platform-macos/models/foreground_restore.tla` | `platform-macos/src/input/skylight.rs` `foreground_turn`, `with_foreground_assist`, `with_foreground_hid_activation`; the same shape in `tools/type_text.rs` (`reactivate_application`) and `tools/invoke_menu.rs` (foreground restore); `focus_arbiter.rs` `UndoWatch::decide` | FR1 never undoes an app switch you made during the turn. FR2 an uninterrupted turn gives your app back | the driver yanks you back to the app you just left |
| invoke_menu cancel | `platform-macos/models/invoke_menu_cancel.tla` | `platform-macos/src/tools/invoke_menu.rs` `PressAdmission`, `CancelOnDrop`, `run_admitted`, `invoke_path_in_background`, `run_in_background`; `cua-driver-core/src/tool.rs` `DesktopLane`, `DesktopCall`, `DesktopLease`; `platform-macos/src/background_mutation.rs` | MC1 a worker whose call was already cancelled does not press. MC2 no other desktop action runs while a menu press is in flight | a menu command fires for a call that already returned, or overlaps the next action's input |
| Recording | `cua-driver-core/models/recording.tla` | `cua-driver-core/src/recording.rs` `Recorder`, `start_with_redactions`, `stop`, `finalize`, `begin_turn`, `finish_turn`; the idle TTL stop in `cua-driver-sdk/src/runtime.rs` | RC1 a turn is written into the recording it was reserved under. RC2 no live recording belongs to an ended session. RC3 a finished directory has a final `session.json`. RC4 a failed start leaves no half-torn recording. RC5 an idle stop never stops a later recording | evidence is written to the wrong recording, or a recording directory is left without its final metadata |
| Windows typing | `platform-windows/models/windows_typing.tla` | `platform-windows/src/input/keyboard.rs` `with_confirmed_foreground` and the text body of `type_text`; `platform-windows/src/typing_turn.rs` `restores_previous` | NoLateKeystroke no keystroke meant for the target lands in your window. NoSteal no taking the front back from a window that took it. GivesWindowBack a drained turn gives your window back | typed text lands in the window you were using |
| Serve drain | `cua-driver/models/serve_drain.tla` | `cua-driver/src/serve.rs` both `run_serve` functions (Unix and Windows), `release_active_proxy_sessions`, `claim_standalone_socket`, `claim_daemon_lock` | SD1 a successor daemon never serves while the old one still runs a call. SD2 shutdown releases every proxy session before the SDK goes down | two daemons drive the desktop at once, or a proxy's session leaks past shutdown |

Each action in a spec carries a comment naming the function and `file:line` it mirrors. The line numbers were right at the sync commit below. After that, find the code by function name.

## Run

From the repository root:

```sh
.cursor/skills/formally-verify-cua-driver/scripts/check-models.sh          # every row, about 20 s
.cursor/skills/formally-verify-cua-driver/scripts/check-models.sh session  # rows whose config path contains "session"
```

It needs Java 11 or later (`JAVA=/path/to/java` if it is not on `PATH`) and `curl`. The first run downloads `tla2tools.jar` v1.7.4 into `~/.cache/cua-driver-tla` and checks its sha256. Set `TLA2TOOLS=/path/to/tla2tools.jar` to use a copy you already have. Each TLC run gets 300 s (`TLC_SECONDS`), and every model here finishes in under 5 s.

[`models.tsv`](models.tsv) lists every config with the result it must give: `pass`, or the one invariant that must fail first. The script prints `ok` or `FAIL` per row, keeps TLC's output in the printed temp directory, and exits non-zero on any mismatch. To run one config by hand:

```sh
cd libs/cua-driver/rust/crates/cua-driver-core/models
java -XX:+UseParallelGC -cp ~/.cache/cua-driver-tla/tla2tools-1.7.4.jar tlc2.TLC -workers 1 -config session.cfg session.tla
```

Read every counterexample, and draw every model you change, with the `/statechart` skill. It gives each broken rule's shortest path in plain steps, and when the model passes it draws a chart of the machine.

## What each row means

There are three kinds of row:

- **The code on main.** It passes, or it fails on a finding listed below.
- **Able-to-fail variants.** A switch puts back the bug a past PR fixed, or removes one guard. The row must fail. A variant that starts passing means the model lost the step that guard covers. Fix the model, not the row.
- **Fixed variants** (`*_fixed.cfg`). The smallest change that makes an open finding hold. A fix PR for that finding should make the main config behave like this one.

Open findings on main at the sync commit:

| Row | Rule that breaks | Shortest path in the code's terms |
| --- | --- | --- |
| `session.cfg` | SE5 | 1. a call is admitted, runs and ends. 2. `end_session` sees `in_flight == 0` and releases the records lock (`end_session_with_reason`). 3. a new call is admitted (`begin_session_dispatch_inner`). 4. the end runs `mark_session_ended`, which removes the record without reading `in_flight`, and the cleanup hooks run while that call is still in flight. Its guard drop then finds no record. |
| `foreground_restore.cfg` | FR1 | 1. a foreground turn fronts the target. 2. you Cmd-Tab to another app. 3. `if prev_ok { set_front(prev_psn) }` pulls the old app back over the one you chose. |
| `invoke_menu_cancel.cfg` | MC2 | 1. the worker passes `PressAdmission::admit`. 2. the caller's future is dropped, so `CancelOnDrop` fires and the call's `DesktopCall` and per-pid lease drop with it. 3. the worker is in `AXPerformAction`. 4. the next desktop action takes the lane and the pid lease and runs during the press. The worker holds neither a `DesktopLease` clone nor the pid lease, unlike `bring_to_front`'s worker. |
| `serve_drain_windows.cfg` | SD1 (SD2 too) | 1. shutdown starts while a call is in flight. 2. the Windows loop drops the pipe lock before `sdk.shutdown` and never joins its detached connection tasks. 3. a new daemon takes the name and serves while the old call still runs. Proxy sessions are never released. |

## Real bug or modelling error?

A counterexample is a claim about the code. Before you report it, replay it against the code one action at a time. For each step, open the function the action's comment names and check three things: the step can happen there, it is one critical section in the code (the same lock held from start to end, or one atomic), and nothing in the code between two steps rules out the next one. If a step cannot happen, the model is wrong. Fix the model, rerun, and say in your report that you did.

Then make it real where you can: a unit test that forces the interleaving with barriers or channels (as `session.rs`'s tests do with `std::sync::Barrier`), or a run on the `verify-cua-driver` lane. Report the trace in the code's words, never in TLA+ names.

Never weaken a rule or delete an action to make a row pass. If the code changed on purpose, change the model to match it, and change `models.tsv` only when the expected result really changes. Say why in the PR.

## Changing or adding a model

- One indivisible step in the code is one action in the model. A lock released and taken again is two actions.
- Name each action after the code, and comment it with the function and `file:line` it mirrors.
- Put every rule the reader cares about in `INVARIANTS`, with a one-line comment in plain words.
- Give each switch a comment saying what TRUE and FALSE mean in the code. Add an able-to-fail config for each guard the rules depend on, and see it fail before you trust a pass.
- Keep the state space small: two or three actors and bounds of 1 to 3. If TLC runs past a minute, add a bound and say in the spec header what it leaves out.
- Every `.tla` and `.cfg` starts with the MIT SPDX header (`\* SPDX-License-Identifier: MIT`) the neighbouring Rust files use.
- Add the rows to `models.tsv` and the machine to the coverage table.

## What is not covered

These are models. A pass covers the model at its bounds, not the code. They leave out: more than one session, call or caller at a time (except where a row says so); real time, so deadlines and TTLs are only orderings; cleanup hooks that keep failing; the unattached Windows typing path (`TypingTurn::Settled`, a 40 ms sleep); the Linux foreground table and desktop identity; the macOS key-focus guard's order watch; per-turn recording content; and the daemon's launch race, which is fixed by the lock files and not modelled here.

## Keeping it in sync

**Sync commit: `cc67e715d11b7450a82663bf7e308e6680422beb`.** At that commit every action's comment matched the code it cites, and `check-models.sh` gave the results in `models.tsv`.

Rerun the affected rows on every change to a file in the coverage table. Rerun everything before a release and after any change to locking in `session.rs`, `runtime.rs`, `tool.rs`, `serve.rs` or `skylight.rs`. To see what moved since the sync commit:

```sh
cd libs/cua-driver/rust/crates
git log --oneline cc67e715d1..HEAD -- \
  cua-driver-core/src/snapshot_store.rs cua-driver-core/src/element_token.rs \
  cua-driver-core/src/session.rs cua-driver-core/src/recording.rs cua-driver-core/src/tool.rs \
  cua-driver-sdk/src/runtime.rs cua-driver/src/serve.rs cua-driver/src/main.rs \
  platform-macos/src/input/skylight.rs platform-macos/src/focus_arbiter.rs \
  platform-macos/src/tools/invoke_menu.rs platform-macos/src/tools/type_text.rs \
  platform-macos/src/background_mutation.rs \
  platform-windows/src/input/keyboard.rs platform-windows/src/typing_turn.rs \
  '*/models/*'
```

When you have reread each changed function against its model actions and the rows give their expected results, move the sync commit forward to the commit you checked. Do not move it on a green run alone.
