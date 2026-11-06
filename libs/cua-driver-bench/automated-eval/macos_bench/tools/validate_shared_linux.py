#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Validate PA-01, UM-01 and the hard variants PA-02, UM-02 (SHARED-DESKTOP.md) on a real Linux desktop, X11 or
Hyprland (the hard variants: X11 only), with no model.

  validate_shared_linux.py --desktop x11|hyprland [--out DIR] [--instances 4] [--cases pa,um]
  validate_shared_linux.py --desktop x11 --cases pa2,um2 [--instances-hard 16,8]

Every case runs one trial through the desktop's session launcher (launch/x11_session.py or
launch/hyprland_session.py), with this script in the agent's place (`--act ...`):

PA-01  oracle      every copy filled by its own process through AT-SPI actions only (set the Code text, the
                   shelf radio's and Submit's default action; no pointer, no keyboard), all at once -> pass, no
                   cross-talk
       no-op       the same copies, untouched -> fail
       cross-talk  as the oracle, but the first copy's Code field first gets another copy's code -> fail, cross-talk 1
UM-01  oracle      the mouse user runs while the desk is filled through AT-SPI -> checker pass, every user click
                   landed, displacement <= 3 px
       displaced   the real pointer is moved away mid-run (xdotool / the compositor) -> displacement > 3 px
       covered     the BenchLab window is put over the user's canvas mid-run -> clicks lost

PA-02  (--cases pa2, X11, at N = 16 and 8; every copy has the one title "BenchLab Desk", told apart by the heading)
       oracle      every copy worked at once through AT-SPI actions (type the code, pick the shelf, Submit, then
                   the Confirm button that appears) -> pass, no cross-talk
       no-op       untouched -> fail
       cross-talk  the first copy first gets another copy's code typed into it -> fail, cross-talk >= 1
       premature   the first copy presses Submit then Confirm before anything is filled in -> that copy fails
       pending     the first copy is filled and Submit is pressed but Confirm is not -> that copy fails
UM-02  (--cases um2, X11) the busy user: clicks and drags with the button held, a finer pointer
       oracle      the desk is filled through AT-SPI -> checker pass, every click and drag landed, displacement <= 3 px
       displaced   the real pointer is moved away mid-run -> displacement > 3 px
       covered     the BenchLab window is put over the user's canvas -> clicks and drags lost
       cut         a second synthetic click (with a warp) while the button is held -> a cut drag

Run it only on a free test desktop: the mouse user moves the real pointer. Exit status 0 when every case met its
expectation.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
LAUNCHERS = {
    "x11": BENCH / "launch" / "x11_session.py",
    "hyprland": BENCH / "launch" / "hyprland_session.py",
}


# --- the agent stand-in (runs inside a trial)
def brief_values(brief: Path) -> dict:
    text = brief.read_text(encoding="utf-8")
    title = re.search(r'titled "(BenchLab Desk(?: [A-P])?)"', text)
    heading = re.search(r'headed "Desk ([A-P])"', text)  # PA-02: one title for every copy
    code = re.search(r"Type the code (\S+) into", text) or re.search(r"code ([A-Z]{2}-\d{4})", text)
    return {
        "title": title[1] if title else "BenchLab Desk",
        "label": heading[1] if heading else None,
        "code": code[1],
        "shelf": re.search(r'Select the "([^"]+)" shelf', text)[1],
    }


def atspi():
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    return Atspi


def find_window(Atspi, title: str, limit: float = 20.0, label: str | None = None):
    """The accessible window `title`; with `label` the one whose heading reads "Desk <label>" (the PA-02 copies
    share one title, so the heading, not the title, tells them apart)."""
    end = time.monotonic() + limit
    while time.monotonic() < end:
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(i)
            if app is None:
                continue
            for j in range(app.get_child_count()):
                win = app.get_child_at_index(j)
                if win is not None and win.get_name() == title:
                    if label is None or find(win, "label", f"Desk {label}") is not None:
                        return win
        time.sleep(0.3)
    raise RuntimeError(f"no accessible window {title!r} {label or ''}".strip())


def find(node, role: str, name: str):
    """The first node of this role and name (role names as AT-SPI spells them; newer at-spi2-core says "button" for
    "push button")."""
    roles = {role, role.replace("push button", "button")}
    if node.get_role_name() in roles and node.get_name() == name:
        return node
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None and (hit := find(child, role, name)) is not None:
            return hit
    return None


def fill(title: str, code: str, shelf: str, first: str | None = None) -> None:
    Atspi = atspi()
    win = find_window(Atspi, title)
    field = find(win, "text", "Code")
    if first:
        field.set_text_contents(first)
        time.sleep(0.4)
        field.set_text_contents("")
        time.sleep(0.4)
    field.set_text_contents(code)
    time.sleep(0.4)
    find(win, "radio button", shelf).do_action(0)
    time.sleep(0.3)
    find(win, "push button", "Submit").do_action(0)
    time.sleep(0.5)


def fill_hard(label: str, code: str, shelf: str, what: str, first: str | None = None) -> None:
    """One PA-02 copy, through AT-SPI actions only: Code text, shelf radio, Submit, then the Confirm button that
    Submit reveals. `what`: fill | premature (Submit and Confirm with nothing filled in) | pending (no Confirm)."""
    Atspi = atspi()
    win = find_window(Atspi, "BenchLab Desk", label=label)

    def press_confirm() -> None:
        end = time.monotonic() + 10
        while time.monotonic() < end:
            button = find(win, "push button", "Confirm")
            if button is not None:
                button.do_action(0)
                time.sleep(0.4)
                return
            time.sleep(0.2)
        raise RuntimeError(f"copy {label}: no Confirm button after Submit")

    if what == "premature":
        find(win, "push button", "Submit").do_action(0)
        time.sleep(0.4)
        press_confirm()
        return
    field = find(win, "text", "Code")
    if first:
        field.set_text_contents(first)
        time.sleep(0.4)
        field.set_text_contents("")
        time.sleep(0.4)
    field.set_text_contents(code)
    time.sleep(0.4)
    find(win, "radio button", shelf).do_action(0)
    time.sleep(0.3)
    find(win, "push button", "Submit").do_action(0)
    time.sleep(0.5)
    if what != "pending":
        press_confirm()


def cut_drags(seconds: float) -> None:
    """Steal the button mid-drag: a second synthetic click (after a pointer warp) while the user holds the button."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        subprocess.run(["xdotool", "mousemove", "5", "5", "click", "1"], check=True, timeout=10)
        time.sleep(0.1)


def move_pointer_away(desktop: str) -> None:
    if desktop == "x11":
        subprocess.run(["xdotool", "mousemove", "5", "5"], check=True, timeout=10)
    else:
        sys.path.insert(0, str(BENCH / "hyprland"))
        import hypr

        hypr.move_cursor(5, 5)


def cover_canvas(desktop: str, title: str) -> None:
    frame = (440, 380, 600, 400)  # over the user window's canvas
    if desktop == "x11":
        window = subprocess.run(["xdotool", "search", "--name", f"^{title}$"], capture_output=True, text=True,
                                timeout=10).stdout.split()[0]  # fmt: skip
        subprocess.run(["xdotool", "windowmove", window, str(frame[0]), str(frame[1]), "windowraise", window],
                       check=True, timeout=10)  # fmt: skip
    else:
        sys.path.insert(0, str(BENCH / "hyprland"))
        import hypr

        client = next(c for c in hypr.clients() if c.get("title") == title)
        hypr.place(client["address"], frame)
        hypr.raise_to_top(client["address"])


def act(args: argparse.Namespace) -> int:
    values = brief_values(args.brief)
    first = None
    desks = sorted(args.brief.parent.parent.glob("desk-*/brief.md"))
    is_first = bool(desks) and args.brief == desks[0]
    if args.act == "crosstalk" and is_first:  # the first copy types another's code
        first = brief_values(desks[1])["code"]
    if args.task == "PA-02":
        what = "fill"
        if args.act == "noop":
            what = None
        elif args.act in ("premature", "pending") and is_first:
            what = args.act  # only the first copy misbehaves
        if what:
            fill_hard(values["label"], values["code"], values["shelf"], what, first)
        time.sleep(args.linger)
        return 0
    if args.act == "cutdrag":
        time.sleep(2.5)
        cut_drags(6.0)
        time.sleep(args.linger)
        return 0
    if args.act == "displace":
        time.sleep(4)
        move_pointer_away(args.desktop)
    if args.act == "cover":
        time.sleep(3)
        cover_canvas(args.desktop, values["title"])
    if args.act != "noop":
        fill(values["title"], values["code"], values["shelf"], first)
    time.sleep(args.linger)
    return 0


# --- the validation driver
def trial(
    args,
    out: Path,
    task: str,
    condition: str,
    command: list[str],
    instances: int | None,
    timeout: int = 120,
) -> dict:
    argv = [sys.executable, str(LAUNCHERS[args.desktop]), "--task", task, "--run-index", "0", "--condition",
            condition, "--out", str(out), "--timeout", str(timeout), "--label", "validate"]  # fmt: skip
    if instances:
        argv += ["--instances", str(instances)]
    subprocess.run([*argv, "--", *command], capture_output=True, timeout=timeout + 300)
    return json.loads((out / "row.json").read_text(encoding="utf-8"))


def stand_in(args, what: str, linger: float = 2.0, task: str = "PA-01") -> list[str]:
    return [sys.executable, str(Path(__file__).resolve()), "--desktop", args.desktop, "--act", what,
            "--brief", "{brief}", "--linger", str(linger), "--task", task]  # fmt: skip


def mouse(row: dict) -> dict:
    return (row.get("disturbance") or {}).get("mouse_user") or {}


def validate(args: argparse.Namespace) -> int:
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    results = []

    def record(name: str, row: dict, ok: bool, detail: dict) -> None:
        results.append(
            {"case": name, "ok": ok, "harness_error": row.get("harness_error"), **detail}
        )
        print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(detail), flush=True)

    cases = set(args.cases.split(","))
    if "pa" in cases:
        for condition in ("foreground", "background"):
            for name, what in (("oracle", "fill"), ("no-op", "noop"), ("cross-talk", "crosstalk")):
                row = trial(args, out / f"pa-{condition}-{name}", "PA-01", condition, stand_in(args, what),
                            args.instances)  # fmt: skip
                par = row.get("parallel") or {}
                detail = {"checker": row.get("checker_passed"), "copies_passed": par.get("passed"),
                          "cross_talk": par.get("cross_talk"), "a22": (row.get("a22") or {}).get("user_undisturbed")}  # fmt: skip
                ok = (
                    not row.get("harness_error")
                    and {
                        "oracle": row.get("checker_passed") is True and par.get("cross_talk") == 0,
                        "no-op": row.get("checker_passed") is False,
                        "cross-talk": row.get("checker_passed") is False
                        and par.get("cross_talk") == 1,
                    }[name]
                )
                record(f"PA-01 {condition} {name} (N={args.instances})", row, ok, detail)
    if "um" in cases:
        for name, what in (("oracle", "fill"), ("displaced", "displace"), ("covered", "cover")):
            row = trial(
                args,
                out / f"um-{name}",
                "UM-01",
                "background",
                stand_in(args, what, args.um_linger),
                None,
            )
            mu = mouse(row)
            detail = {"checker": row.get("checker_passed"), "clicks": mu.get("clicks"),
                      "landed": mu.get("clicks_landed"), "lost": mu.get("clicks_lost"),
                      "max_px": mu.get("max_displacement_px"), "checks": mu.get("checks"),
                      "a22": (row.get("a22") or {}).get("user_undisturbed")}  # fmt: skip
            clicks = int(mu.get("clicks") or 0)
            ok = (
                not row.get("harness_error")
                and {
                    "oracle": row.get("checker_passed") is True
                    and clicks >= 5
                    and mu.get("clicks_landed") == clicks
                    and float(mu.get("max_displacement_px") or 0) <= 3,
                    "displaced": float(mu.get("max_displacement_px") or 0) > 3,
                    "covered": int(mu.get("clicks_lost") or 0) > 0,
                }[name]
            )
            record(f"UM-01 {name}", row, ok, detail)
    if cases & {"pa2", "um2"} and args.desktop != "x11":
        print("PA-02 and UM-02 run on X11 only: skipping", flush=True)
        cases -= {"pa2", "um2"}
    if "pa2" in cases:
        for n in (int(v) for v in args.instances_hard.split(",")):
            plan = [("oracle", "fill", "foreground"), ("oracle", "fill", "background"),
                    ("no-op", "noop", "foreground"), ("cross-talk", "crosstalk", "foreground"),
                    ("premature-confirm", "premature", "foreground"),
                    ("pending-confirm", "pending", "foreground")]  # fmt: skip
            for name, what, condition in plan:
                row = trial(args, out / f"pa2-{n}-{condition}-{name}", "PA-02", condition,
                            stand_in(args, what, task="PA-02"), n, timeout=200)  # fmt: skip
                par = row.get("parallel") or {}
                detail = {"checker": row.get("checker_passed"), "copies_passed": par.get("passed"),
                          "cross_talk": par.get("cross_talk"), "a22": (row.get("a22") or {}).get("user_undisturbed")}  # fmt: skip
                cross = int(par.get("cross_talk") or 0)
                ok = (
                    not row.get("harness_error")
                    and {
                        "oracle": row.get("checker_passed") is True
                        and par.get("passed") == n
                        and cross == 0,
                        "no-op": row.get("checker_passed") is False and par.get("passed") == 0,
                        "cross-talk": row.get("checker_passed") is False and cross >= 1,
                        "premature-confirm": row.get("checker_passed") is False
                        and par.get("passed") == n - 1,
                        "pending-confirm": row.get("checker_passed") is False
                        and par.get("passed") == n - 1,
                    }[name]
                )
                record(f"PA-02 {condition} {name} (N={n})", row, ok, detail)
    if "um2" in cases:
        for name, what in (("oracle", "fill"), ("displaced", "displace"), ("covered", "cover"),
                           ("cut-drag", "cutdrag")):  # fmt: skip
            row = trial(args, out / f"um2-{name}", "UM-02", "background",
                        stand_in(args, what, args.um_linger, task="UM-02"), None)  # fmt: skip
            mu = mouse(row)
            detail = {"checker": row.get("checker_passed"), "clicks": mu.get("clicks"),
                      "landed": mu.get("clicks_landed"), "lost": mu.get("clicks_lost"),
                      "drags": mu.get("drags"), "drags_landed": mu.get("drags_landed"),
                      "drags_cut": mu.get("drags_cut"), "max_px": mu.get("max_displacement_px"),
                      "checks": mu.get("checks"), "a22": (row.get("a22") or {}).get("user_undisturbed")}  # fmt: skip
            clicks, drags = int(mu.get("clicks") or 0), int(mu.get("drags") or 0)
            ok = (
                not row.get("harness_error")
                and {
                    "oracle": row.get("checker_passed") is True
                    and clicks >= 5
                    and drags >= 1
                    and mu.get("clicks_landed") == clicks
                    and mu.get("drags_landed") == drags
                    and float(mu.get("max_displacement_px") or 0) <= 3,
                    "displaced": float(mu.get("max_displacement_px") or 0) > 3,
                    "covered": int(mu.get("clicks_lost") or 0) > 0
                    and int(mu.get("drags_landed") or 0) < drags,
                    "cut-drag": int(mu.get("drags_cut") or 0) >= 1,
                }[name]
            )
            record(f"UM-02 {name}", row, ok, detail)
    (out / "validation.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    return 0 if results and all(r["ok"] for r in results) else 1


def main() -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--desktop", choices=sorted(LAUNCHERS), required=True)
    p.add_argument("--out", type=Path, default=Path("/tmp/cdb-validate-shared"))
    p.add_argument("--instances", type=int, default=4)
    p.add_argument(
        "--cases",
        default="pa,um",
        help="any of pa, um, pa2, um2 (comma separated; pa2/um2: X11 only)",
    )
    p.add_argument(
        "--instances-hard",
        default="16,8",
        help="PA-02: the copy counts to validate (comma separated)",
    )
    p.add_argument(
        "--um-linger",
        type=float,
        default=10.0,
        help="UM-01: seconds the stand-in stays after filling (the mouse user needs 5 clicks, 50 checks)",
    )
    p.add_argument(
        "--act",
        choices=[
            "fill",
            "noop",
            "crosstalk",
            "displace",
            "cover",
            "premature",
            "pending",
            "cutdrag",
        ],
        help=argparse.SUPPRESS,
    )
    p.add_argument("--task", default="PA-01", help=argparse.SUPPRESS)
    p.add_argument("--brief", type=Path, help=argparse.SUPPRESS)
    p.add_argument("--linger", type=float, default=2.0, help=argparse.SUPPRESS)
    args = p.parse_args()
    return act(args) if args.act else validate(args)


if __name__ == "__main__":
    sys.exit(main())
