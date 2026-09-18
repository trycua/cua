# cua-driver optimization for OSWorld (branch `feat/osworld-cuadriver-optimization`)

Worktree `O:\projects\cua-osworld\cua-opt`, head `f16420178` (13 commits over `origin/main` at `b4e3caecd`, cua-driver 0.26.1). Not pushed. All test binaries green in the Ubuntu 22.04 build container (`platform-linux` 453, `cua-driver-core` 612, `cua-driver-contract` 40, `schema_consistency_test`, `protocol_schema_test`, `contract_parity`).

Sub-branches merged in: `feat/opt-foreground` (074c5776f..3d5fb913f), `feat/opt-atspi-background` (d8788d348..d68b8356d), then 245bd4517 (merge), dfbe3c93e, 54e276fdd, 197ba7096, f16420178.

## What changed

**Foreground delivery** (`crates/platform-linux/src/input/foreground.rs`, new): x11rb-based `with_x11_foreground_opts`; activate (`_NET_ACTIVE_WINDOW` + `XSetInputFocus`), confirm focus on the active window and the core-focus tree, one re-activation at 40 % of the budget (pointer 800 ms, keyboard 1500 ms), watchdog threads around confirm and post-check, **no restore afterwards**; structured `foreground_unavailable` / `foreground_timeout` codes and `focus_after: target|same_pid|elsewhere`. `type_text` foreground goes straight to activate → optional AT-SPI focus → XTest; no more `focused_is_editable`/Qt5 probing. Element clicks in foreground: activate + XTest at the element centre. Pointer results say "not verified — confirm with a screenshot" and carry `effect: suspected_noop` when focus left the process.

**AT-SPI query engine** (`crates/platform-linux/src/atspi/{native,mod,cache}.rs`, `tools/impl_.rs`): `get_window_state.timeout_ms` (default 1000, 100..120000) bounds retries + walk + bounds under one task-local deadline; every D-Bus call is clamped to the remaining budget; partial trees return `truncated`, `truncation_reason` (`timeout|node_budget|app_unresponsive|app_lookup_timeout|huge_container`), `nodes_visited`, `nodes_pending`, `bounds_complete`, `walk_elapsed_ms`, and a one-line note telling the model to retry with a larger `timeout_ms` or narrow with `query`/`max_depth`; `elements_complete` is truthful. Containers above 2048 children are never expanded (LibreOffice Calc's sheet reports 2^30). Snapshot cache stores element identity + frame, so element clicks/actions/focus reuse it (`perform_action_in`, `focus_element_ref`, `element_bounds_ref`). Input-time AT-SPI budgets: 1.5 s query, 6 s index resolve, 30 s AX action (`ax_timeout`). Focus-event tracker makes the background keyboard refusal instant (0.1 s, was 13-34 s). `list_windows` / `get_desktop_state` text carries `pid=N window_id=M` per window.

**Daemon deadlock fix** (dfbe3c93e): the cursor-overlay arrival await is capped at 5.5 s with a degraded latch; this was the cause of the multi-minute daemon-wide hangs (see PAPERCUTS.md #37).

**Coordinate frames** (a3bccdcea, 4006e4d15, 54e276fdd, 197ba7096, f16420178): `scope: "desktop"` / `coordinate_frame: "desktop"` with a pid means desktop pixels (translated once); `get_desktop_state` now returns a ≤1280-px screenshot with `frame_scale` and scales desktop-frame actions back (the model's vision downsizes 1920-px images, so raw coordinates were ~20 % short); an ambiguous pid resolves to the topmost window under the target point; pid-only keyboard actions fall back to the pid's active window.

## Verification

Swap test (fresh VM, binary hot-swapped, `opt/swaptest.sh` + `swaptest_fg.sh`):

| Probe | Stock 0.25.0 | Branch |
| --- | --- | --- |
| GIMP menubar pixel click, background | nondeterministic | opens menu, 1.6 s |
| GIMP menubar pixel click, foreground | no effect | opens menu (window-local and desktop-scope) |
| GIMP element click (cached ref) | 5.5 min hang, daemon wedged | opens menu, 1.1 s, both modes |
| Calc `type_text` foreground | **X session freeze** | text in A1, 0.4 s, guest alive |
| Calc `type_text` background | refusal after 13-34 s | same refusal in 0.1 s |
| Calc `get_window_state` | 25 s timeout → guest death | 1.2 s (default) / 2.5 s (`timeout_ms: 5000`), labeled partial |

OSWorld subset (5 tasks: gimp x2, vlc, vs_code, libreoffice_calc; Haiku 4.5, 25 actions, harness forces one delivery mode; classic pyautogui baseline 5/5, stock cua-driver 1/5):

| Run | Score | Notes |
| --- | --- | --- |
| foreground, first merged binary | 0 / 5 | no stalls, coordinates still 20 % short |
| foreground, after screenshot scaling | 3 / 5 | GIMP x2 and VLC pass; VS Code wrote the wrong setting, Calc wrong formulas; no driver errors, no call over 10 s |
| foreground, final binary (fg3) | 3 / 5 | same |
| background, merged | 0 / 5 | no hangs; 42 of 50 errors are the by-design keyboard refusal |

Full 15-task run, foreground mode, final binary (`merged-fg-full`, same tasks/model/budget as EVAL.md):

| Condition | Solved | By domain (solved/total) |
| --- | --- | --- |
| classic pyautogui (baseline) | 12 / 15 (80 %) | os 1/1, gimp 2/2, vlc 2/2, vs_code 3/3, calc 3/3, writer 1/2, impress 1/2 |
| stock cua-driver 0.25.0 | 2 / 15 (13 %) | |
| **branch, foreground** | **8 / 15 (53 %)** | os 1/1, gimp 2/2, vlc 2/2 (one at 0.99), vs_code 3/3, calc 1/3, writer 0/2, impress 0/2 |
| branch, background-only, MPX + uinput (full 15) | 6 / 15 (40 %) | see the MPX section |

Everything outside LibreOffice now matches the pyautogui baseline (8/8). LibreOffice is 1/7 vs 5/7. From the per-call log of the six failures: no hangs, but `get_window_state` on LibreOffice windows still took 13-43 s five times (budget overrun on the bounds/retry path, worth a follow-up), the model burned budget re-calling `get_window_state` (up to 10 times in one episode), invented tools (`triple_click`) and passed numbers as strings (`InputValidationError` in four episodes), and typed wrong formulas; one `pid owns more than one eligible top-level window` refusal remained in the Impress run. Foreground clicks and typing themselves landed.


## Background delivery via MPX + uinput (commits 17444e862 .. 4c40daa49)

The X11 equivalent of the Hyprland isolated seat: each session lazily creates its own XI2 master pointer+keyboard pair, a uinput keyboard (and pointer) is hot-added by the X server and attached to that pair, `XISetFocus` points the virtual keyboard at the target window, and real evdev events are written through uinput. Result path `mpx_uinput`; the honest refusal remains when `/dev/uinput` is not writable. Lifecycle: retained per session with an idle TTL, removed on `end_session` and by the startup reaper. Fixes found in the loop: thaw the retained virtual pointer after a shield grab, deliver clicks into override-redirect popups without the shield grab, choose a mapped toplevel for pid-only keys, prefer the virtual keyboard over AT-SPI `insert_text`.

Verified in the swap test: Calc typing lands (A1), ctrl+z undo, click-then-type, VS Code typing and hotkeys, GIMP menubar and menu-item clicks, with `XGetInputFocus` and the real pointer position unchanged and no stray `CUA` devices after `end_session`.

Background-only subset (same 5 tasks): 0/5 → 2/5 → 3/5 (`mpx-bg5`: GIMP x2, VS Code). Full 15-task background-only run (`mpx-bg-full`, commit 4c40daa49): **6 / 15 (40 %)** — os 1/1, gimp 2/2, vlc 1/2 (other at 0.99), vs_code 1/3, calc 2/3, writer 0/2, impress 0/2. Stock driver was 2/15 (13 %) with the model free to choose modes; the foreground-only run on this branch is 8/15 (53 %); the pyautogui baseline is 12/15 (80 %).

Image requirement: `build/99-cua-uinput.rules` + `build/uinput.conf` + `usermod -aG input user` (added to `build/build-osworld-cua.sh`).

## Remaining gaps
- Background-only keyboard input into GTK/VCL apps has no focus-free route on this image (PAPERCUTS #41); GIMP spin scales are not AT-SPI editable.
- Why the overlay renderer stops reporting arrival above a GIMP menu is undiagnosed (the cap makes it harmless).
- One stale-window-id `BadWindow` in `desktop_to_window_local` is surfaced, not retried.
- Windows/macOS accept `timeout_ms` in the schema but do not honor it yet.

## Round 8: Qt popups, labels, screenshot scale (branch `feat/opt-r8-qt-keys`)

From the round-7 background traces (VLC 59f21cfb, GIMP 554785e9) and the Calc
regression analysis. Probe: `part8_qt.py` (results `part8-qt*.md`).

| Finding | Root cause | Change |
| --- | --- | --- |
| Keys dropped while a Qt popup (combo list, completer) is open, reported "focus untouched" | The X server drops core key events from any *other* master keyboard to a client holding an active keyboard grab (`IsInterferingGrab`); the virtual master keyboard route is silently lost | `popup_of_pid` + `send_keys_under_popup_grab`: XTest on the core keyboard (the grab routes it to the target), core focus / active window verified after (`path=xtest_core_grab`); refused with `popup_keyboard_grab` when another pid owns the core focus |
| `get_window_state(pid, popup)` returned the main menubar (Audio/Video/…) instead of the combo rows | Frames stayed in screen pixels when the override-redirect window's origin was unavailable, so the menubar "fit" the popup box; only `menu` roles were kept | Popup rectangle is the origin fallback; container-subtree scoping; list/tree/table rows win over menubar entries |
| AT-SPI `Description` never read | — | Read for controls; rendered `(description "Pause")`, `label` falls back to it |
| Four GIMP spin scales all `label:"0.0"` | GtkSpinButton implements AtkText with its number; the text became the name | Value controls are never named after text; LABELLED_BY, description (GIMP keeps "Hue"/"Lightness"/"Saturation" there), sibling label, else `unlabelled` + "3rd of 4 spin buttons in this window" |
| Drop onto VLC reported "3.6% changed" | Guard watches the target pid only | `foreign_window`: title before/after + new windows of the pid under the point, as window_change evidence |
| `parent_index` pointed at a preceding push button | "nearest preceding actionable at lower depth" | Real pre-order ancestor chain |
| Model called `triple_click` | — | `click` documents `count: 2/3`; verified: count 3 selects a whole line in gedit, background and foreground |
| Model's Calc coordinates 0.94x short | The API downsizes images above ~1.15 MP; a 1568x861 window shot reached the model as ~1447x795 | Window screenshots capped at 1.15 MP, element frames scaled to the delivered image, `frame_scale` reported; verified: a click at screenshot pixels selects the same cell as a real click |
| One-row Calc misses found six values later | — | After a pointer press the focused cell is named (`focus: cell D2`) from `object:active-descendant-changed` |

Probe (`part8-qt.md`, `part8-qt-b.md`, `part8-qt-c.md`): VLC Open File end to end
by set_value + accessibility Press (title → "tone.wav - VLC media player"),
combo popup rows listed, Escape under the popup grab delivered with focus
verified unchanged, transport buttons described; GIMP Hue-Saturation spin
buttons named; gedit triple click; Calc 1337x860 (1.15 MP) screenshot with
`frame_scale` 0.955 and cell-accurate clicks with `focus: cell`.

Subset run (`r8-bg-subset`, background, tasks 59f21cfb 554785e9 357ef137 abed40dc): see below.
