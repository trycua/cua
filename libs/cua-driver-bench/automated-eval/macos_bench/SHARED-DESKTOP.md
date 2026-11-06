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
