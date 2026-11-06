# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Scripted oracle for the Windows BenchLab modes: solves a task through the GUI with real input.

Used only to validate the port (each task solvable, its checker passes); never a bench setup. It finds
controls through UI Automation (pywinauto) and acts with real pointer and keyboard input in the foreground.

Usage: oracle.py --mode MODE --seed N [--disturb] [--instance L [--code TEXT] [--shelf NAME]]
`--disturb` instead performs one deliberate disturbance of the user window (validates the sentinel).
`--mode desk` does the PA-01 / UM-01 sub-task in the window "BenchLab Desk L" (`--instance`): types the cell's
code, selects its shelf and presses Submit. `--code` and `--shelf` override what to type and select (to
rehearse a wrong or crossed-over agent).
"""

from __future__ import annotations

import argparse
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


def desk_cell(seed: int, instance: str) -> dict:
    """The seed's code and shelf for the desk labelled `instance` (a PA-01 copy, or the UM-01 desk)."""
    for cell in C.derive_parallel(seed, C.PA_MAX_INSTANCES)["cells"]:
        if cell["label"] == instance:
            return cell
    um = C.derive_um_desk(seed)
    if um["label"] == instance:
        return um
    raise SystemExit(f"no desk {instance!r} for seed {seed}")


def desk(seed: int, instance: str, code: str | None = None, shelf: str | None = None) -> None:
    cell = desk_cell(seed, instance)
    win = lab("desk", C.desk_title(instance))
    type_into(win, "Code", cell["code"] if code is None else code)
    click(
        win.child_window(
            title=cell["shelf"] if shelf is None else shelf, control_type="RadioButton"
        )
    )
    click(button(win, "Submit"))


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
    args = parser.parse_args()
    if args.disturb:
        disturb()
    elif args.mode == "desk":
        if not args.instance:
            parser.error("--instance is required for --mode desk")
        desk(args.seed, args.instance, args.code, args.shelf)
    else:
        globals()[args.mode](args.seed)
    time.sleep(1.0)


if __name__ == "__main__":
    main()
