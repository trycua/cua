# Shared-desktop tasks: PA-01 and UM-01 (design note)

Two tasks that measure whether a computer-use driver can share one desktop: with other agents (PA-01) and with
a person who keeps using the mouse (UM-01). They extend Bench v2. Like every v2 task they are deterministic and
checker-graded: seeds, BenchLab state and event logs, and the sentinel's log decide every result. No model grades.

They are not part of the ten registered core tasks (`bench_v2.V2_TASKS`). They are selected by id
(`--tasks PA-01 UM-01`), the way WEB-01 and WEB-02 are, and a dated amendment registers them before their
first scored trial.

## Why these two tasks

* **Parallel agents on one desktop.** Teams now run several agents at once on one machine, each working in its
  own copy of an app. A driver that needs the real pointer and keyboard focus serialises them at best; at worst
  one agent's keystrokes land in another agent's window. No v2 task runs more than one agent.
* **A person who keeps using the mouse.** The background condition already has a user who types. A user who
  moves and clicks the mouse is the harder case: a driver that moves the real pointer, even briefly, drags the
  person's pointer away or makes their clicks land somewhere else. The v2 sentinel's pointer criterion assumes a
  still pointer, so it cannot measure this.

## The BenchLab `desk` mode (both tasks)

`BenchLab --mode desk --seed S --state PATH --events PATH --instance L --frame X,Y,W,H [--no-activate]`, on
macOS (`swift/BenchLabShared.swift`), Linux (`linux/benchlab.py`, GTK 3, X11 and Wayland) and Windows
(`windows/benchlab.py`, wxPython).

* Window title `BenchLab Desk L`, outer frame `--frame` in top-left screen coordinates. The app never activates
  itself; with `--no-activate` it also shows without taking focus.
* Top to bottom: a heading `Desk L`; a `Code` label and text field (accessible name `Code`); a `Shelf` label and
  twelve radio buttons in three columns, in the fixed order of `benchlab_common.DESK_SHELVES` (Amber ... Lumen),
  none selected at start; a `Submit` button (not the default button: Return does nothing); a status line
  (`Not submitted`, then `Submitted: <code> / <shelf>`).
* The app derives nothing from the seed and never knows the expected answer.
* State (rewritten atomically after every event): `mode` `desk`, `seed`, `seq`, `instance`, `code` (the field's
  text), `shelf` (selected shelf or null), `submit_count`, `submitted` (`{code, shelf}` of the last Submit or
  null).
* Events (JSONL, the usual `{seq, t, type, details}`): `app_start {mode, seed, instance}`, `field_edit
  {field: "code", value}` on every text change, `shelf_select {shelf}`, `submit {code, shelf}`. Every Submit is
  accepted and counted.

## PA-01: parallel agents on many app instances

**Starting state.** N copies of BenchLab in `desk` mode (N = 10 headline; N = 4 also reported; N = 1 is the
speed-up baseline), one process each, in a fixed 5 x 2 grid: cell k has its top-left at
(20 + 380 * (k mod 5), 60 + 440 * (k div 5)) and an outer size of 360 x 400 (`benchlab_common.pa_cell_frame`).
Cells are filled in reading order. The seed decides which label (A to J) sits in which cell, and each cell's code
and target shelf (`derive_parallel(seed, N)`; cell k is the same for every N, so N = 1, 4 and 10 pair by seed).

**Agents.** The harness starts N agent processes at the same moment, one per copy, each with the same setup,
system prompt, model and limits as any v2 trial. Agent k's brief names its copy by title and grid position:

```text
{{setting}}

Your copy is the window titled "BenchLab Desk {{label}}", at {{position}} of the grid (counting from the top
left). Work only in that window: do not click, type into, move, close or otherwise change any other BenchLab
window.

In your window:

1. Type the code {{code}} into the Code field.
2. Select the "{{shelf}}" shelf.
3. Press Submit once.

Do not close BenchLab.
```

`{{setting}}` is "N BenchLab windows are open on your Mac, laid out in a grid. Each is a separate copy of the app,
and a different agent is working in each of the other N - 1 copies at the same time as you." (for N = 1: "One
BenchLab window is open on your Mac, at the top left of the screen."). The platform word follows the platform,
as for the other tasks.

**Codes are attributable.** Each code is two letters, a hyphen and four digits. The 20 code letters of the ten
cells are distinct (drawn from 24 letters without I and O), so a letter of another copy's code in a copy's Code
field can only have come from that other agent.

**Checker** (`probes/PA-01/evaluate.py`, one block of checks per copy, `benchlab_common.check_desk`). Required
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

**Budget.** 360 s of wall clock for the whole trial, shared by the N agents (each agent process gets the same
360 s and the usual 45 turns). A driver without multi-session support may serialise the agents (a lock, a
queue, one pointer at a time): that is allowed, and its copies count if they are correct and submitted within
the budget. The report shows its throughput and speed-up next to everyone else's.

**Conditions.** Foreground and background. In the background condition the usual "User activity" window types
at (420, 340, 900, 520), in front of part of the grid, and every agent's brief gets the usual background
paragraph. A full pass in the background condition is a checker pass, no GUI-only violation by any agent, and
the eight undisturbed criteria of Amendment 22.

**GUI-only guard.** Every agent's stream is scanned; any agent's violation ends all N agents and fails the trial.

## UM-01: the user's mouse keeps working

**Starting state.** One BenchLab `desk` copy at (200, 200, 360, 400), its label, code and shelf from
`derive_um_desk(seed)`. The sentinel is the "User activity" window at (420, 340, 900, 520), in front and key,
covering the right part of the desk. Its content is a drawing area (840 x 360, canvas-local coordinates from
its top-left) with eight numbered dots.

**The agent's task.** The same desk sub-task as PA-01, one agent:

```text
BenchLab is already open on your Mac, in a window titled "BenchLab Desk {{label}}".

In that window:

1. Type the code {{code}} into the Code field.
2. Select the "{{shelf}}" shelf.
3. Press Submit once.

Do not close BenchLab.
```

followed by the background paragraph with "and is typing in it" replaced by "and is moving the pointer and
clicking in it" (`bench_v2.BACKGROUND_BRIEF_MOUSE`).

**Condition.** Background only (a user who is using the mouse is the background condition by definition).

### Mouse user script

`derive_mouse_user(seed)` writes the script the runner hands to the sentinel (`--mouse-user PATH`): the
canvas size, `home`, eight dots (radius 18, at least 110 px apart), a sequence of 24 dot indices with no repeat
in a row, and the timings. While armed the sentinel:

1. moves the real pointer to `home` (no check on this first move);
2. for leg j = 0, 1, ...: target `sequence[j mod 24]`; for each point of `mouse_leg(current, target, 9)`:
   **check**, then post a real pointer move to the point, then wait 25 ms; at the dot: wait 150 ms, **check**,
   post a left button down at the dot centre, wait 60 ms, post the button up, log `mu_click`, wait 250 ms;
3. stops at once when disarmed.

**Check.** Read the real pointer, convert to canvas coordinates and log
`{"ev": "mu", "i", "want", "got", "d"}`, where `d` is the distance from `got` to the last commanded point or the
one before it, whichever is smaller (one step of lag is not a displacement).

**Clicks.** The canvas logs every button-down it receives as `mu_rx {dot, at, own}`. The sentinel resolves
each scripted click within 400 ms as `mu_click_result {n, ok, why}`: `landed` (the canvas got it on the intended
dot within 3 px), `wrong_dot`, `off_target` or `lost` (no button-down reached the canvas: the click went to
another window or was swallowed). A button-down that is not the user's own is a leak, logged as `mouseDown`
like any other.

**Telling the user's input from anyone else's.** Where the platform can tag synthetic input, the sentinel tags
its own events and logs every untagged pointer event it sees system-wide as `mu_foreign {kind}`:

| Platform | User's pointer | Own-event tag | Foreign pointer events |
| --- | --- | --- | --- |
| macOS | `CGEventPost` (HID tap), session event tap reads | `kCGEventSourceUserData` = magic | listen-only CGEventTap |
| Windows | `SendInput` | `dwExtraInfo` = magic | `WH_MOUSE_LL` hook |
| Linux X11 | XTest | none | not measured (`null`): trajectory only |
| Hyprland | the compositor's cursor move dispatcher, clicks via `wlrctl` or `ydotool` | none | not measured: trajectory only; agent seats of the Cua plugin have their own pointer |

Where there is no tag, the trajectory check is the measure: the sentinel compares the observed pointer with
its own script 40 times a second, and a button-down is the user's own only if it matches a pending scripted
click (within 400 ms and 3 px).

### Scoring

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

## Hard variants: PA-02 and UM-02

PA-01 and UM-01 are registered and measured as written above, and nothing in this section changes what they
derive, run or score. The two variants below are new task ids with their own seeds and their own tables. They
exist because the first measurements showed a ceiling: a driver that acts on an app's window directly, without
the real pointer or keyboard focus, passes every PA-01 and UM-01 trial on every platform, so the tasks separate
drivers that need the foreground from drivers that do not, and then stop separating. Each variant keeps the
sub-task (a code, a shelf, Submit) and adds one pressure that a clean per-window driver can still fail.

| What a trial never asked for in PA-01 / UM-01 | PA-02 / UM-02 adds |
| --- | --- |
| Two windows that overlap, so a click on screen coordinates can reach the wrong copy | PA-02: a 4 x 4 cascade; each row covers the lower 195 px (Submit, status, the lower shelves) of the row above |
| Windows that cannot be told apart by title | PA-02: every copy has the same window title; only the heading `Desk L` and the grid position differ |
| Typing with real exposure to a stray keystroke | PA-02: a 15-character code with capitals, small letters, digits and a symbol (`TAG-1234-aBcDe#`), so 16 agents type about 240 characters |
| A control label that exists in more than one window | PA-02: a Confirm step (`Confirm`, `Cancel`) after Submit, the same labels in all 16 copies |
| A deadline per copy | PA-02: each copy's confirmed Submit must come within 180 s of the agents' start (the trial budget is 240 s) |
| A user who clicks about once per 1.4 s | UM-02: a pointer that moves at about 1,500 px/s and clicks about every 0.33 s (about 4 x the events) |
| A user whose button is held | UM-02: every third action is a drag, the button held from one dot to another |
| An agent target that is mostly visible | UM-02: the user's window covers all but a 100 px strip of the desk (including Submit and most shelves) |

Why these and not others. A modal dialog window per copy (all opening at once, same title) was not built: a new
window that takes keyboard focus makes every driver fail the key-focus criterion in the background condition, so
it would measure BenchLab, not the driver; the inline Confirm step keeps the identical-label trap without a new
window. A drag the agent must make was not built: it needs a new custom-drawn widget in three toolkits and an
accessibility-value shortcut would bypass it. The user typing as well as moving (UM-01's typist and mouse user
at once) and the user dragging windows or scrolling were not built; the typist's criterion is already measured
by the background condition of every core task, and window drags and scrolls need window-manager input on
every platform. They are candidates for a later amendment, not hidden omissions.

### PA-02: the crowded desk

* **Starting state.** N copies (N = 16 headline; 8 and 1 also reported; cell k is the same for every N) of the
  `desk` mode with `--title "BenchLab Desk" --confirm`, in a 4 x 4 cascade: cell k at
  (20 + 400 * (k mod 4), 36 + 205 * (k div 4)), outer size 360 x 400, opened in cell order so lower rows sit on
  top where the platform keeps the opening order. A 1920 x 1080 screen is needed (the grid ends at y = 1051).
  The foreground condition's witness window is in the free strip at (1610, 40, 290, 120).
* **`--title TEXT`** sets the window title; **`--confirm`** makes Submit a two-step action: Submit records
  `confirm_open {code, shelf}`, sets state `confirm_pending`, disables Submit and shows `Confirm` and `Cancel`
  (hidden before). Confirm records `submit {code, shelf}` with the values at that moment and hides them; Cancel
  records `confirm_cancel`. Without `--confirm`, Submit is as in PA-01.
* **Codes.** `TAG-dddd-xxxxxs`: a 3-letter tag unique to the copy, four digits, five characters from
  `abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789` and one symbol of `#@%+&`. No two codes share a tag
  or a 5-character fragment, so a fragment of another copy's code in a Code value is attributable to that copy's
  agent.
* **Brief.** PA-01's, with "overlapping" in the setting, the heading `Desk L` instead of the title, a warning
  that some windows hide parts of others, "exactly as written (capital and small letters matter)" and "Press
  Submit, then press Confirm."
* **Checker** (`probes/PA-02/evaluate.py`). Per copy, PA-01's checks with two changes and two additions:
  `no_foreign_input` is the tag/fragment rule; `in_budget` uses the copy's own deadline (180 s from the agents'
  start); `confirm_before_submit` (every `submit` is preceded by a `confirm_open`) and `confirm_resolved`
  (`confirm_pending` is false at the end) are required. A Confirm pressed in a copy whose agent has not filled it
  in submits that copy's current values, which fail its own `submit_values`. Cross-talk, throughput and speed-up
  are as in PA-01.
* **Conditions and full pass.** As PA-01 (foreground and background; background adds the eight undisturbed
  criteria and the GUI-only guard over all agents). The trial budget is 240 s.
* **Platforms.** macOS, Windows, Linux X11. Not Hyprland: it is rejected as a harness error there, never scored.

### UM-02: the busy user

* **Starting state.** One desk (the UM-01 sub-task and seed derivation, own salt) at (320, 300, 360, 400). The
  sentinel is the same "User activity" window at (420, 340, 900, 520), so it covers the desk except a strip 100
  px wide on the left and 40 px on top.
* **User script** (`derive_mouse_user(seed, "um02")`, schema `cdb-mouse-user/2`). The dots and sequence are
  UM-01's; steps of 12 px every 8 ms, dwell before 30 ms, press 25 ms, dwell after 40 ms. The `plan` walks the
  sequence: every third leg is a drag (`from` the dot the leg reaches, `to` the next dot, button held, steps of
  12 px every 8 ms, 40 ms hold), the others are clicks. A schema-1 script has no plan and plays as before.
* **Drags.** The canvas logs the button-up as `mu_up`. The sentinel logs `mu_drag` and resolves it as a
  `mu_click_result` with `kind: "drag"`: `landed` only when the down landed on `from` and the up on `to`; `lost`
  (no down reached the canvas), `cut` (the up did not arrive on `to`, or too late), `wrong_dot` or `off_target`.
  A drag counts as one scripted click in `clicks` and `clicks_landed`; `drags`, `drags_landed` and `drags_cut`
  are reported too. A foreign button-down or button-up is a leak.
* **Full pass.** As UM-01: checker pass, no GUI-only violation, the eight criteria with `pointer` (at most 3 px
  off the script, no foreign pointer event where tagging exists) and `typing` (every scripted click **and drag**
  landed, nothing else reached the user window).
* **Harness error.** Fewer than 15 scripted actions or 150 checks in the armed window, or no mouse-user log.
* **Platforms.** macOS, Windows, Linux X11 (trajectory only there, as UM-01). Not Hyprland.

## Relation to the eight undisturbed criteria

Both tasks reuse the sentinel and the eight criteria unchanged (key focus, window raised, typing, pointer,
clipboard, Space, dialog, activation). PA-01 adds no new user criterion: its new measure is between agents
(cross-talk), and it is scored by the checker. UM-01 changes what two criteria compare against: `pointer`
compares the pointer with the user's own script instead of a fixed point, and `typing` becomes the user's
clicks. `summarize_sentinel` reports the mouse user in `mouse_user` and puts the displacement in
`pointer_max_deviation_px`, so every consumer of that field reads the right number.

## Platform notes

* **macOS.** N BenchLab processes of the one bundle, told apart by window title, as a user would see them.
  The sentinel needs Accessibility (to post events) and Input Monitoring (for the listen-only tap) in the VM;
  without the tap, `foreign_pointer_events` is `null` and the trajectory check still applies.
* **Windows.** `SendInput` moves the real cursor; the low-level mouse hook flags injected events and reads
  `dwExtraInfo`. Screen coordinates are physical pixels (the sentinel is per-monitor DPI aware).
* **Linux X11.** XTest moves the pointer (`libXtst` via ctypes); nothing can tell XTest events apart, so the
  trajectory check is the measure. The grid needs a 1920 x 1080 screen.
* **Linux Hyprland (Omarchy).** Placement through Hyprland IPC as in the BenchLab port. The pointer moves
  through the compositor's cursor-move dispatcher; clicks need `wlrctl` (virtual-pointer protocol) or
  `ydotool` (uinput). If neither exists, UM-01 is a harness error on that desktop, never a fail.

## Threats to validity

* **Attribution of cross-talk** rests on the code letters. A keystroke of digits only, a hyphen or a shelf
  click in the wrong copy is not attributable; it still fails that copy's own checks (wrong code, wrong shelf or
  two Submits) but is not counted as cross-talk. An agent that mistypes its own code with another copy's letter
  is counted as cross-talk (unlikely: two given letters, read from the brief).
* **Concurrency load.** N agent processes and N tool servers run on one VM at once; a slow VM lowers
  throughput for everyone. Report the VM size and keep it the same across setups.
* **Rate limits.** N concurrent model sessions can hit API rate limits; a rate-limited agent is an
  infrastructure error and the whole trial is re-run.
* **Sampling.** The trajectory check samples at 40 Hz. A driver that moves the pointer and puts it back
  within one step can escape it on X11 and Hyprland; on macOS and Windows the foreign-event tap still sees it.
* **Synthetic user.** The user's motion is synthetic and regular (straight legs, fixed speed). It stands in for
  a person, not for every way a person moves a mouse.
* **Simulated multi-agent load.** The sub-task is short on purpose (three actions), so the measure is about
  sharing, not about how hard the task is.
* **Home field.** Cua wrote these tasks, BenchLab and the harness, and Cua Driver has multi-session and
  background features these tasks reward.
