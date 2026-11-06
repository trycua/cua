#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Validate PA-01, UM-01 and the hard variants PA-02, UM-02 (SHARED-DESKTOP.md) on a real macOS desktop, with no
model.

  validate_shared.py --build DIR [--out DIR] [--instances 4] [--seconds 12] [--cases pa um pa2 um2]
                     [--pa2-instances 16 8]

DIR holds BenchLab.app, BenchSentinel.app (swift/build.sh) and benchinput (tools/benchinput.swift). Cases:

PA-01  oracle      N copies at their grid frames, each filled by its own thread through accessibility (System
                   Events; the real pointer is never used) at the same time -> pass, no cross-talk
       no-op       the same copies, untouched -> fail
       cross-talk  as the oracle, but copy 0's Code field first gets copy 1's code letters -> fail, cross-talk 1
UM-01  oracle      the mouse user runs while the desk is filled through accessibility -> checker pass, every
                   user click landed, displacement <= 3 px, no foreign pointer event
       displaced   benchinput moves the real pointer away mid-run -> displacement > 3 px (and foreign events)
       covered     a BenchLab window is brought over the user's canvas mid-run -> clicks lost

PA-02  (case `pa2`, run at N = 16 and N = 8; every copy has the same window title and a two-step Submit)
       oracle      all copies filled at once through accessibility (type the code, pick the shelf, Submit, then
                   Confirm) -> pass, no cross-talk
       no-op       untouched -> fail
       cross-talk  copy 0's Code field first gets a fragment of copy 1's code -> fail, cross_talk >= 1
       premature   copy 0 presses Submit and Confirm without being filled in -> that copy fails
       pending     copy 0 is filled and Submit is pressed but never confirmed -> that copy fails (confirm_resolved)
UM-02  (case `um2`: the busy user, schema-2 script with drags)
       oracle      every scripted click AND drag landed, displacement <= 3 px, checker pass
       displaced   benchinput warps the real pointer mid-run -> displacement > 3 px
       covered     a BenchLab window over the canvas -> lost clicks / drags
       cutclick    untagged clicks while a drag holds the button -> at least one `cut` drag
       cutwarp     pointer warps while a drag holds the button -> at least one `cut` drag

System Events needs Accessibility for the process that runs this script, BenchSentinel needs Accessibility
(to post) and Input Monitoring (for its tap). Run it only on a free desktop (a test VM): the mouse user moves
the real pointer. Exit status 0 when every case met its expectation.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "swift"))
sys.path.insert(0, str(BENCH / "probes" / "_common"))
import bench_v2 as v2  # noqa: E402
import benchlab_common as C  # noqa: E402
import shared_desktop as SD  # noqa: E402
import summarize_sentinel  # noqa: E402

SEED = 271828


def exe(build: Path, name: str) -> str:
    return str(
        next(p for p in (build / f"{name}.app" / "Contents" / "MacOS").iterdir() if p.is_file())
    )


def osa(script: str) -> str:
    done = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=30)
    if done.returncode:
        raise RuntimeError(done.stderr.strip())
    return done.stdout.strip()


def ax_fill(
    pid: int,
    code: str,
    shelf: str,
    wrong_first: str | None = None,
    confirm: bool = False,
    fill: bool = True,
    press_confirm: bool = True,
) -> None:
    """Fill one desk through accessibility only: Code field value, the shelf radio button, Submit. With
    `confirm` (PA-02) Submit is followed by the Confirm button (unless `press_confirm` is False); with
    `fill` False nothing is typed or picked (a premature Submit). The copy is found by its process id: the
    PA-02 copies share one window title."""
    proc = f"(first process whose unix id is {pid})"
    head = f'tell application "System Events" to tell window 1 of {proc} to '
    if wrong_first:
        osa(head + f'set value of text field 1 to "{wrong_first}"')
        time.sleep(0.4)
        osa(head + 'set value of text field 1 to ""')
        time.sleep(0.4)
    if fill:
        osa(head + f'set value of text field 1 to "{code}"')
        time.sleep(0.4)
        osa(head + f'click radio button "{shelf}"')
        time.sleep(0.3)
    osa(head + 'click button "Submit"')
    time.sleep(0.5)
    if confirm and press_confirm:
        osa(head + 'click button "Confirm"')
        time.sleep(0.5)


def start_desks(
    build: Path, plan: list[dict[str, Any]], tmp: Path
) -> list[subprocess.Popen[bytes]]:
    procs = []
    for e in plan:
        e["dir"].mkdir(parents=True, exist_ok=True)
        home = tmp / f"home-{e['label']}"
        home.mkdir(exist_ok=True)
        procs.append(
            subprocess.Popen(
                [exe(build, "BenchLab"), "--mode", "desk", *e["app_args"], "--no-activate"],
                env={**os.environ, "HOME": str(home)},
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
        )
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and not all(e["state"].exists() for e in plan):
        time.sleep(0.2)
    time.sleep(1.5)
    return procs


def stop(procs: list[subprocess.Popen[bytes]]) -> None:
    for p in procs:
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except OSError:
            pass
    for p in procs:
        try:
            p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)


def pa_case(build: Path, out: Path, n: int, kind: str, task: str = "PA-01") -> dict[str, Any]:
    crowded = SD.is_crowded(task)
    root = out / (f"pa2-{kind}-{n}" if crowded else f"pa-{kind}")
    plan = SD.pa_agents(SEED, n, root, task=task)
    with tempfile.TemporaryDirectory() as tmp:
        procs = start_desks(build, plan, Path(tmp))
        try:
            extra = {"copy_deadline_s": C.PA2_COPY_DEADLINE_S} if crowded else {}
            SD.write_meta(root, n, time.time() * 1000, SD.budget_s(task), **extra)
            if kind != "noop":
                cells = (C.derive_crowded if crowded else C.derive_parallel)(SEED, n)["cells"]
                threads = []
                for k, (e, p) in enumerate(zip(plan, procs)):
                    other = (
                        (cells[1]["code"][:8] if crowded else cells[1]["letters"]) if n > 1 else ""
                    )
                    wrong = other if kind == "crosstalk" and k == 0 else None
                    c = cells[k]
                    opts = {"confirm": crowded}
                    if kind == "premature" and k == 0:
                        opts["fill"] = False
                    if kind == "pending" and k == 0:
                        opts["press_confirm"] = False
                    threads.append(
                        threading.Thread(
                            target=ax_fill,
                            args=(p.pid, c["code"], c["shelf"], wrong),
                            kwargs=opts,
                        )
                    )
                for t in threads:
                    t.start()
                for t in threads:
                    t.join()
            time.sleep(1.0)
        finally:
            stop(procs)
    result = root / "result.json"
    subprocess.run(
        SD.evaluate_argv(sys.executable, SEED, root, result, task), check=True, capture_output=True
    )
    r = json.loads(result.read_text())
    p = (r.get("diagnostics") or {}).get("parallel", {}).get("value") or {}
    one_failed = not r["passed"] and (n == 1 or p.get("passed") == n - 1)
    expect = {"oracle": r["passed"] and p.get("cross_talk") == 0,
              "noop": not r["passed"],
              "crosstalk": not r["passed"] and (p.get("cross_talk") or 0) >= 1,
              "premature": one_failed,
              "pending": one_failed}[kind]  # fmt: skip
    return {"case": f"{task} {kind} (N={n})", "ok": bool(expect), "passed": r["passed"], "parallel": p,
            "harness_error": r.get("harness_error")}  # fmt: skip


def um_case(
    build: Path, out: Path, kind: str, seconds: float, task: str = "UM-01"
) -> dict[str, Any]:
    root = out / (f"um2-{kind}" if SD.is_busy_user(task) else f"um-{kind}")
    root.mkdir(parents=True, exist_ok=True)
    script = SD.write_mouse_script(SEED, root / "mouse-user.json", task)
    log = root / "sentinel.jsonl"
    log.unlink(missing_ok=True)
    cell = SD.um_desk(SEED, task)
    plan = [{"label": cell["label"], "dir": root / "desk", "state": root / "desk" / "state.json",
             "app_args": ["--seed", str(SEED), "--state", str(root / "desk" / "state.json"),
                          "--events", str(root / "desk" / "events.jsonl"), *C.desk_app_args(cell)]}]  # fmt: skip
    with tempfile.TemporaryDirectory() as tmp:
        procs = start_desks(build, plan, Path(tmp))
        sentinel = subprocess.Popen(
            [exe(build, "BenchSentinel"), "--log", str(log), *v2.sentinel_args("background", root / "mouse-user.json")],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        )  # fmt: skip
        procs.append(sentinel)
        try:
            time.sleep(2.5)
            sentinel.send_signal(signal.SIGUSR1)
            time.sleep(1.5)
            sentinel.send_signal(signal.SIGUSR2)  # arm
            time.sleep(seconds / 3)
            if kind == "oracle":
                ax_fill(procs[0].pid, cell["code"], cell["shelf"])
            elif kind == "displaced":
                subprocess.run([str(build / "benchinput"), "move", "1700", "980"], check=False)
            elif kind in ("cutclick", "cutwarp"):
                # Something other than the user presses or warps while a drag holds the button.
                ux, uy, _, _ = v2.USER_WINDOW_FRAME
                end = time.monotonic() + seconds * 2 / 3 - 1.0
                while time.monotonic() < end:
                    args = (["click", str(ux + 220), str(uy + 200)] if kind == "cutclick"
                            else ["move", str(ux + 700), str(uy + 450)])  # fmt: skip
                    subprocess.run([str(build / "benchinput"), *args], check=False)
                    time.sleep(0.15)
            elif kind == "covered":
                ux, uy, _, _ = v2.USER_WINDOW_FRAME
                cover = SD.C.desk_app_args({"label": "Z", "frame": [ux + 10, uy + 30, 900, 420]})
                procs += start_desks(build, [{"label": "Z", "dir": root / "cover", "state": root / "cover" / "state.json",
                                              "app_args": ["--seed", "1", "--state", str(root / "cover" / "state.json"),
                                                           "--events", str(root / "cover" / "events.jsonl"), *cover]}],
                                     Path(tmp))  # fmt: skip
                subprocess.run(
                    [str(build / "benchinput"), "activate", str(procs[-1].pid)], check=False
                )
            time.sleep(seconds * 2 / 3)
            sentinel.send_signal(signal.SIGUSR2)  # disarm
            time.sleep(1.0)
        finally:
            stop(procs)
    s = summarize_sentinel.summarize(str(log))
    mu = s.get("mouse_user") or {}
    crit = v2.multiplayer_criteria(s)
    problem = v2.mouse_user_problem(s)
    if kind == "oracle":
        result = root / "result.json"
        subprocess.run([sys.executable, str(BENCH / "probes" / task / "evaluate.py"), "--seed", str(SEED),
                        "--state", str(root / "desk" / "state.json"), "--events", str(root / "desk" / "events.jsonl"),
                        "--result", str(result), "--sentinel", str(log)], check=True, capture_output=True)  # fmt: skip
        r = json.loads(result.read_text())
        ok = r["passed"] and not problem and crit["pointer"] and crit["typing"]
        if SD.is_busy_user(task):
            ok = ok and int(mu.get("drags") or 0) >= 1 and mu.get("drags_landed") == mu.get("drags")
            ok = ok and mu.get("clicks_landed") == mu.get("clicks")
    elif kind == "displaced":
        ok = not problem and not crit["pointer"]
    elif kind in ("cutclick", "cutwarp"):
        ok = int(mu.get("drags_cut") or 0) >= 1
    else:
        ok = not problem and int(mu.get("clicks_lost") or 0) > 0 and crit["typing"] is False
        if SD.is_busy_user(task):
            ok = ok and int(mu.get("drags") or 0) >= 1 and mu.get("drags_landed") == 0
    return {"case": f"{task} {kind}", "ok": bool(ok), "problem": problem, "mouse_user": mu,
            "pointer": crit["pointer"], "input": crit["typing"]}  # fmt: skip


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--build", type=Path, required=True)
    p.add_argument("--out", type=Path, default=Path("validation-shared"))
    p.add_argument("--instances", type=int, default=4)
    p.add_argument("--seconds", type=float, default=12.0)
    p.add_argument("--pa2-instances", type=int, nargs="+", default=[16, 8])
    p.add_argument("--cases", nargs="+", default=["pa", "um"], choices=["pa", "um", "pa2", "um2"])
    args = p.parse_args(argv)
    args.out.mkdir(parents=True, exist_ok=True)
    results = []
    if "pa" in args.cases:
        for kind in ("oracle", "noop", "crosstalk"):
            results.append(pa_case(args.build, args.out, args.instances, kind))
            print(json.dumps(results[-1])[:400], flush=True)
    if "um" in args.cases:
        for kind in ("oracle", "displaced", "covered"):
            results.append(um_case(args.build, args.out, kind, args.seconds))
            print(json.dumps(results[-1])[:400], flush=True)
    if "pa2" in args.cases:
        for n in args.pa2_instances:
            for kind in ("oracle", "noop", "crosstalk", "premature", "pending"):
                results.append(pa_case(args.build, args.out, n, kind, "PA-02"))
                print(json.dumps(results[-1])[:400], flush=True)
    if "um2" in args.cases:
        for kind in ("oracle", "displaced", "covered", "cutclick", "cutwarp"):
            results.append(um_case(args.build, args.out, kind, args.seconds, "UM-02"))
            print(json.dumps(results[-1])[:400], flush=True)
    (args.out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(r["ok"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
