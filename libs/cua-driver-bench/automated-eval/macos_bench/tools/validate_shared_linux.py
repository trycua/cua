#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Validate PA-01 and UM-01 (SHARED-DESKTOP.md) on a real Linux desktop, X11 or Hyprland, with no model.

  validate_shared_linux.py --desktop x11|hyprland [--out DIR] [--instances 4] [--cases pa,um]

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
    return {
        "title": re.search(r'titled "(BenchLab Desk [A-J])"', text)[1],
        "code": re.search(r"code ([A-Z]{2}-\d{4})", text)[1],
        "shelf": re.search(r'Select the "([^"]+)" shelf', text)[1],
    }


def atspi():
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    return Atspi


def find_window(Atspi, title: str, limit: float = 20.0):
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
                    return win
        time.sleep(0.3)
    raise RuntimeError(f"no accessible window {title!r}")


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
    if (
        args.act == "crosstalk" and desks and args.brief == desks[0]
    ):  # the first copy types another's code
        first = brief_values(desks[1])["code"]
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
    args, out: Path, task: str, condition: str, command: list[str], instances: int | None
) -> dict:
    argv = [sys.executable, str(LAUNCHERS[args.desktop]), "--task", task, "--run-index", "0", "--condition",
            condition, "--out", str(out), "--timeout", "120", "--label", "validate"]  # fmt: skip
    if instances:
        argv += ["--instances", str(instances)]
    subprocess.run([*argv, "--", *command], capture_output=True, timeout=400)
    return json.loads((out / "row.json").read_text(encoding="utf-8"))


def stand_in(args, what: str, linger: float = 2.0) -> list[str]:
    return [sys.executable, str(Path(__file__).resolve()), "--desktop", args.desktop, "--act", what,
            "--brief", "{brief}", "--linger", str(linger)]  # fmt: skip


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
    (out / "validation.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    return 0 if results and all(r["ok"] for r in results) else 1


def main() -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--desktop", choices=sorted(LAUNCHERS), required=True)
    p.add_argument("--out", type=Path, default=Path("/tmp/cdb-validate-shared"))
    p.add_argument("--instances", type=int, default=4)
    p.add_argument("--cases", default="pa,um")
    p.add_argument(
        "--um-linger",
        type=float,
        default=10.0,
        help="UM-01: seconds the stand-in stays after filling (the mouse user needs 5 clicks, 50 checks)",
    )
    p.add_argument(
        "--act", choices=["fill", "noop", "crosstalk", "displace", "cover"], help=argparse.SUPPRESS
    )
    p.add_argument("--brief", type=Path, help=argparse.SUPPRESS)
    p.add_argument("--linger", type=float, default=2.0, help=argparse.SUPPRESS)
    args = p.parse_args()
    return act(args) if args.act else validate(args)


if __name__ == "__main__":
    sys.exit(main())
