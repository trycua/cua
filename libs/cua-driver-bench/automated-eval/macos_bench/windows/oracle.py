# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Scripted oracle for the Windows BenchLab modes: solves a task through the GUI with real input.

Used only to validate the port (each task solvable, its checker passes); never a bench setup. It finds
controls through UI Automation (pywinauto) and acts with real pointer and keyboard input in the foreground.

Usage: oracle.py --mode MODE --seed N [--disturb] [--instance L [--code TEXT] [--shelf NAME] [--task ID]]
       oracle.py --mode crowded --seed N [--instances N] [--skip L,..] [--premature L,..] [--pending L,..]
                 [--type L=TEXT ...]
`--disturb` instead performs one deliberate disturbance of the user window (validates the sentinel).
`--mode desk` does the PA-01 / UM-01 sub-task in the window "BenchLab Desk L" (`--instance`): types the cell's
code, selects its shelf and presses Submit. `--code` and `--shelf` override what to type and select (to
rehearse a wrong or crossed-over agent). With `--task UM-02` the desk is the busy user's and the oracle acts
only through UI Automation patterns (set value, select, invoke): no pointer or key input, so it works under the
user window and never moves the real pointer.
`--mode crowded` is the PA-02 oracle: it works every copy of the crowded desk in one run through UI Automation
patterns, whatever covers what (type the code, pick the shelf, Submit, then Confirm). The copies share one window
title, so each is told apart by the heading "Desk L" inside it. Rehearsals: `--skip` leaves copies alone,
`--premature` presses Submit and Confirm on copies that were not filled in, `--pending` stops those copies at
the Confirm step, `--type L=TEXT` types TEXT instead of copy L's code (cross-talk).
"""

from __future__ import annotations

import argparse
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
import benchlab_common as C  # noqa: E402

from pywinauto import Desktop, keyboard, mouse  # noqa: E402


def lab(mode: str, title: str | None = None):
    win = Desktop(backend="uia").window(title=title or f"BenchLab — {mode}")
    win.wait("exists visible", timeout=20)
    win.set_focus()
    time.sleep(0.4)
    return win


def center(ctrl) -> tuple[int, int]:
    r = ctrl.rectangle()
    return (r.left + r.right) // 2, (r.top + r.bottom) // 2


def click(ctrl) -> None:
    ctrl.click_input()
    time.sleep(0.3)


def type_into(win, name: str, value: str) -> None:
    edit = win.child_window(title=name, control_type="Edit")
    edit.click_input()
    keyboard.send_keys("^a{BACKSPACE}", pause=0.02)
    keyboard.send_keys(str(value), with_spaces=True, pause=0.02)
    time.sleep(0.2)


def choose(win, name: str, value: str) -> None:
    combo = win.child_window(title=name, control_type="ComboBox")
    combo.click_input()
    time.sleep(0.4)
    combo.child_window(title=value, control_type="ListItem").click_input()
    time.sleep(0.3)


def button(win, title: str):
    return win.child_window(title=title, control_type="Button")


def dialog_button(title: str) -> None:
    for _ in range(40):
        for w in Desktop(backend="uia").windows():
            try:
                found = w.descendants(title=title, control_type="Button")
            except Exception:  # noqa: BLE001
                continue
            if found:
                found[0].click_input()
                time.sleep(0.3)
                return
        time.sleep(0.25)
    raise RuntimeError(f"no dialog button {title!r}")


def drag(src: tuple[int, int], dst: tuple[int, int]) -> None:
    mouse.press(coords=src)
    for i in range(1, 11):
        mouse.move(
            coords=(src[0] + (dst[0] - src[0]) * i // 10, src[1] + (dst[1] - src[1]) * i // 10)
        )
        time.sleep(0.04)
    mouse.release(coords=dst)
    time.sleep(0.5)


def listdrag(seed: int) -> None:
    p = C.derive_listdrag(seed)
    win = lab("listdrag")
    order = list(p["initial"])
    for index, item in enumerate(p["target"]):
        source = order.index(item)
        if source == index:
            continue
        rows = win.child_window(control_type="List").children(control_type="ListItem")
        drag(center(rows[source]), center(rows[index]))
        order.pop(source)
        order.insert(index, item)
    click(button(win, "Done"))


def hover(seed: int) -> None:
    p = C.derive_hover(seed)
    win = lab("hover")
    zone = win.child_window(title="Hover here to reveal actions", control_type="Text")
    x, y = center(zone)
    mouse.move(coords=(x, y))
    time.sleep(0.6)
    mouse.move(coords=(x + 2, y + 30))
    time.sleep(0.6)
    click(button(win, p["target"]))


def tooltip(seed: int) -> None:
    p = C.derive_tooltip(seed)
    win = lab("tooltip")
    icon = win.child_window(title=p["target"], control_type="Text")
    x, y = center(icon)
    mouse.move(coords=(x - 3, y))
    time.sleep(0.2)
    mouse.move(coords=(x, y))
    time.sleep(2.0)
    type_into(win, "Code", p["target_code"])
    click(button(win, "Submit"))


def irmodal(seed: int) -> None:
    p = C.derive_irmodal(seed)
    win = lab("irmodal")
    type_into(win, "Name", p["name"])
    type_into(win, "Email", p["email"])
    click(button(win, "Next"))
    dialog_button("Don’t Allow")
    choose(win, "Team", p["team"])
    type_into(win, "Seats", str(p["seats"]))
    click(button(win, "Submit"))


def irbanner(seed: int) -> None:
    p = C.derive_irbanner(seed)
    win = lab("irbanner")
    type_into(win, "Quantity", str(p["quantity"]))
    time.sleep(0.6)
    click(button(win, "Dismiss"))
    choose(win, "Priority", p["priority"])
    click(button(win, "Apply"))


def irunsaved(seed: int) -> None:
    p = C.derive_irunsaved(seed)
    win = lab("irunsaved")
    click(button(win, p["titles"][p["status_index"]]))
    choose(win, "Status", p["status"])
    click(button(win, p["titles"][p["rename_index"]]))
    dialog_button("Save")
    type_into(win, "Title", p["new_title"])
    click(button(win, "Done"))
    dialog_button("Save")


def irconsent(seed: int) -> None:
    p = C.derive_irconsent(seed)
    win = lab("irconsent")
    type_into(win, "Email", p["email"])
    dialog_button("Reject optional")
    type_into(win, "Email", p["email"])  # the consent prompt takes the keys typed after it opened
    choose(win, "Plan", p["plan"])
    box = win.child_window(title="Email digest", control_type="CheckBox")
    if bool(box.get_toggle_state()) != p["digest"]:
        click(box)
    click(button(win, "Sign up"))


def desk_cell(seed: int, instance: str, task: str | None = None) -> dict:
    """The seed's code and shelf for the desk labelled `instance` (a PA-01 copy, or the UM-01 / UM-02 desk)."""
    if task == "UM-02":
        cell = C.derive_busy_desk(seed)
        if cell["label"] == instance:
            return cell
        raise SystemExit(f"no desk {instance!r} for seed {seed}")
    for cell in C.derive_parallel(seed, C.PA_MAX_INSTANCES)["cells"]:
        if cell["label"] == instance:
            return cell
    um = C.derive_um_desk(seed)
    if um["label"] == instance:
        return um
    raise SystemExit(f"no desk {instance!r} for seed {seed}")


def desk(
    seed: int,
    instance: str,
    code: str | None = None,
    shelf: str | None = None,
    task: str | None = None,
) -> None:
    cell = desk_cell(seed, instance, task)
    if task == "UM-02":  # under the user's window, with the user's pointer in use: patterns only
        win = Desktop(backend="uia").window(title=C.desk_title(instance))
        win.wait("exists", timeout=20)
        fill_copy(win, cell, code, shelf)
        uia_invoke(win, "Submit")
        time.sleep(0.3)
        return
    win = lab("desk", C.desk_title(instance))
    type_into(win, "Code", cell["code"] if code is None else code)
    click(
        win.child_window(
            title=cell["shelf"] if shelf is None else shelf, control_type="RadioButton"
        )
    )
    click(button(win, "Submit"))


# ---------------------------------------------------------------- UI Automation patterns (no pointer, no keys)


def uia_type(win, name: str, value: str) -> None:
    """Set an edit's value through the value pattern, read it back, retry once."""
    edit = win.child_window(title=name, control_type="Edit").wrapper_object()
    for _ in range(2):
        edit.set_edit_text(value)
        time.sleep(0.15)
        if edit.get_value() == value:
            return
    raise RuntimeError(f"could not set {name!r} to {value!r}")


def uia_invoke(win, title: str, control_type: str = "Button") -> None:
    ctrl = win.child_window(title=title, control_type=control_type).wrapper_object()
    if control_type == "RadioButton":
        for how in ("select", "invoke"):
            try:
                getattr(ctrl, how)()
                return
            except Exception:  # noqa: BLE001 - try the next pattern
                continue
        raise RuntimeError(f"could not select {title!r}")
    ctrl.invoke()


def fill_copy(win, cell: dict, code: str | None = None, shelf: str | None = None) -> None:
    uia_type(win, "Code", cell["code"] if code is None else code)
    uia_invoke(win, cell["shelf"] if shelf is None else shelf, "RadioButton")
    time.sleep(0.4)  # BenchLab polls the radio group every 250 ms; Submit reads it again


def crowded_copies(instances: int) -> dict:
    """The PA-02 windows by label. They share one title, so each is read by its "Desk L" heading."""
    found: dict = {}
    end = time.monotonic() + 30
    while time.monotonic() < end:
        found = {}
        for w in Desktop(backend="uia").windows(title=C.PA2_WINDOW_TITLE):
            try:
                for t in w.descendants(control_type="Text"):
                    m = re.fullmatch(r"Desk ([A-P])", t.window_text())
                    if m:
                        found[m.group(1)] = w
                        break
            except Exception:  # noqa: BLE001 - a window closing under us
                continue
        if len(found) >= instances:
            return found
        time.sleep(0.5)
    raise RuntimeError(f"found {len(found)} of {instances} desk windows")


def wait_for_button(win, title: str, limit: float = 5.0) -> None:
    end = time.monotonic() + limit
    while time.monotonic() < end:
        if win.child_window(title=title, control_type="Button").exists(timeout=0.2):
            return
        time.sleep(0.1)
    raise RuntimeError(f"no {title!r} button")


def crowded(
    seed: int,
    instances: int,
    skip: set[str] = frozenset(),
    premature: set[str] = frozenset(),
    pending: set[str] = frozenset(),
    types: dict[str, str] | None = None,
) -> None:
    """Work every copy of the crowded desk: type the code, pick the shelf, Submit, then Confirm."""
    cells = C.derive_crowded(seed, instances)["cells"]
    windows = crowded_copies(instances)
    for cell in cells:
        label = cell["label"]
        if label in skip:
            continue
        win = windows[label]
        if label not in premature:
            fill_copy(win, cell, (types or {}).get(label))
        uia_invoke(win, "Submit")
        wait_for_button(win, "Confirm")
        if label not in pending:
            uia_invoke(win, "Confirm")
            time.sleep(0.2)


def labels(text: str | None) -> set[str]:
    return {x.strip() for x in (text or "").split(",") if x.strip()}


def disturb() -> None:
    """One deliberate disturbance of each kind the sentinel must see."""
    import ctypes

    time.sleep(2)
    win = Desktop(backend="uia").window(title_re="BenchLab — .*")
    win.set_focus()  # focus steal, activation change, raise over the user window
    time.sleep(1)
    mouse.move(coords=(300, 300))  # real pointer movement
    ctypes.windll.user32.OpenClipboard(0)
    ctypes.windll.user32.EmptyClipboard()  # clipboard change
    ctypes.windll.user32.CloseClipboard()
    time.sleep(2)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--disturb", action="store_true")
    parser.add_argument("--instance")
    parser.add_argument("--code")
    parser.add_argument("--shelf")
    parser.add_argument("--task", help="desk: UM-02 selects the busy user's desk (patterns only)")
    parser.add_argument("--instances", type=int, default=C.PA2_DEFAULT_INSTANCES)
    parser.add_argument("--skip", help="crowded: labels to leave alone, comma separated")
    parser.add_argument(
        "--premature", help="crowded: labels to Submit and Confirm without filling in"
    )
    parser.add_argument("--pending", help="crowded: labels to leave at the Confirm step")
    parser.add_argument("--type", action="append", default=[], metavar="L=TEXT",
                        help="crowded: type TEXT into copy L instead of its code")  # fmt: skip
    args = parser.parse_args()
    if args.disturb:
        disturb()
    elif args.mode == "desk":
        if not args.instance:
            parser.error("--instance is required for --mode desk")
        desk(args.seed, args.instance, args.code, args.shelf, args.task)
    elif args.mode == "crowded":
        crowded(
            args.seed, args.instances, labels(args.skip), labels(args.premature),
            labels(args.pending), dict(t.split("=", 1) for t in args.type),
        )  # fmt: skip
    else:
        globals()[args.mode](args.seed)
    time.sleep(1.0)


if __name__ == "__main__":
    main()
