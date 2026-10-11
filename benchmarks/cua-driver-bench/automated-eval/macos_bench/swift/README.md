# macOS pilot test apps

Two small single-file AppKit apps used by the macOS pilot.

- **BenchSentinel** (`ai.cua.benchsentinel`) is a witness. It sits frontmost and key while an agent works in other apps, so a run can be scored for focus theft, leaked keystrokes, clicks and scrolls, and pointer movement.
- **BenchLab** (`ai.cua.benchlab`) is the test bench. One app, six modes, each paired with a probe in `../probes/`.

Neither app is committed as a binary. Build them with:

```bash
./build.sh /tmp/bench-build   # writes BenchSentinel.app and BenchLab.app, ad-hoc signed
```

`build.sh` needs Xcode (`xcrun swiftc`), targets macOS 26 on arm64, and signs with `codesign --sign -` only. Each bundle has exactly one file in `Contents/MacOS`.

## BenchSentinel

```text
BenchSentinel --log PATH [--no-activate]
```

One 360x140 window titled "Bench Sentinel" at the bottom-right of the primary screen, with one focused text field. It activates at launch unless `--no-activate` is given (a testing aid that the pilot does not use).

Signals, handled with `DispatchSource` on the main queue:

| Signal | Effect |
| --- | --- |
| `SIGUSR1` | Re-activate and make the window key. If the app is still inactive after 300 ms it asks LaunchServices to activate it. |
| `SIGUSR2` | Toggle ARMED. Logs `{"ev":"armed"}` or `{"ev":"disarmed"}`. |
| `SIGTERM` | Logs `exit` and quits. |

The orchestrator starts the app, sends `SIGUSR1` so it is frontmost, sends `SIGUSR2` just before the agent starts and `SIGUSR2` again when it ends, then runs `summarize_sentinel.summarize(log)`. The app starts disarmed; only the first armed window is summarized.

### Log format

JSONL, appended, one `write(2)` per line. Every line has `ev`, `t` (epoch ms as a float) and `armed` (bool).

| `ev` | Extra fields |
| --- | --- |
| `s` (20 Hz sample) | `mouse` `[x,y]` (global, `NSEvent.mouseLocation`), `front` `{bid,pid}`, `active`, `key`, `idle` `{move,down,key,scroll}` from `CGEventSource.secondsSinceLastEventType(.hidSystemState, ...)` |
| `armed`, `disarmed` | the same fields as a sample |
| `keyDown` | `chars`, `keyCode` |
| `mouseDown`, `mouseUp` | `button`, `loc` |
| `scrollWheel` | `dx`, `dy` |
| `text` | `len` (text field length after a change) |
| `didBecomeKey`, `didResignKey`, `didBecomeActive`, `didResignActive` | `front` at that moment |
| `front` | `front` of the app that just became frontmost (from the workspace notification) |
| `start`, `activate`, `activate_fallback`, `exit` | bookkeeping |

`summarize_sentinel.py` returns `front_changes`, `front_changed_to`, `key_loss`, `activations_lost`, `keystrokes_leaked`, `clicks_leaked`, `scrolls_leaked`, `pointer_max_deviation_px`, `pointer_deviation_episodes`, `hid_events` (`move`, `down`, `key`, `scroll`), `samples`, `duration_s` and `available` (at least 10 samples). Front changes are counted from samples and `front` events only. `hid_events` counts the times an idle value drops between consecutive lines, which means a real HID-level event happened; events posted straight to a process do not reset those timers.

## BenchLab

```text
BenchLab --mode forms|table|canvas|canvasclick|hover|clipboard --seed N --state PATH --events PATH
```

A 760x560 window titled "BenchLab" with its top-left at (60, 80) on the primary screen. It never calls activate itself.

- `--state`: the full state as JSON, rewritten atomically after every event.
- `--events`: JSONL, one line per meaningful UI event: `{"seq":n,"t":ms,"type":"...","details":{...}}`. `seq` starts at 1 (`app_start`) and the state file carries the latest `seq`, so the evaluator can check that the two belong together.
- The state never contains an expected answer. Evaluators recompute everything from the seed. Restarting the app mid-run resets `seq` and invalidates the run.

In canvasclick mode a click is a `mouseDown` and a `mouseUp` of the same button on the same circle. The state keeps per-circle left and right click counts and an ordered `click_log` of `(button, circle, label, x, y)`; a click that starts or ends outside every circle has `circle: null`.

Mouse events in canvas and hover mode use `{"phase": "mouseDown|mouseDragged|mouseUp|mouseEntered|mouseExited|mouseMoved", "x", "y"}` in the receiving view's coordinates (origin top-left).

| Mode | What it shows | Notable events |
| --- | --- | --- |
| `forms` | Labelled fields: Customer name, Invoice amount, Category (8-item pop-up), Priority (3 radios), Notify me, Quantity (field plus stepper), Notes (text view), Submit, status. | `field_edit`, `submit` |
| `table` | 400-row view-based `NSTableView` (Code `K-0001`..`K-0400`, Name, Qty, Flag checkbox), Save, status. Rows outside the viewport are not in the accessibility tree until scrolled (see below). | `flag_toggle`, `scroll`, `save` |
| `canvas` | One custom 700x420 view with three 56 px tiles and three 96 px target zones, drawn as pixels with no per-tile accessibility children. A Done button below. | `mouse`, `done_click` |
| `canvasclick` | One custom 700x420 view with 24 numbered circles (radius 14) on a jittered 6x4 grid, again pixels only. A Done button below. A left-clicked circle turns darker and a right-clicked one gets an orange ring. | `mouse` (`mouseDown`, `mouseUp`, `rightMouseDown`, `rightMouseUp`, with `button`, `circle`, `label`), `done_click` |
| `hover` | A toolbar strip with an "Actions" hot zone. While the pointer is in it, four buttons appear below. They are hidden (and absent from the accessibility tree) otherwise. | `mouse`, `overlay_shown`, `overlay_hidden`, `overlay_click` |
| `clipboard` | A Result field, Save and status. The agent computes `A x B + C` in Calculator. | `field_edit`, `save` |

Hover behavior: the overlay shows on `mouseEntered` or `mouseMoved` in the hot zone and hides 400 ms after the pointer has left both the hot zone and the overlay. Each click records whether the overlay was visible.

Table accessibility: stock AppKit lists all 400 rows to an accessibility client and realizes their cells while it walks, which in testing made a tree walk run past 20 seconds and hid the Save button behind the element cap. BenchLab therefore uses a thin `NSTableView` subclass that reports only the rows in the viewport as accessibility children and rows (the stock row elements, sliced), so rows enter the tree as they are scrolled into view. Pass `--ax-rows all` to get the stock behavior.

Forms and clipboard also poll the controls every 250 ms and log `via: "poll"` edits, so values set without key events are still logged.

## Seed derivation

All task parameters come from `--seed` through splitmix64. The Swift code (`BenchLab.swift`) and the Python code (`../probes/_common/benchlab_common.py`) implement it identically, and the evaluators recompute the expected values from the seed alone.

```text
state = seed (uint64)
next():   state += 0x9E3779B97F4A7C15 (mod 2^64)
          z = state
          z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9 (mod 2^64)
          z = (z ^ (z >> 27)) * 0x94D049BB133111EB (mod 2^64)
          return z ^ (z >> 31)
index(n)      = next() % n
randint(a, b) = a + next() % (b - a + 1)           (inclusive)
shuffled(xs)  = Fisher-Yates: for i = n-1 down to 1: swap xs[i], xs[next() % (i+1)]
```

Seed 0 yields `0xE220A8397B1DCDAF` first. Each mode starts a fresh generator from the seed and draws in this order:

| Mode | Draws |
| --- | --- |
| forms | name `index(20)`; dollars `randint(200,9800)`; cents `[0,25,50,75][index(4)]`; category `randint(1,7)`; priority `index(3)`; notify `index(2)==1`; quantity `randint(2,99)`; notes `index(10)` |
| table | three target rows `randint(30,130)`, `randint(131,260)`, `randint(261,400)` |
| canvas | zone columns `shuffled([0,1,2])`; for each color (red, green, blue): zone x `col*233 + randint(8,129)`, y `randint(16,150)`; tile columns `shuffled([0,1,2])`; for each color: tile x `col*233 + randint(8,169)`, y `randint(310,356)` (top-left, y down) |
| canvasclick | circle labels `shuffled(1..24)` (label of grid cell i, row-major); for each cell i in 0..23: `jx = randint(-30,30)`, `jy = randint(-26,26)`, center `(i%6*116 + 58 + jx, i//6*105 + 52 + jy)`; then `perm = shuffled(1..24)`: `perm[0:12]` is the left-click order and `perm[12:16]` are the right-click targets |
| hover | `shuffled(["Archive","Duplicate","Export","Pin","Share","Rename"])`, first four are the buttons left to right; target index `randint(0,3)` |
| clipboard | A `randint(120,989)`; B `randint(11,97)`; C `randint(1000,99999)` |

The name, category, priority and notes tables live in both files; a unit test checks that they match. Table row names and quantities are decoration drawn from a separate stream (`seed ^ 0xA5A5A5A5A5A5A5A5`) and are not used by any evaluator.

To confirm that Swift and Python agree, run `BENCHLAB_PARITY=1 python -m unittest discover -s ../probes/tests`. It compiles a variant of BenchLab with `-D BENCHLAB_DUMP`, which adds a `--dump-params` flag, and compares about 50 seeds. The release build does not contain that flag.

## Tests

`BenchLab.swift` also contains a test-only `BENCHLAB_SELFTEST` variant (not in the release build) that drives each mode through its own controls with synthetic events one second after launch, so the Swift output can be fed to the Python evaluators without real input.

```bash
python -m unittest discover -s swift/tests        # summarizer, from automated-eval/macos_pilot
python -m unittest discover -s probes/tests       # evaluators, briefs, PRNG
```
