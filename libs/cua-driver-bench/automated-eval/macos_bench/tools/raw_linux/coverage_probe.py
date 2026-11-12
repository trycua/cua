# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""AT-SPI coverage of the apps a task starts, measured, for the raw-linux arm's per-app fallback labels.

Usage: coverage_probe.py TASK_ID [--settle S] [--out FILE]   (inside a Linux session with the AT-SPI bus)

Starts the task's apps the way the runner does (BenchLab for MB/IR tasks, the pack descriptor for CDB tasks), waits,
then walks each app's AT-SPI tree and prints one JSON object per app:
  toolkit, nodes, showing, and how many showing nodes are interactive by role versus reachable through an AT-SPI
  Action, EditableText, Value or Selection interface. ``label`` is "full" when every showing interactive node has
  a semantic route and the tree is not empty, "partial" when some do not or a document/table role hides its cells,
  "none" when the app is not on the bus.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent))

import gi  # noqa: E402

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

INTERACTIVE = {
    "push button",
    "button",
    "toggle button",
    "check box",
    "radio button",
    "combo box",
    "menu item",
    "check menu item",
    "radio menu item",
    "text",
    "entry",
    "password text",
    "spin button",
    "slider",
    "link",
    "list item",
    "table cell",
    "tree item",
    "page tab",
    "menu",
}
OPAQUE = {"document spreadsheet", "document frame", "canvas", "drawing area"}  # cells or content behind one node


def walk(node, depth=0, out=None, limit=4000):
    out = out if out is not None else []
    if node is None or len(out) >= limit or depth > 60:
        return out
    try:
        out.append((node, depth))
        n = node.get_child_count()
    except Exception:  # noqa: BLE001
        return out
    for i in range(min(n, 500)):  # a spreadsheet table reports millions of cells
        try:
            walk(node.get_child_at_index(i), depth + 1, out, limit)
        except Exception:  # noqa: BLE001
            continue
    return out


def routes(node) -> list[str]:
    r = []
    if node.get_action_iface() is not None and Atspi.Action.get_n_actions(node) > 0:
        r.append("action")
    if node.get_editable_text_iface() is not None:
        r.append("editable_text")
    if node.get_value_iface() is not None:
        r.append("value")
    if node.get_selection_iface() is not None:
        r.append("selection")
    return r


def probe_app(app) -> dict:
    nodes = walk(app)
    showing = interactive = covered = 0
    roles: dict[str, int] = {}
    uncovered: list[str] = []
    opaque: list[str] = []
    for node, _ in nodes:
        try:
            role = node.get_role_name()
            if not node.get_state_set().contains(Atspi.StateType.SHOWING):
                continue
        except Exception:  # noqa: BLE001
            continue
        showing += 1
        roles[role] = roles.get(role, 0) + 1
        if role in OPAQUE:
            opaque.append(role)
        if role in INTERACTIVE:
            interactive += 1
            if routes(node):
                covered += 1
            elif len(uncovered) < 8:
                uncovered.append(f"{role} {node.get_name()!r}")
    try:
        toolkit = f"{app.get_toolkit_name()} {app.get_toolkit_version()}"
    except Exception:  # noqa: BLE001
        toolkit = "?"
    label = "full" if interactive and covered == interactive and not opaque else "partial"
    return {
        "app": app.get_name(),
        "pid": app.get_process_id(),
        "toolkit": toolkit,
        "nodes": len(nodes),
        "showing": showing,
        "interactive": interactive,
        "covered": covered,
        "opaque_roles": sorted(set(opaque)),
        "uncovered_sample": uncovered,
        "top_roles": dict(sorted(roles.items(), key=lambda kv: -kv[1])[:10]),
        "label": label,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("task")
    parser.add_argument("--settle", type=float, default=15)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    spec = json.loads((HERE.parent.parent / "probes" / args.task / "task.json").read_text("utf-8"))
    procs = []
    cdb = None
    art = Path("/tmp/coverage-probe") / args.task
    art.mkdir(parents=True, exist_ok=True)
    if spec.get("kind") == "cdb":
        import cdb_adapter

        cdb = cdb_adapter.CdbTask(spec, art, seed=1)
        cdb.reset()
        cdb.start_apps()
    else:
        import subprocess

        procs.append(
            subprocess.Popen(
                [sys.executable, str(HERE.parent.parent / "linux" / "benchlab.py"), *spec["app_args"],
                 "--seed", "1", "--state", str(art / "state.json"), "--events", str(art / "events.jsonl")]
            )
        )
    try:
        time.sleep(args.settle)
        desktop = Atspi.get_desktop(0)
        rows = []
        for i in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(i)
            if app is None:
                continue
            try:
                rows.append(probe_app(app))
            except Exception as error:  # noqa: BLE001
                rows.append({"app": str(app), "error": str(error)})
        text = "\n".join(json.dumps(r) for r in rows)
        print(text)
        if args.out:
            args.out.write_text(text + "\n", "utf-8")
    finally:
        if cdb:
            cdb.stop_apps()
            cdb.clean_workspace()
        for p in procs:
            p.terminate()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
