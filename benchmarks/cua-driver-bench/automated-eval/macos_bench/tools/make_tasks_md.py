#!/usr/bin/env python3
"""Generate TASKS.md from probes/MB-*/ (task.json, brief.md, evaluate.py docstrings) plus the prose below.

Prompts are copied verbatim from brief.md so the document cannot drift from what the runner sends.
Validation evidence is read from validation/results.json when present.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROBES = ROOT / "probes"
OUT = ROOT / "TASKS.md"
VALIDATION = ROOT / "validation" / "results.json"

AF = "reconstruction of the public description of the argofowl test"
P = "probes of the disputed abilities"

# Per-task prose. Keys: claim, setup, checker_req, checker_diag, disturbance, calls, caveat, notes
T = {
    "MB-01": dict(
        claim="Plain left clicks on a custom-drawn view. argofowl: on 0.33.0 '0 of 24 plain left clicks on custom-drawn views arrived'.",
        setup="BenchLab `--mode canvasclick`. One custom 700x420 view with 24 numbered circles (radius 14) on a jittered 6x4 grid, drawn as pixels; the view is a single accessibility group with no per-circle children, and the circle numbers are in no label, title or value. Circle labels and the click order come from the seed.",
        req="circles_consistent, state_matches_events, left_sequence_in_order (the 12 numbers are clicked in this order, other clicks in between are allowed), no_extra_left_clicks (the circle-hitting left clicks are exactly the 12, in order; clicks that miss every circle are allowed), done_pressed, done_after_clicks, integrity.",
        diag="event_pairing_anomalies (mouseDown/mouseUp pairing problems), left_misses, right_clicks.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass.",
        calls="14 to 20 (1 look, 12 clicks, Done, optional check).",
        caveat="None. Both tool sets have a click tool.",
    ),
    "MB-02": dict(
        claim="Right-clicks on a custom-drawn view. argofowl: 'right-click and scroll land in the background'. Here the right-click must also open a context menu and the agent must pick the stated item.",
        setup="BenchLab `--mode canvasmenu`. The same 24-circle board as MB-01. Right-clicking a circle opens a native context menu with five actions (Pin, Mute, Archive, Highlight, Duplicate); choosing one draws its name under the circle. The four target circles and their actions come from the seed. The app opens the menu once per right press and ignores (and logs) a second rightMouseDown that arrives while a menu is pending, so the known right-click doubling (Down, Down, Up, Up; trycua/cua#4679) cannot decide the outcome.",
        req="circles_consistent, target_1..4_action (the last action chosen on that circle equals the stated one), no_wrong_actions (no action on a non-target circle), menu_via_right_press (each chosen action follows a menu opened by a logged right mouse press on the same circle), state_matches_events, done_pressed, done_after_actions, integrity.",
        diag="right_event_pairing_anomalies (the doubled-event diagnostic), right_down_duplicates_ignored, menu_opens, menu_dismissed, left_clicks, action_events.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass.",
        calls="12 to 20 (per target: right-click, look at the menu, choose).",
        caveat="None for the tools. The context menu is a separate on-screen window that a window-only capture may not show; that is part of what is measured.",
    ),
    "MB-03": dict(
        claim="Scrolling a virtualised list to find a row. argofowl: scroll lands in the background.",
        setup="BenchLab `--mode tablesel`. A 400-row `NSTableView` (Code K-0001..K-0400, Name, Qty) in a scroll view, about 17 rows visible. Only the rows in the viewport are exposed to accessibility, so an agent that reads the accessibility tree must scroll to see more rows (the same table the pilot used for PROBE-TABLE). The target row (Name and Qty from the seed) lies between rows 200 and 380; two other rows share its Name, one other row shares its Qty, so only the pair identifies it. The app never writes the target code.",
        req="confirm_pressed, selected_target_at_confirm (the row selected at the last Confirm is the target), no_wrong_confirm (every Confirm press had the target selected), selection_events_consistent, state_matches, integrity.",
        diag="scrolled_near_target (the viewport reached within 25 rows of the target), scroll_events, select_events, confirm_count.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass.",
        calls="8 to 25 depending on scroll step size. This is the longest AF task by tool calls.",
        caveat="None. Both tool sets scroll and select.",
    ),
    "MB-04": dict(
        claim="Drags on a custom-drawn view. argofowl: 'drags are refused' for Cua in the background; 'codex's engine sends real clicks and drags'. Cua Driver documents that macOS has no background drag: it runs in foreground delivery (moving the real pointer) or is refused.",
        setup="BenchLab `--mode canvas`. One custom 700x420 view with three 56 px tiles and three 96 px outlined zones in matching colours, drawn as pixels with no per-tile accessibility children. Tile and zone positions come from the seed. Same app mode as the pilot's PROBE-CANVAS.",
        req="zones_consistent, red_in_zone, green_in_zone, blue_in_zone (the tile sits inside the outline, 8 px tolerance), done_pressed, placed_when_done, drag_sequences (3 or more logged mouseDown, mouseDragged, mouseUp sequences), positions_via_events (the final tile positions are reproduced by replaying the logged drags), tiles_moved, integrity.",
        diag="none beyond the check details.",
        dist="Foreground delivery may legitimately move the pointer or bring the app forward; this is measured, not penalised.",
        calls="5 to 12 (1 look, 3 drags, Done).",
        caveat="None for the tools. The result splits into landed-in-background versus needed-foreground through the sentinel data.",
    ),
    "MB-05": dict(
        claim="Ordinary native controls: text fields, pop-up, radio buttons, checkbox, stepper field and text view.",
        setup="BenchLab `--mode forms`. An invoice form with Customer name, Invoice amount, Category (8-item pop-up), Priority (3 radios), Notify me (checkbox), Quantity (field and stepper), Notes (text view), Submit and a status line. All controls are reachable through accessibility with stable labels. Values come from the seed. Same app mode as PROBE-FORMS.",
        req="customer_name, invoice_amount, category, priority, notify, quantity, notes, submitted_once, ui_events (the submitted values are explained by logged field edits), integrity.",
        diag="none.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass.",
        calls="8 to 20.",
        caveat="None.",
    ),
    "MB-06": dict(
        claim="Text editing: typing, selecting a stated span, applying a format, and replacing a word.",
        setup="BenchLab `--mode richtext`. An empty rich text editor (`NSTextView`, 16 pt, automatic substitutions off) with a Bold button, a Format menu with Bold (Cmd-B) and a Done button. The editor is an ordinary accessibility text area. The paragraph is three sentences drawn from a bank of 12 by the seed; which sentence is bolded and which gets the replacement are also from the seed (never the same sentence).",
        req="text_correct (the final text equals the paragraph with the one word replaced), replacement_applied, bold_covers_sentence (every non-space character of the stated sentence is bold), no_bold_elsewhere, done_pressed (the Done snapshot equals the live document), doc_via_events, integrity. The app writes the text and the bold character ranges of the document model; the checker recomputes the expected text and range from the seed.",
        diag="text_change_via (typing versus whole-value edits), text_change_events, format_change_events, selection_events, max_selection_length.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass.",
        calls="8 to 18.",
        caveat="None. Typing and selecting by keyboard are possible with both tool sets; selecting with a mouse drag may need foreground delivery in one arm, which is measured.",
    ),
    "MB-07": dict(
        claim="Multi-app work: Calculator to BenchLab, through the clipboard.",
        setup="BenchLab `--mode clipboard` (a Result field, Save, status). Calculator is not running at the start (killed and its saved state removed at reset) and the pasteboard is cleared. The expression is A x B + C with values from the seed. Same app mode as PROBE-CLIPBOARD.",
        req="result_saved, result_correct (the saved integer equals A x B + C; thousands separators are accepted), ui_events (the saved value equals the last logged Result edit), integrity.",
        diag="pasteboard_changes, whole_value_in_one_edit, save_count. The runner also records whether Calculator was running at the end. The route (Calculator, copy, paste) is requested in the prompt, but only the result is scored.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass. The agent has to open Calculator, which takes the front in some tool sets.",
        calls="8 to 18.",
        caveat="A model can do the arithmetic itself and skip Calculator; both arms are equally able to. The Calculator-ran flag makes that visible.",
    ),
    "MB-08": dict(
        claim="Multi-window flow driven from the menu bar: open a second window, change several controls, apply, then check the effect in the first window.",
        setup="BenchLab `--mode settings`. The main window (greeting panel, ticket line, Confirm ticket field, Confirm, status). Settings... in the BenchLab menu (Cmd-,) opens a separate window titled Settings at a fixed position (860, 120) with Display name (text field), Theme (pop-up of Light, Sepia, Slate, Forest), Compact layout (checkbox; its starting state comes from the seed and the target is the opposite) and Apply. Apply restyles the main window and shows a ticket number that is a hash of the seed and the applied settings; before Apply it reads 'none yet'. The target name and theme come from the seed. Nothing is persisted.",
        req="settings_opened, applied_name, applied_theme, applied_compact (from the last Apply), ticket_correct (the number typed into the main window equals the number shown after Apply and equals the ticket the checker recomputes for the stated settings), confirm_after_apply, confirm_pressed, integrity.",
        diag="opened_via (menu or Cmd-, key), apply_presses, earlier_confirm_entries.",
        dist="Sentinel records focus, key loss, leaked input and pointer movement. Not part of pass. Menu bar access normally needs the app to be frontmost or an accessibility menu path.",
        calls="10 to 20.",
        caveat="None. The menu bar item can be reached by a menu path (accessibility), by clicking, or by Cmd-,.",
    ),
    "MB-09": dict(
        claim="Drag and drop in a native list whose rows are visible to accessibility, so the accessibility tree helps to find the rows but offers no way to reorder.",
        setup="BenchLab `--mode listdrag`. A single-column `NSTableView` of five items (from a bank of 10, order from the seed) with drag-to-reorder and a Done button. The stated order differs from the initial order in at least 4 of 5 positions, so it needs about three drags. There is no keyboard or button route to reorder.",
        req="order_correct, done_pressed, done_order_correct, moves_via_drop (the logged drop events, replayed from the seed's initial order, reproduce every logged order and the final state; a drop is only raised by a real drag session), initial_consistent, integrity.",
        diag="drag_sessions_begun, drops.",
        dist="Foreground delivery may legitimately move the pointer; this is measured, not penalised.",
        calls="6 to 15.",
        caveat="None for the tools. Same drag question as MB-04, on rows an accessibility client can read.",
    ),
    "MB-10": dict(
        claim="A hover-only control. argofowl: Codex has 'no hover'.",
        setup="BenchLab `--mode hover`. A toolbar strip with an 'Actions' hot zone. While the pointer is in it, four action buttons (from a shuffle of six, order from the seed) appear below; they are hidden, and absent from the accessibility tree, otherwise. They hide 400 ms after the pointer leaves. Same app mode as PROBE-HOVER.",
        req="overlay_names_consistent, clicked_target, overlay_visible_at_click, hover_genuine (the click is preceded by logged hover events and an overlay_shown event, and the enter/exit counts match the events), no_wrong_clicks, single_click, integrity.",
        diag="none.",
        dist="Hover needs the real pointer or a posted pointer-move; measured, not penalised.",
        calls="4 to 10.",
        caveat="COVERAGE ITEM. The Codex computer-use API has no hover or pointer-move tool, so a failure there is a missing capability, not a task failure of the same kind. Report this task separately and exclude it from the headline macro mean.",
    ),
    "MB-11": dict(
        claim="A native tooltip, which appears only after the pointer rests on a control.",
        setup="BenchLab `--mode tooltip`. A custom-drawn strip of four icons labelled Truck, Cube, Flag and Clock (order from the seed), a Code field, Submit and a status line. Each icon has a native tooltip (`NSViewToolTipOwner`, rect based, so there is no help attribute or per-icon element in the accessibility tree) reading 'Reference code XX-0000' with a seed-derived code. The tooltip text is in no label, title or value.",
        req="code_correct (the submitted code is the one in the tooltip of the stated icon), submitted_once, tooltip_displayed_for_target (the app logged AppKit asking for that icon's tooltip before Submit, so the pointer really dwelt on the icon), value_via_events, integrity.",
        diag="icons_queried, wrong_icon_queries.",
        dist="Hover needs the real pointer or a posted pointer-move; measured, not penalised.",
        calls="5 to 10.",
        caveat="COVERAGE ITEM, same as MB-10. A tooltip is a separate system window; a capture of one window may not include it, so this also tests what each tool set can see. Report separately and exclude from the headline mean.",
    ),
    "MB-12": dict(
        claim="Background operation: working in an app while another app stays in front and the user's pointer stays where it is. argofowl: Codex's engine works 'without touching my cursor'.",
        setup="BenchLab `--mode forms` (the MB-05 form). BenchSentinel (a small window titled Bench Sentinel with a focused text field) is made frontmost and key before the agent starts, with BenchLab behind it, as for every task. The prompt asks the agent to leave that window in front and in focus, not to type or click into it, and not to move the mouse pointer, and to set only three fields.",
        req="Task: customer_name, category, priority, submitted_once, ui_events, integrity. Disturbance (this is the only task where it is part of pass): sentinel_available (a usable sentinel summary exists), front_unchanged (no change of frontmost app and the sentinel kept key focus), pointer_still (largest real pointer deviation <= 5 px), no_leaked_input (no key, click or scroll reached the sentinel).",
        diag="hid_events, pointer_deviation_episodes.",
        dist="Counted in pass for this task only. Every other task records the same sentinel metrics as a separate disturbance measure.",
        calls="6 to 14.",
        caveat="None for the tools. The prompt names the constraint (stay in front, do not move the pointer) in plain words; it does not say how.",
    ),
}


def load(task_dir: Path):
    spec = importlib.util.spec_from_file_location(
        "r_" + task_dir.name, task_dir / "render_brief.py"
    )
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def seed_for(task: str, rnd: int) -> int:
    return int(hashlib.sha256(f"{task}:{rnd}".encode()).hexdigest()[:8], 16) % 1_000_000


def main() -> int:
    tasks = sorted(PROBES.glob("MB-*/task.json"))
    meta = {json.loads(p.read_text())["id"]: json.loads(p.read_text()) for p in tasks}
    val = json.loads(VALIDATION.read_text()) if VALIDATION.exists() else None
    out: list[str] = []
    w = out.append
    w("# macOS bench task spec (MB-01 to MB-12)\n")
    w(
        "Status: DRAFT for review. It becomes frozen when the pre-registration commit lands; after that only bug fixes are allowed, each logged as an amendment.\n"
    )
    w(
        "Generated by `tools/make_tasks_md.py` from `probes/MB-*/` (prompts are copied verbatim from `brief.md`, limits from `task.json`). "
        "Edit the sources, not this file.\n"
    )
    w("## What this is\n")
    w(
        "Twelve tasks for a head-to-head of two computer-use tool sets with the same model, run sequentially on one Mac. "
        "Tasks MB-01 to MB-08 (group AF) are a **reconstruction** of the kind of tasks @argofowl described on 3 Oct 2026 "
        "('same 8-task test'; canvases, drags, clicks, right-clicks, scroll, no hover). His tasks were never published. "
        "These are **not his tasks**; they are ours, built from the public description, and every report must say so. "
        "Tasks MB-09 to MB-12 (group P) probe the abilities he said differ: drag and drop in a readable list, hover-only controls, "
        "tooltips, and background operation.\n"
    )
    w(
        "All tasks run in one test app, **BenchLab** (a small AppKit app), except that MB-07 also uses macOS Calculator and MB-12 also uses the BenchSentinel witness window.\n"
    )

    w("## Task list\n")
    w("| ID | Name | Group | Ability | Wall (s) | Max turns | Coverage caveat |")
    w("|---|---|---|---|---|---|---|")
    for tid in sorted(meta):
        m = meta[tid]
        cav = "hover: no Codex tool, report separately" if m.get("coverage_caveat") else "none"
        w(
            f"| {tid} | {m['title']} | {m['group']} | {m['ability']} | {m['timeout_s']} | {m['max_turns']} | {cav} |"
        )
    w("")
    w("Group AF = " + AF + ". Group P = " + P + ".\n")

    w("## Conventions that apply to every task\n")
    w(
        "**Prompts.** Each prompt is tool-neutral and identical for both arms. It names the app, states a concrete goal and says what done means. It never mentions any tool, accessibility, pixels, screenshots or MCP. The runner adds only its shared preamble and the time budget.\n"
    )
    w(
        '**Seeds.** `seed = sha256(f"{task}:{round_index}") mod 1e6` (round index from 0), the same for both arms in a round and different across rounds and tasks. Everything random in a task (circle numbers, target rows, sentences, values) is derived from the seed with splitmix64, in Swift (the app) and in Python (the checker), and a test compares both on 60 seeds. The app never writes an expected answer into its state or log.\n'
    )
    w(
        "**Setup and reset.** Every trial starts a fresh BenchLab process with a throwaway `HOME`, new empty state and event files, window restoration disabled, and the window at a fixed frame (outer 760x560, top-left at (60, 80) on the primary display; the MB-08 Settings window opens at (860, 120)). Reset is: kill BenchLab (and Calculator for MB-07, with its saved state removed and the pasteboard cleared), delete the state and event files, relaunch. Measured reset plus check time is in the validation table; the budget is 10 s. BenchSentinel is started and made frontmost and key before the agent starts, with BenchLab behind it, for every task.\n"
    )
    w(
        "**Checker interface.** `probes/MB-xx/render_brief.py: render(seed) -> prompt`; app launch `BenchLab <app_args from task.json> --seed S --state PATH --events PATH`; `probes/MB-xx/evaluate.py --seed S --state PATH --events PATH --result OUT.json [--sentinel PATH]`. The result JSON has `passed` (every required check passed), `score` (weighted share of required checks passed), `checks` (required, per-check `passed`, `weight`, `detail`) and `diagnostics` (recorded, never part of pass). No model grades anything. The checkers read the event log and state the app writes from real UI events (mouse down and up with coordinates, key and edit events, drops, tooltip requests, menu actions) and recompute every expected value from the seed.\n"
    )
    w(
        "**Pass is the outcome, not the route.** The route is free (any tool, any order) unless the prompt says otherwise. The one known tool defect on this build, right-clicks reaching the app as Down, Down, Up, Up (trycua/cua#4679), is recorded as a diagnostic in MB-02 and does not decide pass.\n"
    )
    w(
        "**Disturbance.** BenchSentinel records focus changes, key loss, leaked keystrokes, clicks and scrolls, HID-level events and the real pointer position for every trial. Outside MB-12 it is a separate metric and never part of pass, because drags and hover may legitimately need foreground delivery. MB-12 is the one task where it is part of pass.\n"
    )
    w(
        "**Limits.** Wall 360 s and 45 turns for every task (no overrides), the same for both arms. Turns include the tool-schema loads (ToolSearch) and skill reads that Claude Code makes before the first action, so the cap is higher than the 14 to 25 tool calls a task needs.\n"
    )
    w(
        "**Build.** `swift/build.sh <dir>` writes BenchLab.app and BenchSentinel.app (ad-hoc signed, no keychain identity) in about 10 s. BenchLab is built from `swift/BenchLab.swift` and `swift/BenchLabModes.swift`.\n"
    )

    for tid in sorted(meta):
        m = meta[tid]
        t = T[tid]
        brief = (PROBES / tid / "brief.md").read_text()
        mod = load(PROBES / tid)
        example_seed = seed_for(tid, 0)
        example = mod.render(example_seed)
        w(f"## {tid} {m['title']}\n")
        w(
            f"- **Group:** {m['group']}. **Ability:** {m['ability']}. **Limits:** {m['timeout_s']} s wall, {m['max_turns']} turns, expected tool calls {t['calls']}"
        )
        w(f"- **What it tests:** {t['claim']}")
        w(f"- **Coverage caveat:** {t['caveat']}")
        w(
            f"- **App and apps used:** BenchLab `{' '.join(m['app_args'])}`"
            + (f"; also {', '.join(m['needs_apps'])}" if m.get("needs_apps") else "")
            + (
                ". Also BenchSentinel (disturbance is part of pass)."
                if m.get("sentinel_in_pass")
                else "."
            )
        )
        w("")
        w("**Prompt template** (the `{{...}}` values come from the seed):\n")
        w("```text")
        w(brief.rstrip("\n"))
        w("```\n")
        w(f"**Example, round 0 (seed {example_seed}):**\n")
        w("```text")
        w(example.rstrip("\n"))
        w("```\n")
        w(f"**Setup and fixture.** {t['setup']}\n")
        w(
            f"**Checker.** `probes/{tid}/evaluate.py`. Required checks: {t['req']} Diagnostics (never part of pass): {t['diag']}\n"
        )
        w(f"**Disturbance relevance.** {t['dist']}\n")
        v = (val or {}).get("tasks", {}).get(tid)
        if v:
            w(
                f"**Validation (real input, run on 5 Oct 2026).** no-op: {v.get('noop')}; oracle: {v.get('oracle')} (oracle took {v.get('oracle_s')} s); reset twice: {v.get('reset')}; wrong action: {v.get('wrong')}; timing: {v.get('timing')}.\n"
            )
        else:
            w(
                "**Validation.** NOT run against the live app (work stopped before it). Only synthetic-event checks of the evaluator exist (`probes/tests/test_mb_synthetic.py`). See the validation summary.\n"
            )
    w("## Validation summary\n")
    w(
        "Run by `tools/validate.py` with real HID events from `tools/benchinput` (CGEvent, foreground), on the shared Mac, against the final app build. Per task: (1) no-op run fails; (2) an oracle that does the task through real input passes; (3) setup, reset, setup, reset in a row gives the same initial state each time and leaves no process or file behind; (4) a deliberately wrong action fails with the expected failed check(s).\n"
    )
    w(
        "| Task | No-op fails | Oracle passes | Reset clean x2 | Wrong action fails (failed checks) | Setup / check / reset (s) | Oracle time (s) |"
    )
    w("|---|---|---|---|---|---|---|")
    for tid in sorted(meta):
        v = (val or {}).get("tasks", {}).get(tid)
        if v:
            wf = v.get("wrong", "").replace("FAIL as required, failed ", "")
            w(
                f"| {tid} | {'yes' if v.get('noop_ok') else 'NO'} | {'yes' if v.get('oracle_ok') else 'NO'} | {'yes' if v.get('reset_clean') else 'NO'} | {'yes' if v.get('wrong_ok') else 'NO'}: {wf} | {v.get('setup_s')} / {v.get('check_s')} / {v.get('reset_s')} | {v.get('oracle_s')} |"
            )
        else:
            w(f"| {tid} | not run | not run | not run | not run | not run | not run |")
    w("")
    EXTRA = ROOT / "tools" / "tasks_tail.md"
    if EXTRA.exists():
        w(EXTRA.read_text())
    OUT.write_text("\n".join(out) + "\n")
    print(f"wrote {OUT} ({len(out)} blocks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
