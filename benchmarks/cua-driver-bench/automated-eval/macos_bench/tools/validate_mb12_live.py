#!/usr/bin/env python3
"""Live validation of MB-12 with a model-free background actor. No model call.

BenchLab (forms mode) runs behind BenchSentinel, which is frontmost and key. The actor fills the three fields
and submits through Cua Driver 0.34.0 (the private agent daemon), using background accessibility actions only:
no pointer, no focus change. Then the MB-12 evaluator runs on the real sentinel log. Two runs:

  clean   background AX actions only                      -> must pass (disturbance checks included)
  noisy   the actor first raises BenchLab to the front    -> must fail the disturbance checks (front_unchanged)

  validate_mb12_live.py [--explore] [--out FILE]      (run from Terminal.app on an otherwise idle desktop)
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "swift"))
sys.path.insert(0, str(ROOT / "probes" / "_common"))

import bench_core as core  # noqa: E402
import benchlab_common as C  # noqa: E402
import claude_arms as ca  # noqa: E402
import run_bench as rb  # noqa: E402
import run_pilot as pilot  # noqa: E402


def call(tool: str, args: dict, timeout: float = 40) -> dict:
    args = {**args, "session": "mb12"}  # element tokens are bound to a session
    done = ca.cua_cli("call", tool, json.dumps(args), timeout=timeout)
    text = done.stdout
    try:
        return json.loads(text[text.index("{") :])
    except ValueError:
        return {"_raw": text[-400:], "_err": done.stderr[-300:]}


def find_lab_window() -> tuple[int, int]:
    data = call("list_windows", {})
    for w in data.get("windows", []):
        if w.get("app_name") == "BenchLab":
            return int(w["pid"]), int(w["window_id"])
    raise RuntimeError("BenchLab window not found")


def elements(pid: int, wid: int) -> list[dict]:
    return call("get_window_state", {"pid": pid, "window_id": wid, "include_screenshot": False}).get(
        "elements", []
    )


def run_once(mode: str, build: Path, explore: bool) -> dict:
    task = rb.load_tasks(ROOT / "probes")["MB-12"]
    seed = core.probe_seed("MB-12", 7 if mode == "noisy" else 6)
    work = Path(tempfile.mkdtemp(prefix=f"mb12-{mode}-"))
    art = work / "artifacts"
    art.mkdir()
    lab_app = build / "BenchLab.app"
    brief, paths = pilot.prepare_probe(task, seed, work, lab_app)
    p = C.derive_forms(seed)
    sentinel = pilot.Sentinel(build / "BenchSentinel.app", art / "sentinel.jsonl")
    rb.kill_bench_apps()
    sentinel.start()
    lab = pilot.launch_lab(task, seed, paths, lab_app, pilot.clean_app_env(work / "apphome"))
    sentinel.activate()
    pid, wid = find_lab_window()
    els = elements(pid, wid)
    if explore:
        for e in els:
            print(e.get("element_index"), e.get("role"), repr(e.get("label")), repr(e.get("value")), e.get("actions"))
    sentinel.toggle_armed()
    time.sleep(0.5)
    notes: list[str] = []
    try:
        if mode == "noisy":
            call("bring_to_front", {"pid": pid, "window_id": wid})
            time.sleep(0.5)
        def fresh(label: str) -> dict | None:
            return next((e for e in elements(pid, wid) if str(e.get("label")) == label), None)

        def act(tool: str, label: str, extra: dict | None = None) -> None:
            e = fresh(label)  # one fresh snapshot per action: tokens die with the next snapshot
            if e is None:
                notes.append(f"missing element {label!r}")
                return
            args = {"pid": pid, "window_id": wid, "element_token": e["element_token"], **(extra or {})}
            res = call(tool, args)
            notes.append(f"{tool} {label!r}: {json.dumps(res)[:120]}")

        act("set_value", "Customer name", {"value": p["customer_name"]})
        # category: set the popup's value (no menu); opening the menu makes the sentinel resign key for ~2 s
        e = fresh("Category")
        res = call("set_value", {"pid": pid, "window_id": wid, "element_token": e["element_token"], "value": p["category_title"]}) if e else {}
        notes.append(f"set_value 'Category': {json.dumps(res)[:140]}")
        shown = (fresh("Category") or {}).get("value")
        if shown != p["category_title"]:
            notes.append(f"popup value after set_value is {shown!r}; falling back to press + pick (opens the menu)")
            act("click", "Category")
            time.sleep(0.5)
            item = fresh(p["category_title"])
            if item is not None:
                call("click", {"pid": pid, "window_id": wid, "element_token": item["element_token"], "action": "pick"})
                notes.append(f"picked {p['category_title']!r}")
        act("click", p["priority"])
        act("click", "Submit")
        time.sleep(1.0)
    finally:
        sentinel.toggle_armed()
        time.sleep(0.5)
        sentinel.stop()
        lab_alive = lab.poll() is None
        evaluation = rb.evaluate_probe(task, seed, paths, art, art / "sentinel.jsonl")
        import claude_driver  # noqa: PLC0415

        claude_driver.kill_group(lab.pid)
        rb.kill_bench_apps()
    return {"mode": mode, "seed": seed, "lab_alive_before_eval": lab_alive, "evaluation": evaluation, "notes": notes, "dir": str(work)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--explore", action="store_true")
    ap.add_argument("--modes", nargs="+", default=["clean", "noisy"])
    ap.add_argument("--build-dir", type=Path, default=ca.WORK / "build")
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()
    proc = ca.start_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE, overlay=False, log_name="mb12.log")
    results = []
    try:
        for mode in args.modes:
            r = run_once(mode, args.build_dir, args.explore)
            print(json.dumps(r, indent=1)[:2500])
            results.append(r)
    finally:
        ca.stop_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE / "home")
        proc.terminate()
    ok = (
        len(results) == 2
        and results[0]["evaluation"].get("passed") is True
        and results[1]["evaluation"].get("passed") is False
    )
    if args.out:
        args.out.write_text(json.dumps({"ok": ok, "runs": results}, indent=2) + "\n", "utf-8")
    print("MB12_LIVE_OK" if ok else "MB12_LIVE_NOT_OK")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
