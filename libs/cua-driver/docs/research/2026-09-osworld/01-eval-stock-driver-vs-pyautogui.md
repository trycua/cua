# Does driving OSWorld through cua-driver drop the score? (Haiku 4.5, 2026-09-09)

Claim under test: "cua-driver dropped OSWorld scores from ~80% to under 10%."

## Setup

- **Tasks:** 15 tasks from OSWorld `evaluation_examples/test_small.json` (upstream `xlang-ai/OSWorld` at HEAD on 2026-09-09): `os` (1), `gimp` (2), `vlc` (2), `vs_code` (3), `libreoffice_calc` (3), `libreoffice_writer` (2), `libreoffice_impress` (2). Excluded: `chrome`, `thunderbird`, `multi_apps` (need accounts, proxies, or internet) and `os/5812b315` (its evaluator `apt-get install`s inside the guest, and the local VM has no outbound network).
- **Environment:** the OSWorld golden image patched with cua-driver 0.25.0 (see README), one fresh qcow2 overlay per episode, bare-metal QEMU/KVM under WSL2, 4 vCPU / 8 GB. Task setup runs through OSWorld's own `SetupController`, scoring through OSWorld's own `metrics`/`getters` (a 40-line port of `DesktopEnv.evaluate`).
- **Policy:** Claude Haiku 4.5 (`claude-haiku-4-5-20251001`) via the Claude Agent SDK (bundled Claude Code CLI, OAuth token), `permission_mode=bypassPermissions`, no built-in tools, same system prompt skeleton, **25-action cap** and 15-minute episode timeout in every condition. Each task run once per condition (n=1; this is a smoke-scale eval, not a benchmark number).
- **Conditions:**
  - `noop`: task setup, no agent, evaluate. Control for evaluator leniency.
  - `pyautogui`: the classic OSWorld loop. Tools: `screenshot` (1280x720 JPEG of the 1920x1080 screen), `pyautogui(code)` (runs in the guest via the OSWorld server's `/execute`, coordinates scaled 1.5x), `done`, `fail`.
  - `cuadriver`: the stock cua-driver MCP catalog (60 tools) over Streamable HTTP, plus `done`/`fail`.
  - `cuadriver_curated`: same, but 42 of the 60 tools hidden (`disallowed_tools`), leaving 18 desktop tools: `get_desktop_state`, `get_screen_size`, `list_windows`, `list_apps`, `get_window_state`, `launch_app`, `bring_to_front`, `click`, `double_click`, `right_click`, `type_text`, `press_key`, `hotkey`, `scroll`, `drag`, `zoom`, `invoke_menu`, `set_value`.

Harness: `eval/harness.py` in the session scratchpad (copied to `~/osworld-eval` in WSL); results in `~/osworld-eval/results/*.jsonl`.

## Results

| Condition | Tasks solved | Mean score | Avg actions | Haiku spend |
| --- | --- | --- | --- | --- |
| `noop` (control) | 0 / 15 (0 %) | 0.00 | 0 | $0 |
| `pyautogui` (classic OSWorld loop) | **12 / 15 (80 %)** | 0.87 | 9.9 | $1.26 |
| `cuadriver` (stock 60-tool catalog) | **2 / 15 (13 %)** | 0.13 | 22.8 | $3.71 |
| `cuadriver_curated` (18 desktop tools) | **0 / 15 (0 %)** | 0.07 | 21.3 | $3.28 |

Per task (score, actions, how the episode ended; `!` = episode timeout or evaluator timeout because the guest froze):

```text
domain               task     |      pyautogui |          cuadriver |  cuadriver_curated
gimp                 554785e9 |  1.00  6 DONE  |  1.00 28 (cap)     |  0.00 28 (cap)
gimp                 7a4deb26 |  1.00  8 DONE  |  0.00 28 (cap)     |  0.00  8 timeout !
libreoffice_calc     357ef137 |  1.00  5 DONE  |  0.00 13 timeout ! |  0.00 14 eval-timeout !
libreoffice_calc     42e0a640 |  1.00 10 DONE  |  0.00 18 FAIL      |  0.00 16 timeout !
libreoffice_calc     abed40dc |  1.00 11 DONE  |  0.00 14 FAIL !    |  0.00 12 FAIL
libreoffice_impress  550ce7e7 |  1.00  9 DONE  |  0.00 28 (cap)     |  0.00 28 (cap)
libreoffice_impress  5d901039 |  0.00 25 (cap) |  0.00 28 (cap)     |  0.00 28 (cap)
libreoffice_writer   0810415c |  1.00  7 DONE  |  0.00 28 (cap)     |  0.00 28 (cap)
libreoffice_writer   0a0faba3 |  0.00 25 DONE  |  0.00 15 FAIL      |  0.00 28 (cap)
os                   5ea617a3 |  1.00  4 DONE  |  0.00  8 timeout ! |  0.00 15 timeout !
vlc                  59f21cfb |  1.00  8 DONE  |  0.00 28 (cap)     |  0.00 18 DONE
vlc                  8f080098 |  0.99 14 DONE  |  0.00 28 (cap)     |  0.99 19 (cap)
vs_code              0ed39f63 |  1.00  6 DONE  |  0.00 28 (cap)     |  0.00 28 (cap)
vs_code              276cc624 |  1.00  7 DONE  |  1.00 22 DONE      |  0.00 23 DONE
vs_code              53ad5833 |  1.00  3 DONE  |  0.00 28 (cap)     |  0.00 27 DONE
```

Raw records: `eval/*.jsonl`; harness and diagnostics: `eval/harness.py`, `eval/diag_click.py`, `eval/diag_typetext.py`.

**So the claim reproduces.** Same model, same tasks, same image, same step budget: 80 % through the classic screenshot + pyautogui loop, 13 % through the stock cua-driver MCP, 0 % through a trimmed catalog. The driver conditions also cost about 3x more per task (long tool schemas, 2.3x more actions per episode, and turns spent recovering from stalls).

## Why the driver conditions lose

Three mechanisms, each verified outside the eval with a scripted test on a fresh VM (`diag_click.py`, `diag_typetext.py`; screenshots in `1_cua_click_bg.png` ... `4_pyautogui_click.png` and `diag-typetext/`):

1. **Default clicks do not operate GTK menus.** In GIMP, `click {pid, x, y}` (background XSendEvent delivery) and `click {..., delivery_mode: "foreground"}` both report "✅ Clicked" and open nothing; only `click {target: {kind: "desktop"}}` or OSWorld's `pyautogui.click` open the Colors menu (pixel-diff 0.30 % / 0.04 % vs 0.54 % / 0.51 %). Haiku's default is the pid form, so in the GIMP episode it clicked the menubar five times, then abandoned menus.
2. **No working text-input route into LibreOffice, and the foreground route freezes the X session.** `type_text {pid}` is refused after 13-26 s ("Background delivery is not available ... no focus-free input backend"), `type_text {target: desktop}` claims success but types nothing, and `type_text {..., delivery_mode: "foreground"}` hangs and freezes the guest display (top-bar clock stops; OSWorld server and MCP endpoint stop answering). Both eval VMs froze this way on the first Calc task (`diag-hang/*.png`), which is why every LibreOffice episode under the driver scores 0.
3. **Stalls and per-call latency.** An AT-SPI element click on Files took 5.5 min to return; after a stalled action, every following call on that window took ~60 s until `health_report` was called. With a 25-action budget and a 15-minute episode cap, a single stall removes a third of the episode.

Secondary, not decisive: the stock catalog is 60 tools (~200 KB of schema) and Haiku spent calls on `page`, `health_report`, `check_permissions`, `list_sessions`, `end_session`/`start_session` while trying to recover; the curated 18-tool variant did not score better, so the catalog size is not what loses the tasks. The model's first click of every episode also targeted `pid: 1` because `get_desktop_state` does not say which pid owns what.

## Caveats

- n=1 per task per condition, 15 tasks, one model. Enough to see a large effect, not to estimate a rate precisely.
- `test_small` skews easy for the pyautogui loop (12/15 is well above OSWorld leaderboard numbers for Haiku-class models); the comparison is within this harness, not against published scores.
- The cua-driver prompt was written once and not tuned; a prompt that says "always use `target: {kind: desktop}` for clicks and `foreground` delivery" would likely recover part of the gap. That would also mean giving up the no-focus contract the driver is designed around.
